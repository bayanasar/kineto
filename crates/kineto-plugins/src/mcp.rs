//! A Model Context Protocol client for the servers the user registered
//! (ADR-0007), in `<config>/kineto/mcp.toml`:
//!
//! ```toml
//! [[server]]
//! id = "studio-archive"
//! executable = "/opt/studio/bin/archive-mcp"
//! args = []
//! secrets = ["ARCHIVE_TOKEN"]
//! tools = ["search_wardrobe", "location_notes"]
//! enabled = true
//! ```
//!
//! Kineto is a client only, over stdio, and offers nothing back: a server's
//! request for sampling, roots or elicitation is answered with an error, so
//! it can neither ask Kineto's model anything nor see its files. Each use
//! starts the server under the same rules as a provider plugin (only the
//! configured absolute executable, no shell, a cleared environment with the
//! named secrets, a scratch working directory outside any project),
//! initializes it, makes one request, and stops it. Only tools the entry
//! allows are listed or called; anything else is refused before the server
//! starts. Every message is capped, every exchange has a deadline, and what
//! a server answers is data, never an instruction to Kineto.

use std::{
    collections::BTreeSet,
    env, fmt, fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::{
    config::{MAX_ID, portable, secret_name},
    host::{Scratch, search_path},
};

/// The configuration file's name, in Kineto's configuration directory.
pub const MCP_CONFIG_FILE: &str = "mcp.toml";

/// The protocol revision Kineto asks for.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Revisions whose tools Kineto can use.
const KNOWN_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];

/// The most pages of tools one listing reads.
const MAX_PAGES: usize = 16;

/// One registered server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerEntry {
    pub id: String,
    /// Absolute path of the executable to start.
    pub executable: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Names of the secrets passed to it, as environment variables of the
    /// same names.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// The tools Kineto may call; no other is offered or called.
    pub tools: Vec<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

const fn enabled() -> bool {
    true
}

impl McpServerEntry {
    #[must_use]
    pub fn allows(&self, tool: &str) -> bool {
        self.enabled && self.tools.iter().any(|allowed| allowed == tool)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default, rename = "server")]
    servers: Vec<McpServerEntry>,
}

/// The registered servers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpConfig {
    servers: Vec<McpServerEntry>,
}

impl McpConfig {
    /// Read the configuration in `text`. Every ID is portable and unique,
    /// every executable an absolute path, every secret a variable name, and
    /// every server allows at least one named tool.
    pub fn parse(text: &str) -> Result<Self, crate::ConfigError> {
        use crate::ConfigError;
        let file: File =
            toml::from_str(text).map_err(|error| ConfigError::Unreadable(error.to_string()))?;
        let mut seen = BTreeSet::new();
        for server in &file.servers {
            if !portable(&server.id) {
                return Err(ConfigError::Invalid(format!(
                    "{:?} is not a server ID: lowercase letters, digits, - and _, at most {MAX_ID}",
                    server.id
                )));
            }
            if !seen.insert(server.id.as_str()) {
                return Err(ConfigError::Invalid(format!(
                    "server {:?} is registered twice",
                    server.id
                )));
            }
            if !server.executable.is_absolute() {
                return Err(ConfigError::Invalid(format!(
                    "server {:?} names a relative executable; it must be an absolute path",
                    server.id
                )));
            }
            if let Some(name) = server.secrets.iter().find(|name| !secret_name(name)) {
                return Err(ConfigError::Invalid(format!(
                    "server {:?} names {name:?} as a secret, which is not a variable name",
                    server.id
                )));
            }
            if server.tools.is_empty() || server.tools.iter().any(|tool| tool.trim().is_empty()) {
                return Err(ConfigError::Invalid(format!(
                    "server {:?} must allow at least one tool, each by name",
                    server.id
                )));
            }
        }
        Ok(Self {
            servers: file.servers,
        })
    }

