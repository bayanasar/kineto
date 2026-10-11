//! OAuth 2.1 for agents that connect from the internet (ADR-0011), as MCP's
//! authorization specification and ChatGPT's and Claude's connectors ask:
//! protected resource metadata (RFC 9728), authorization server metadata
//! (RFC 8414), dynamic client registration (RFC 7591), the authorization
//! code flow with PKCE S256 only, and refresh tokens that turn over with
//! each use.
//!
//! The director is the authorization: an agent's authorization request
//! waits until the director allows it in Kineto's window, never on a web
//! page anyone could open. Tokens are random, and only their SHA-256
//! hashes are kept, in `mcp-agents.json`, readable by its owner alone.

use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    desk::{Desk, Verdict},
    http::{Request, Response, encode, field},
};

/// The file that keeps the agents allowed to connect from the internet.
pub const AGENTS_FILE: &str = "mcp-agents.json";
/// The one scope there is: everything the tools do.
pub const SCOPE: &str = "kineto";
/// An access token's life, in seconds.
const ACCESS_SECONDS: u64 = 60 * 60;
const CODE_LIFETIME: Duration = Duration::from_secs(60);
/// How long an authorization waits for the director.
const PENDING_LIFETIME: Duration = Duration::from_secs(10 * 60);
/// How long an authorization page waits for the director before it
/// reloads itself.
const PAGE_WAIT: Duration = Duration::from_secs(2);
const MAX_CLIENTS: usize = 64;
const MAX_GRANTS: usize = 5;
/// How many agents may wait for the director at once; anyone can ask
/// through the public address, and each request reaches the window.
const MAX_PENDING: usize = 5;
const MAX_REDIRECTS: usize = 5;
const MAX_URI_CHARS: usize = 512;
const MAX_NAME_CHARS: usize = 60;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Stored {
    #[serde(default)]
    clients: Vec<Client>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Client {
    id: String,
    name: String,
    redirect_uris: Vec<String>,
    /// When it registered, in Unix seconds.
    registered: u64,
    #[serde(default)]
    grants: Vec<Grant>,
}

/// One authorization the director allowed, by its tokens' hashes.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Grant {
    refresh: String,
    access: String,
    /// When the access token expires, in Unix seconds.
    access_expires: u64,
    /// When the director allowed it, in Unix seconds.
    since: u64,
}

/// An agent the director allowed to connect, as Settings list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ConnectedAgent {
    pub id: String,
    pub name: String,
    /// Where it sends the director back to, such as `chatgpt.com`.
    pub site: String,
    /// When the director first allowed it, in Unix seconds.
    pub since: u64,
}

#[derive(Debug)]
struct Pending {
    id: String,
    client: String,
    redirect_uri: String,
    state: Option<String>,
    challenge: String,
    ask: u64,
    at: Instant,
}

#[derive(Debug)]
struct Code {
    hash: String,
    client: String,
    redirect_uri: String,
    challenge: String,
    at: Instant,
}

