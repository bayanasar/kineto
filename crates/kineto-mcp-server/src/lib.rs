//! Kineto as an MCP server (ADR-0010, ADR-0011): agents outside Kineto
//! (Claude Code, Codex, Claude Desktop, ChatGPT, claude.ai, any MCP client)
//! read and change the project open in Kineto through tools, over MCP's
//! Streamable HTTP transport.
//!
//! Agents on this computer connect to `127.0.0.1`, at an address whose
//! path holds a random 256-bit token (`/mcp/<64 hex digits>`); anything
//! else is answered 404. A request whose `Host` is not this computer, or
//! whose `Origin` is a web page elsewhere, is refused, so a page in a
//! browser cannot reach it by rebinding a name to `127.0.0.1`.
//!
//! Agents in the cloud connect through a tunnel the director runs, to a
//! second listener that takes OAuth access tokens only ([`oauth`]): the
//! director allows each agent in Kineto's window.
//!
//! Reading tools open the project read-only, beside the session that
//! writes it. Changing tools go through that session, as the director's
//! buttons do, and a paid one waits for the director ([`Desk`]).
//!
//! The director turns it on and off in Settings. Whether it is on, its
//! ports, its token and the public address are kept in `mcp-server.toml`
//! beside the provider settings, readable by its owner alone, so the
//! address stays the same across restarts until the director asks for a
//! new one.

mod desk;
mod http;
pub mod oauth;
mod tools;

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub use desk::{ANSWER_WAIT, Charge, Desk, Verdict, Waiting, Work};
pub use oauth::{AGENTS_FILE, Agents, ConnectedAgent};
pub use tools::Tools;

use http::{Answer, Listener, Request, Response};

/// The file that keeps whether the server is on, its ports and its token.
pub const SERVER_FILE: &str = "mcp-server.toml";
/// The MCP versions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
/// How long a tunnel command may be, in words.
const MAX_TUNNEL_WORDS: usize = 32;
const MAX_URL_CHARS: usize = 256;

/// Whether the server is on, and the addresses it keeps.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerSettings {
    #[serde(default)]
    pub enabled: bool,
    /// 0 until it first listens.
    #[serde(default)]
    pub port: u16,
    /// 64 hex digits; empty until it is first turned on.
    #[serde(default)]
    pub token: String,
    /// Whether agents in the cloud may connect, through `public_url`.
    #[serde(default)]
    pub public: bool,
    /// The HTTPS origin the director's tunnel answers at, such as
    /// `https://example.ts.net:10000`; empty until the director gives one.
    #[serde(default)]
    pub public_url: String,
    /// The port the tunnel forwards to; 0 until it first listens.
    #[serde(default)]
    pub public_port: u16,
    /// The command that opens the tunnel while the public address is on,
    /// word by word; `{port}` stands for `public_port`. Empty when the
    /// director runs the tunnel some other way.
    #[serde(default)]
    pub tunnel: Vec<String>,
}

impl ServerSettings {
    /// Where they are kept: beside the provider settings.
    #[must_use]
    pub fn default_path() -> Option<PathBuf> {
        kineto_plugins::settings::default_path().map(|path| path.with_file_name(SERVER_FILE))
    }

    /// The settings at `path`; off, with no address, when there are none.
    pub fn load(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not settings")),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error),
        }
    }

    /// Keep them at `path`, readable by their owner alone.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        let text = toml::to_string(self)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not settings"))?;
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let temporary = path.with_extension("toml.new");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temporary)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    }

    /// A new token, so the old address stops working.
    pub fn renew_token(&mut self) -> io::Result<()> {
        let mut bytes = [0_u8; 32];
        getrandom::getrandom(&mut bytes).map_err(|_| io::Error::other("no randomness"))?;
        self.token = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(())
    }

    /// The address an agent on this computer connects to; empty until it
    /// has listened.
    #[must_use]
    pub fn address(&self) -> String {
        if self.port == 0 || self.token.is_empty() {
            String::new()
        } else {
            format!("http://127.0.0.1:{}/mcp/{}", self.port, self.token)
        }
    }

    /// The address an agent in the cloud connects to; empty without a
    /// public origin.
    #[must_use]
    pub fn public_address(&self) -> String {
        if self.public_url.is_empty() {
            String::new()
        } else {
            format!("{}/mcp", self.public_url)
        }
    }

    /// The tunnel command with the public port in it.
    #[must_use]
    pub fn tunnel_command(&self) -> Vec<String> {
        self.tunnel
            .iter()
            .map(|word| word.replace("{port}", &self.public_port.to_string()))
            .collect()
    }
}