    /// Read the configuration at `path`; none when the file does not exist.
    pub fn load(path: &Path) -> Result<Self, crate::ConfigError> {
        match fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(crate::ConfigError::Unreadable(error.to_string())),
        }
    }

    /// The user's configuration, beside `plugins.toml`; none when there is
    /// no such file or no place for one.
    pub fn load_default() -> Result<Self, crate::ConfigError> {
        default_path().map_or_else(|| Ok(Self::default()), |path| Self::load(&path))
    }

    #[must_use]
    pub fn servers(&self) -> &[McpServerEntry] {
        &self.servers
    }

    /// The enabled server `id`, if one is registered.
    #[must_use]
    pub fn enabled(&self, id: &str) -> Option<&McpServerEntry> {
        self.servers
            .iter()
            .find(|server| server.id == id && server.enabled)
    }
}

/// Where the user's MCP configuration is, beside `plugins.toml`.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::config::default_path().map(|plugins| plugins.with_file_name(MCP_CONFIG_FILE))
}

/// How long and how much one use of a server may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct McpLimits {
    /// For starting and initializing it, and for listing its tools.
    pub quick: Duration,
    /// For one tool call.
    pub call: Duration,
    /// The longest message it may send, in bytes.
    pub max_message: usize,
}

impl McpLimits {
    pub const DEFAULT: Self = Self {
        quick: Duration::from_secs(30),
        call: Duration::from_secs(120),
        max_message: 1024 * 1024,
    };
}

/// A tool a server offers and the entry allows.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    /// The JSON Schema its arguments follow, as the server gave it.
    pub input_schema: Value,
}

/// What a tool call answered. Only text is kept; what else it sent is
/// named in `dropped`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub text: Vec<String>,
    /// The kinds of content dropped, such as `image`, in order.
    pub dropped: Vec<String>,
    /// Whether the server said the call failed.
    pub is_error: bool,
}

#[derive(Debug)]
pub enum McpError {
    /// The server or tool is not registered, enabled and allowed; nothing
    /// was started.
    NotAllowed(String),
    /// The server could not be started.
    Spawn(std::io::Error),
    /// Its scratch directory could not be prepared; nothing was started.
    Setup(std::io::Error),
    /// It ran past its deadline and was stopped.
    Timeout,
    /// It exited or closed its output before it answered.
    Exited,
    /// It sent a message longer than allowed.
    Oversized(usize),
    /// It answered outside the protocol, or with a revision Kineto does not
    /// know.
    Protocol(String),
    /// It answered the request with a JSON-RPC error.
    Server { code: i64, message: String },
}

impl fmt::Display for McpError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed(what) => write!(formatter, "not allowed: {what}"),
            Self::Spawn(error) => write!(formatter, "the MCP server could not be started: {error}"),
            Self::Setup(error) => write!(
                formatter,
                "the MCP server's directory could not be prepared: {error}"
            ),
            Self::Timeout => {
                formatter.write_str("the MCP server ran past its deadline and was stopped")
            }
            Self::Exited => formatter.write_str("the MCP server stopped before it answered"),
            Self::Oversized(cap) => write!(
                formatter,
                "the MCP server sent a message longer than {cap} bytes"
            ),
            Self::Protocol(reason) => {
                write!(formatter, "the MCP server broke the protocol: {reason}")
            }
            Self::Server { code, message } => {
                write!(formatter, "the MCP server refused ({code}): {message}")
            }
        }
    }
}

impl std::error::Error for McpError {}

/// What the reader thread passes on: a line, or why there are no more.
enum Line {
    Text(String),
    TooLong,
    Closed,
}

/// One running server, initialized. Stopped when dropped.
struct Connection {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Line>,
    next_id: u64,
    limits: McpLimits,
    _scratch: Scratch,
}