/// The agents allowed to connect from the internet, and the
/// authorizations under way.
#[derive(Debug)]
pub struct Agents {
    path: Option<PathBuf>,
    stored: Mutex<Stored>,
    pending: Mutex<Vec<Pending>>,
    codes: Mutex<Vec<Code>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `bytes` random bytes, as URL-safe text.
fn random(bytes: usize) -> io::Result<String> {
    let mut buffer = vec![0_u8; bytes];
    getrandom::getrandom(&mut buffer).map_err(|_| io::Error::other("no randomness"))?;
    Ok(URL_SAFE_NO_PAD.encode(buffer))
}

fn hash(secret: &str) -> String {
    Sha256::digest(secret.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
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

/// The host a redirect URI sends the director to.
fn site(uri: &str) -> String {
    let rest = uri.split_once("://").map_or(uri, |(_, rest)| rest);
    rest.split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// Whether `uri` may receive an authorization code: HTTPS anywhere, or
/// HTTP to this computer for an agent on it, and no fragment.
fn redirect_allowed(uri: &str) -> bool {
    if uri.len() > MAX_URI_CHARS || uri.contains('#') || uri.chars().any(char::is_control) {
        return false;
    }
    let host = site(uri);
    if host.is_empty() || host.contains(['@', '\\']) {
        return false;
    }
    uri.starts_with("https://")
        || (uri.starts_with("http://")
            && ["localhost", "127.0.0.1", "[::1]"]
                .iter()
                .any(|name| host == *name || host.starts_with(&format!("{name}:"))))
}

/// `text` as HTML text.
fn escape(text: &str) -> String {
    text.chars()
        .map(|character| match character {
            '&' => "&amp;".to_owned(),
            '<' => "&lt;".to_owned(),
            '>' => "&gt;".to_owned(),
            '"' => "&quot;".to_owned(),
            '\'' => "&#39;".to_owned(),
            other => other.to_string(),
        })
        .collect()
}

fn page(title: &str, words: &str, refresh: Option<&str>) -> String {
    let refresh = refresh.map_or_else(String::new, |to| {
        format!(
            "<meta http-equiv=\"refresh\" content=\"{}; url={}\">",
            PAGE_WAIT.as_secs(),
            escape(to)
        )
    });
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{refresh}\
<title>Kineto</title><style>body{{font:16px/1.5 system-ui,sans-serif;max-width:32rem;\
margin:15vh auto;padding:0 16px;color:#222;background:#fafaf7}}\
@media (prefers-color-scheme:dark){{body{{color:#ddd;background:#1b1b1a}}}}\
h1{{font-size:1.25rem}}</style></head><body><h1>{}</h1><p>{}</p></body></html>",
        escape(title),
        escape(words)
    )
}

fn oauth_error(status: u16, error: &str, description: &str) -> Response {
    Response::json(
        status,
        &json!({"error": error, "error_description": description}),
    )
}

/// Send the director back to `redirect_uri` with `fields`.
fn back(redirect_uri: &str, fields: &[(&str, &str)]) -> Response {
    let separator = if redirect_uri.contains('?') { '&' } else { '?' };
    let query = fields
        .iter()
        .map(|(name, value)| format!("{name}={}", encode(value)))
        .collect::<Vec<_>>()
        .join("&");
    Response::redirect(format!("{redirect_uri}{separator}{query}"))
}

impl Agents {
    /// The agents kept at `path`; none when there is no file.
    pub fn load(path: &Path) -> io::Result<Self> {
        let stored = match fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not agents"))?,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Stored::default(),
            Err(error) => return Err(error),
        };
        Ok(Self {
            path: Some(path.to_path_buf()),
            stored: Mutex::new(stored),
            pending: Mutex::new(Vec::new()),
            codes: Mutex::new(Vec::new()),
        })
    }

    /// Agents kept nowhere, for tests.
    #[must_use]
    pub fn in_memory() -> Self {
        Self {
            path: None,
            stored: Mutex::new(Stored::default()),
            pending: Mutex::new(Vec::new()),
            codes: Mutex::new(Vec::new()),
        }
    }

    fn save(&self, stored: &Stored) -> io::Result<()> {
        let Some(path) = &self.path else {
            return Ok(());
        };
        let text = serde_json::to_string_pretty(stored).map_err(io::Error::other)?;
        if let Some(directory) = path.parent() {
            fs::create_dir_all(directory)?;
        }
        let temporary = path.with_extension("json.new");
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

    /// The agents the director allowed, in the order they first were.
    #[must_use]
    pub fn connected(&self) -> Vec<ConnectedAgent> {
        lock(&self.stored)
            .clients
            .iter()
            .filter_map(|client| {
                let since = client.grants.iter().map(|grant| grant.since).min()?;
                Some(ConnectedAgent {
                    id: client.id.clone(),
                    name: client.name.clone(),
                    site: client
                        .redirect_uris
                        .first()
                        .map(|uri| site(uri))
                        .unwrap_or_default(),
                    since,
                })
            })
            .collect()
    }

    /// Forget agent `id` and every token it holds: it must be allowed
    /// again. Returns whether there was one.
    pub fn disconnect(&self, id: &str) -> io::Result<bool> {
        let mut stored = lock(&self.stored);
        let before = stored.clients.len();
        stored.clients.retain(|client| client.id != id);
        if stored.clients.len() == before {
            return Ok(false);
        }
        self.save(&stored)?;
        Ok(true)
    }

    /// The agent that holds access token `token`, by name.
    #[must_use]
    pub fn bearer(&self, token: &str) -> Option<String> {
        let hashed = hash(token);
        let now = now();
        lock(&self.stored).clients.iter().find_map(|client| {
            client
                .grants
                .iter()
                .any(|grant| same(&grant.access, &hashed) && grant.access_expires > now)
                .then(|| client.name.clone())
        })
    }

    /// RFC 7591: an agent says who it is and where codes go.
    pub(crate) fn register(&self, request: &Request) -> Response {
        let Ok(body) = serde_json::from_slice::<Value>(&request.body) else {
            return oauth_error(400, "invalid_client_metadata", "the body is not JSON");
        };
        let redirect_uris: Vec<String> = body["redirect_uris"]
            .as_array()
            .map(|uris| {
                uris.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if redirect_uris.is_empty()
            || redirect_uris.len() > MAX_REDIRECTS
            || !redirect_uris.iter().all(|uri| redirect_allowed(uri))
        {
            return oauth_error(
                400,
                "invalid_redirect_uri",
                "redirect URIs must be HTTPS, or HTTP to this computer, without a fragment",
            );
        }
        let name: String = body["client_name"]
            .as_str()
            .unwrap_or_default()
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_NAME_CHARS)
            .collect::<String>()
            .trim()
            .to_owned();
        let name = if name.is_empty() {
            "An agent".to_owned()
        } else {
            name
        };
        let Ok(id) = random(18) else {
            return Response::empty(500);
        };
        let registered = now();
        let client = Client {
            id: id.clone(),
            name: name.clone(),
            redirect_uris: redirect_uris.clone(),
            registered,
            grants: Vec::new(),
        };
        let mut stored = lock(&self.stored);
        stored.clients.push(client);
        // The oldest an agent registered but the director never allowed go
        // first.
        while stored.clients.len() > MAX_CLIENTS {
            let index = stored
                .clients
                .iter()
                .position(|client| client.grants.is_empty())
                .unwrap_or(0);
            stored.clients.remove(index);
        }
        if self.save(&stored).is_err() {
            return Response::empty(500);
        }
        Response::json(
            201,
            &json!({
                "client_id": id,
                "client_id_issued_at": registered,
                "client_name": name,
                "redirect_uris": redirect_uris,
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
                "scope": SCOPE,
            }),
        )
    }

    /// The authorization endpoint: a new request waits for the director;
    /// its page reloads until the director has answered.
    pub(crate) fn authorize(&self, desk: &Desk, issuer: &str, request: &Request) -> Response {
        let fields = request.query_fields();
        if let Some(pending) = field(&fields, "pending") {
            return self.follow(desk, issuer, pending);
        }
        let client_id = field(&fields, "client_id").unwrap_or_default();
        let Some(client) = lock(&self.stored)
            .clients
            .iter()
            .find(|client| same(&client.id, client_id))
            .cloned()
        else {
            return Response::html(
                400,
                page(
                    "Kineto does not know this agent",
                    "Connect it again from the agent: it registers itself first.",
                    None,
                ),
            );
        };
        let redirect_uri = match field(&fields, "redirect_uri") {
            Some(uri) if client.redirect_uris.iter().any(|known| known == uri) => uri.to_owned(),
            None if client.redirect_uris.len() == 1 => client.redirect_uris[0].clone(),
            _ => {
                return Response::html(
                    400,
                    page(
                        "Kineto cannot send you back",
                        "The agent asked to send you to an address it did not register.",
                        None,
                    ),
                );
            }
        };
        let state = field(&fields, "state").map(str::to_owned);
        let refuse = |error: &str, description: &str| {
            let mut answer = vec![("error", error), ("error_description", description)];
            if let Some(state) = &state {
                answer.push(("state", state));
            }
            answer.push(("iss", issuer));
            back(&redirect_uri, &answer)
        };
        if field(&fields, "response_type") != Some("code") {
            return refuse("unsupported_response_type", "only code is supported");
        }
        let challenge = field(&fields, "code_challenge").unwrap_or_default();
        if field(&fields, "code_challenge_method") != Some("S256")
            || challenge.len() != 43
            || URL_SAFE_NO_PAD.decode(challenge).is_err()
        {
            return refuse("invalid_request", "PKCE with S256 is required");
        }
        if field(&fields, "resource").is_some_and(|resource| {
            resource != issuer && !resource.starts_with(&format!("{issuer}/"))
        }) {
            return refuse("invalid_target", "this server is the only resource");
        }
        let Ok(id) = random(18) else {
            return Response::empty(500);
        };
        let mut pending = lock(&self.pending);
        Self::lapse(desk, &mut pending);
        if pending.len() >= MAX_PENDING {
            return Response::html(
                429,
                page(
                    "Too many agents are waiting",
                    "Kineto's director has not answered the agents already asking. Try again later.",
                    None,
                ),
            );
        }
        let ask = desk.ask_to_connect(
            &client.name,
            format!(
                "connect from the internet, returning to {}",
                site(&redirect_uri)
            ),
        );
        pending.push(Pending {
            id: id.clone(),
            client: client.id,
            redirect_uri,
            state,
            challenge: challenge.to_owned(),
            ask,
            at: Instant::now(),
        });
        drop(pending);
        Self::waiting_page(&client.name, &id)
    }

    /// Forget authorizations that waited too long, and take their requests
    /// out of the window.
    fn lapse(desk: &Desk, pending: &mut Vec<Pending>) {
        pending.retain(|pending| {
            let waiting = pending.at.elapsed() < PENDING_LIFETIME;
            if !waiting {
                desk.withdraw(pending.ask);
            }
            waiting
        });
    }

    fn waiting_page(name: &str, pending: &str) -> Response {
        Response::html(
            200,
            page(
                &format!("Allow {name} in Kineto"),
                &format!(
                    "{name} asks to use your Kineto project. Switch to Kineto's window and choose \
                     Allow or Decline there. This page moves on by itself."
                ),
                Some(&format!("/authorize?pending={pending}")),
            ),
        )
    }

    /// Where authorization `pending` stands, once the director answered.
    fn follow(&self, desk: &Desk, issuer: &str, pending: &str) -> Response {
        let mut all = lock(&self.pending);
        Self::lapse(desk, &mut all);
        let Some(index) = all.iter().position(|known| same(&known.id, pending)) else {
            return Response::html(
                400,
                page(
                    "This request lapsed",
                    "Start connecting again from the agent.",
                    None,
                ),
            );
        };
        let ask = all[index].ask;
        drop(all);
        let verdict = desk.verdict(ask, PAGE_WAIT);
        let mut all = lock(&self.pending);
        let Some(index) = all.iter().position(|known| same(&known.id, pending)) else {
            return Response::html(400, page("This request lapsed", "Start again.", None));
        };
        if verdict == Verdict::Waiting {
            let client = all[index].client.clone();
            drop(all);
            let name = lock(&self.stored)
                .clients
                .iter()
                .find(|known| known.id == client)
                .map_or_else(|| "An agent".to_owned(), |known| known.name.clone());
            return Self::waiting_page(&name, pending);
        }
        let pending = all.remove(index);
        drop(all);
        let mut answer: Vec<(&str, String)> = Vec::new();
        match verdict {
            Verdict::Allowed => {
                let Ok(code) = random(32) else {
                    return Response::empty(500);
                };
                let mut codes = lock(&self.codes);
                codes.retain(|code| code.at.elapsed() < CODE_LIFETIME);
                codes.push(Code {
                    hash: hash(&code),
                    client: pending.client.clone(),
                    redirect_uri: pending.redirect_uri.clone(),
                    challenge: pending.challenge.clone(),
                    at: Instant::now(),
                });
                answer.push(("code", code));
            }
            _ => {
                answer.push(("error", "access_denied".to_owned()));
                answer.push((
                    "error_description",
                    "the director declined it in Kineto".to_owned(),
                ));
            }
        }
        if let Some(state) = pending.state {
            answer.push(("state", state));
        }
        answer.push(("iss", issuer.to_owned()));
        let fields: Vec<(&str, &str)> = answer
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        back(&pending.redirect_uri, &fields)
    }

    /// The token endpoint: a code and its verifier, or a refresh token, for
    /// new tokens.
    pub(crate) fn token(&self, issuer: &str, request: &Request) -> Response {
        if !request
            .header("content-type")
            .is_some_and(|kind| kind.starts_with("application/x-www-form-urlencoded"))
        {
            return oauth_error(400, "invalid_request", "send a form");
        }
        let fields = request.form();
        // A public client names itself in the form, or as Basic's user.
        let basic = request
            .header("authorization")
            .and_then(|value| value.strip_prefix("Basic "))
            .and_then(|value| STANDARD.decode(value.trim()).ok())
            .and_then(|value| String::from_utf8(value).ok())
            .and_then(|value| value.split_once(':').map(|(id, _)| decode_basic(id)));
        let client_id = field(&fields, "client_id")
            .map(str::to_owned)
            .or(basic)
            .unwrap_or_default();
        if field(&fields, "resource").is_some_and(|resource| {
            resource != issuer && !resource.starts_with(&format!("{issuer}/"))
        }) {
            return oauth_error(400, "invalid_target", "this server is the only resource");
        }
        match field(&fields, "grant_type") {
            Some("authorization_code") => {
                let code = field(&fields, "code").unwrap_or_default();
                let verifier = field(&fields, "code_verifier").unwrap_or_default();
                let mut codes = lock(&self.codes);
                codes.retain(|code| code.at.elapsed() < CODE_LIFETIME);
                let hashed = hash(code);
                let Some(index) = codes.iter().position(|known| same(&known.hash, &hashed)) else {
                    return oauth_error(400, "invalid_grant", "the code is unknown or used");
                };
                // A code is used once, whatever comes of it.
                let known = codes.remove(index);
                drop(codes);
                if !same(&known.client, &client_id)
                    || field(&fields, "redirect_uri").is_some_and(|uri| uri != known.redirect_uri)
                {
                    return oauth_error(
                        400,
                        "invalid_grant",
                        "the code was not given to this client",
                    );
                }
                let valid_verifier = (43..=128).contains(&verifier.len())
                    && verifier
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"-._~".contains(&byte));
                if !valid_verifier
                    || !same(
                        &URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())),
                        &known.challenge,
                    )
                {
                    return oauth_error(400, "invalid_grant", "the code verifier does not match");
                }
                self.grant(&client_id, None)
            }
            Some("refresh_token") => {
                let refresh = hash(field(&fields, "refresh_token").unwrap_or_default());
                self.grant(&client_id, Some(&refresh))
            }
            _ => oauth_error(
                400,
                "unsupported_grant_type",
                "authorization_code or refresh_token",
            ),
        }
    }

    /// New tokens for `client_id`: a new grant, or the grant whose refresh
    /// token hashes to `refresh`, turned over.
    fn grant(&self, client_id: &str, refresh: Option<&str>) -> Response {
        let (Ok(access), Ok(next_refresh)) = (random(32), random(32)) else {
            return Response::empty(500);
        };
        let now = now();
        let mut stored = lock(&self.stored);
        let Some(client) = stored
            .clients
            .iter_mut()
            .find(|client| same(&client.id, client_id))
        else {
            return oauth_error(400, "invalid_grant", "the client is unknown");
        };
        let grant = Grant {
            refresh: hash(&next_refresh),
            access: hash(&access),
            access_expires: now + ACCESS_SECONDS,
            since: now,
        };
        match refresh {
            Some(refresh) => {
                let Some(known) = client
                    .grants
                    .iter_mut()
                    .find(|known| same(&known.refresh, refresh))
                else {
                    return oauth_error(400, "invalid_grant", "the refresh token is unknown");
                };
                *known = Grant {
                    since: known.since,
                    ..grant
                };
            }
            None => {
                client.grants.push(grant);
                if client.grants.len() > MAX_GRANTS {
                    client.grants.remove(0);
                }
            }
        }
        if self.save(&stored).is_err() {
            return Response::empty(500);
        }
        Response::json(
            200,
            &json!({
                "access_token": access,
                "token_type": "Bearer",
                "expires_in": ACCESS_SECONDS,
                "refresh_token": next_refresh,
                "scope": SCOPE,
            }),
        )
        .with("Pragma", "no-cache".to_owned())
    }
}

/// A Basic user name, which is form-encoded.
fn decode_basic(id: &str) -> String {
    crate::http::form_fields(&format!("id={id}"))
        .pop()
        .map(|(_, value)| value)
        .unwrap_or_default()
}

/// RFC 9728: where agents learn who authorizes them.
pub(crate) fn resource_metadata(issuer: &str) -> Value {
    json!({
        "resource": format!("{issuer}/mcp"),
        "authorization_servers": [issuer],
        "scopes_supported": [SCOPE],
        "bearer_methods_supported": ["header"],
        "resource_name": "Kineto",
    })
}

/// RFC 8414: how this server authorizes.
pub(crate) fn server_metadata(issuer: &str) -> Value {
    json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "registration_endpoint": format!("{issuer}/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
        "scopes_supported": [SCOPE],
        "authorization_response_iss_parameter_supported": true,
    })
}