/// `url` as a public origin: HTTPS, a host and maybe a port, nothing more.
/// A trailing slash is dropped; anything else is refused.
pub fn public_origin(url: &str) -> Result<String, &'static str> {
    let url = url.trim().trim_end_matches('/');
    let Some(authority) = url.strip_prefix("https://") else {
        return Err("the public address must start with https://");
    };
    if url.len() > MAX_URL_CHARS
        || authority.is_empty()
        || authority.contains(['/', '?', '#', '@', '\\'])
        || authority
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("the public address must be a host, and a port if any, with no path");
    }
    Ok(url.to_owned())
}

/// `words` as a tunnel command: a program and its arguments, each one
/// line of text.
pub fn tunnel_words(words: &[String]) -> Result<Vec<String>, &'static str> {
    if words.len() > MAX_TUNNEL_WORDS
        || words.iter().any(|word| {
            word.is_empty() || word.len() > MAX_URL_CHARS || word.chars().any(char::is_control)
        })
    {
        return Err("the tunnel command is a program and its arguments, one per line");
    }
    Ok(words.to_vec())
}

/// Whether `a` and `b` are equal, in time that does not depend on where
/// they first differ.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0_u8, |difference, (x, y)| difference | (x ^ y))
            == 0
}

fn rpc_error(code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": null, "error": {"code": code, "message": message}})
}

/// One JSON-RPC message posted to an MCP endpoint, from `agent`.
fn mcp(tools: &Tools, request: &Request, agent: &str) -> Response {
    if request.method != "POST" {
        return Response::empty(405).with("Allow", "POST".to_owned());
    }
    if !request
        .header("content-type")
        .is_some_and(|kind| kind.starts_with("application/json"))
    {
        return Response::empty(415);
    }
    if request
        .header("mcp-protocol-version")
        .is_some_and(|version| !PROTOCOL_VERSIONS.contains(&version))
    {
        return Response::json(400, &rpc_error(-32600, "unsupported MCP protocol version"));
    }
    let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
        return Response::json(400, &rpc_error(-32700, "the body is not JSON"));
    };
    match tools.respond(&message, agent) {
        Some(answer) => Response::json(200, &answer),
        None => Response::empty(202),
    }
}

/// The server for agents on this computer, listening until it is dropped.
#[derive(Debug)]
pub struct Server {
    listener: Listener,
}

impl std::fmt::Debug for Listener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Listener")
            .field("port", &self.port())
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Listen on `127.0.0.1:port` (any free port for 0) at the path that
    /// holds `token`, reaching the project through `desk`.
    pub fn start(port: u16, token: &str, desk: Arc<Desk>) -> io::Result<Self> {
        if token.len() < 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the token is not one this server made",
            ));
        }
        let path = format!("/mcp/{token}");
        // The port is known once bound; the handler learns it then.
        let local = Arc::new(LocalSlot::default());
        let listener = Listener::start(
            port,
            "kineto-mcp-server",
            Arc::clone(&local) as Arc<dyn Answer>,
        )?;
        local.set(Local {
            port: listener.port(),
            path,
            tools: Tools::new(desk),
        });
        Ok(Self { listener })
    }

    /// The port it listens on.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.listener.port()
    }
}

/// The local handler, set once its listener knows its port.
#[derive(Default)]
struct LocalSlot(std::sync::OnceLock<Local>);

impl LocalSlot {
    fn set(&self, local: Local) {
        let _ = self.0.set(local);
    }
}

impl Answer for LocalSlot {
    fn answer(&self, request: &Request) -> Response {
        match self.0.get() {
            Some(local) => local.answer(request),
            None => Response::empty(503),
        }
    }
}

struct Local {
    port: u16,
    /// `/mcp/<token>`.
    path: String,
    tools: Tools,
}

impl Local {
    /// Whether `host` names this computer at this server's port.
    fn local_host(&self, host: &str) -> bool {
        ["127.0.0.1", "localhost", "[::1]"]
            .iter()
            .any(|name| host == format!("{name}:{}", self.port))
    }