impl Drop for Connection {
    fn drop(&mut self) {
        // Closing its input is the stdio way to ask a server to stop; one
        // that does not is killed.
        drop(self.stdin.take());
        let started = Instant::now();
        while started.elapsed() < Duration::from_millis(200) {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_lines(stdout: impl Read + Send + 'static, cap: usize) -> Receiver<Line> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            let limit = u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1);
            match (&mut reader).take(limit).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => {
                    let _ = sender.send(Line::Closed);
                    return;
                }
                Ok(_) => {}
            }
            if line.last() != Some(&b'\n') && line.len() > cap {
                let _ = sender.send(Line::TooLong);
                return;
            }
            let text = String::from_utf8_lossy(&line).trim().to_owned();
            if !text.is_empty() && sender.send(Line::Text(text)).is_err() {
                return;
            }
        }
    });
    receiver
}

impl Connection {
    /// Start `entry` and initialize it.
    fn open(
        entry: &McpServerEntry,
        secret: &dyn Fn(&str) -> Option<String>,
        limits: McpLimits,
    ) -> Result<Self, McpError> {
        let scratch = Scratch::new().map_err(McpError::Setup)?;
        let mut command = Command::new(&entry.executable);
        command
            .args(&entry.args)
            .env_clear()
            .env("PATH", search_path())
            .env("HOME", &scratch.0)
            .current_dir(&scratch.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for name in &entry.secrets {
            if let Some(value) = secret(name) {
                command.env(name, value);
            }
        }
        let mut child = command.spawn().map_err(McpError::Spawn)?;
        let stdin = child.stdin.take();
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            return Err(McpError::Exited);
        };
        let mut connection = Self {
            child,
            stdin,
            lines: read_lines(stdout, limits.max_message),
            next_id: 0,
            limits,
            _scratch: scratch,
        };
        let initialized = connection.request(
            "initialize",
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "kineto", "version": env!("CARGO_PKG_VERSION")},
            }),
            limits.quick,
        )?;
        let version = initialized["protocolVersion"].as_str().unwrap_or_default();
        if !KNOWN_VERSIONS.contains(&version) {
            return Err(McpError::Protocol(format!(
                "it speaks MCP {version:?}, which Kineto does not"
            )));
        }
        connection.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(connection)
    }

    fn send(&mut self, message: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_vec(message)
            .map_err(|error| McpError::Protocol(format!("a message did not encode: {error}")))?;
        line.push(b'\n');
        self.stdin
            .as_mut()
            .ok_or(McpError::Exited)?
            .write_all(&line)
            .and_then(|()| self.stdin.as_mut().map_or(Ok(()), Write::flush))
            .map_err(|_| McpError::Exited)
    }

    /// Send `method` and wait for its answer, refusing whatever the server
    /// asks of Kineto meanwhile.
    fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Duration,
    ) -> Result<Value, McpError> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let started = Instant::now();
        loop {
            let left = deadline
                .checked_sub(started.elapsed())
                .ok_or(McpError::Timeout)?;
            let text = match self.lines.recv_timeout(left) {
                Ok(Line::Text(text)) => text,
                Ok(Line::TooLong) => return Err(McpError::Oversized(self.limits.max_message)),
                Ok(Line::Closed) | Err(RecvTimeoutError::Disconnected) => {
                    return Err(McpError::Exited);
                }
                Err(RecvTimeoutError::Timeout) => return Err(McpError::Timeout),
            };
            let message: Value = serde_json::from_str(&text)
                .map_err(|_| McpError::Protocol("a message is not JSON".to_owned()))?;
            if message["jsonrpc"] != "2.0" {
                return Err(McpError::Protocol(
                    "a message is not JSON-RPC 2.0".to_owned(),
                ));
            }
            match (
                message.get("method").and_then(Value::as_str),
                message.get("id"),
            ) {
                // A request of the server's. Kineto offers nothing but a
                // ping's answer.
                (Some(asked), Some(request_id)) => {
                    let answer = if asked == "ping" {
                        json!({"jsonrpc": "2.0", "id": request_id, "result": {}})
                    } else {
                        json!({"jsonrpc": "2.0", "id": request_id, "error": {
                            "code": -32601,
                            "message": format!("Kineto does not offer {asked}"),
                        }})
                    };
                    self.send(&answer)?;
                }
                // A notification: logging or progress. Nothing to do.
                (Some(_), None) => {}
                (None, Some(answered)) if answered.as_u64() == Some(id) => {
                    if let Some(error) = message.get("error") {
                        return Err(McpError::Server {
                            code: error["code"].as_i64().unwrap_or(0),
                            message: error["message"].as_str().unwrap_or_default().to_owned(),
                        });
                    }
                    return message
                        .get("result")
                        .cloned()
                        .ok_or_else(|| McpError::Protocol("an answer has no result".to_owned()));
                }
                // An answer to nothing Kineto asked.
                (None, _) => {
                    return Err(McpError::Protocol(
                        "it answered a request Kineto did not make".to_owned(),
                    ));
                }
            }
        }
    }
}