    fn answer(&self, request: &Request) -> Response {
        if !request
            .header("host")
            .is_some_and(|host| self.local_host(host))
        {
            return Response::empty(403);
        }
        if let Some(origin) = request.header("origin") {
            let local = origin.strip_prefix("http://").is_some_and(|host| {
                self.local_host(host) || matches!(host, "127.0.0.1" | "localhost" | "[::1]")
            });
            if !local {
                return Response::empty(403);
            }
        }
        if !same(&request.path, &self.path) {
            return Response::empty(404);
        }
        mcp(&self.tools, request, &local_agent(request))
    }
}

/// What an agent on this computer is called: the program its user agent
/// names, as in `claude-code (on this computer)`.
fn local_agent(request: &Request) -> String {
    let program: String = request
        .header("user-agent")
        .and_then(|agent| agent.split(['/', ' ']).next())
        .unwrap_or_default()
        .chars()
        .filter(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        .take(40)
        .collect();
    if program.is_empty() {
        "An agent on this computer".to_owned()
    } else {
        format!("{program} (on this computer)")
    }
}

/// The server for agents in the cloud, behind the director's tunnel,
/// listening until it is dropped.
#[derive(Debug)]
pub struct PublicServer {
    listener: Listener,
}

impl PublicServer {
    /// Listen on `127.0.0.1:port` (any free port for 0) for the tunnel to
    /// `issuer`, the public origin, taking OAuth access tokens that
    /// `agents` issued.
    pub fn start(
        port: u16,
        issuer: &str,
        desk: Arc<Desk>,
        agents: Arc<Agents>,
    ) -> io::Result<Self> {
        let issuer = public_origin(issuer)
            .map_err(|why| io::Error::new(io::ErrorKind::InvalidInput, why))?;
        let public = Arc::new(Public {
            tools: Tools::new(Arc::clone(&desk)),
            issuer,
            desk,
            agents,
        });
        Ok(Self {
            listener: Listener::start(port, "kineto-mcp-public", public)?,
        })
    }

    /// The port it listens on.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.listener.port()
    }
}

struct Public {
    issuer: String,
    desk: Arc<Desk>,
    agents: Arc<Agents>,
    tools: Tools,
}

impl Answer for Public {
    fn answer(&self, request: &Request) -> Response {
        // Agents in the cloud call from servers; a web page elsewhere may
        // not call at all.
        if request
            .header("origin")
            .is_some_and(|origin| origin != self.issuer)
        {
            return Response::empty(403);
        }
        let get = request.method == "GET";
        let post = request.method == "POST";
        match request.path.as_str() {
            "/.well-known/oauth-protected-resource"
            | "/.well-known/oauth-protected-resource/mcp"
                if get =>
            {
                Response::json(200, &oauth::resource_metadata(&self.issuer))
            }
            "/.well-known/oauth-authorization-server"
            | "/.well-known/oauth-authorization-server/mcp"
            | "/.well-known/openid-configuration"
                if get =>
            {
                Response::json(200, &oauth::server_metadata(&self.issuer))
            }
            "/register" if post => self.agents.register(request),
            "/authorize" if get => self.agents.authorize(&self.desk, &self.issuer, request),
            "/token" if post => self.agents.token(&self.issuer, request),
            "/mcp" => {
                let challenge = |error: Option<&str>| {
                    let mut value = format!(
                        "Bearer resource_metadata=\"{}/.well-known/oauth-protected-resource/mcp\", scope=\"{}\"",
                        self.issuer,
                        oauth::SCOPE
                    );
                    if let Some(error) = error {
                        value.push_str(&format!(", error=\"{error}\""));
                    }
                    Response::empty(401).with("WWW-Authenticate", value)
                };
                let Some(token) = request
                    .header("authorization")
                    .and_then(|value| value.strip_prefix("Bearer "))
                else {
                    return challenge(None);
                };
                match self.agents.bearer(token.trim()) {
                    Some(agent) => mcp(&self.tools, request, &agent),
                    None => challenge(Some("invalid_token")),
                }
            }
            _ => Response::empty(404),
        }
    }
}

#[cfg(test)]
mod tests;