/// The tools `entry` offers that it allows, in the server's order.
pub fn list_tools(
    entry: &McpServerEntry,
    secret: &dyn Fn(&str) -> Option<String>,
    limits: McpLimits,
) -> Result<Vec<ToolInfo>, McpError> {
    if !entry.enabled {
        return Err(McpError::NotAllowed(format!(
            "server {:?} is disabled",
            entry.id
        )));
    }
    let mut connection = Connection::open(entry, secret, limits)?;
    let mut tools = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let params = cursor
            .as_ref()
            .map_or_else(|| json!({}), |cursor| json!({ "cursor": cursor }));
        let page = connection.request("tools/list", params, limits.quick)?;
        let listed = page["tools"]
            .as_array()
            .ok_or_else(|| McpError::Protocol("tools/list answered no tools".to_owned()))?;
        for tool in listed {
            let name = tool["name"]
                .as_str()
                .ok_or_else(|| McpError::Protocol("a tool has no name".to_owned()))?;
            if entry.allows(name) {
                tools.push(ToolInfo {
                    name: name.to_owned(),
                    description: tool["description"].as_str().unwrap_or_default().to_owned(),
                    input_schema: tool
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                });
            }
        }
        match page["nextCursor"].as_str() {
            Some(next) if !next.is_empty() => cursor = Some(next.to_owned()),
            _ => return Ok(tools),
        }
    }
    Ok(tools)
}

/// Call `tool` of `entry` with `arguments`, a JSON object. A tool the entry
/// does not allow is refused before the server starts.
pub fn call_tool(
    entry: &McpServerEntry,
    secret: &dyn Fn(&str) -> Option<String>,
    tool: &str,
    arguments: &Value,
    limits: McpLimits,
) -> Result<ToolResult, McpError> {
    if !entry.allows(tool) {
        return Err(McpError::NotAllowed(format!(
            "server {:?} does not allow tool {tool:?}",
            entry.id
        )));
    }
    if !arguments.is_object() {
        return Err(McpError::NotAllowed(
            "a tool's arguments are a JSON object".to_owned(),
        ));
    }
    let mut connection = Connection::open(entry, secret, limits)?;
    let answered = connection.request(
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
        limits.call,
    )?;
    let content = answered["content"]
        .as_array()
        .ok_or_else(|| McpError::Protocol("a tool answered no content".to_owned()))?;
    let mut text = Vec::new();
    let mut dropped = Vec::new();
    for item in content {
        match (item["type"].as_str(), item["text"].as_str()) {
            (Some("text"), Some(said)) => text.push(said.to_owned()),
            (Some(kind), _) => dropped.push(kind.to_owned()),
            (None, _) => return Err(McpError::Protocol("content has no type".to_owned())),
        }
    }
    Ok(ToolResult {
        text,
        dropped,
        is_error: answered["isError"].as_bool().unwrap_or(false),
    })
}

/// A secret the entry names, read from the environment only: the
/// development source. The desktop shell reads the system's secure storage
/// first (`kineto_core::plugin::read_secrets_with`).
#[must_use]
pub fn environment_secret(name: &str) -> Option<String> {
    env::var(name).ok()
}
