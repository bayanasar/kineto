//! Kineto as an MCP server (ADR-0010): agents outside Kineto (Claude Code,
//! Codex, Claude Desktop, any MCP client) read the project open in Kineto
//! through tools, over MCP's Streamable HTTP transport.
//!
//! It serves this computer only. It listens on `127.0.0.1`, at an address
//! whose path holds a random 256-bit token (`/mcp/<64 hex digits>`), and
//! answers anything else with 404. A request whose `Host` is not this
//! computer, or whose `Origin` is a web page elsewhere, is refused, so a
//! page in a browser cannot reach it by rebinding a name to `127.0.0.1`.
//!
//! The tools only read. Each opens the project read-only, beside the
//! session that writes it, and reads files that session replaces whole.
//!
//! The director turns it on and off in Settings. Whether it is on, its
//! port and its token are kept in `mcp-server.toml` beside the provider
//! settings, readable by its owner alone, so the address stays the same
//! across restarts until the director asks for a new one.

use std::{
    fmt, fs,
    io::{self, BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use kineto_core::workflow::{self, Stage, StepState};
use kineto_project::{
    fs::ProjectRelativePath,
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, RevisionStatus},
    shot::{ShotKey, ShotWorkflow, VideoState},
    shot_plan::shots_in_play,
    source,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The file that keeps whether the server is on, its port and its token.
pub const SERVER_FILE: &str = "mcp-server.toml";
/// The MCP versions this server speaks, newest first.
pub const PROTOCOL_VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
const MAX_HEAD_BYTES: usize = 16 * 1024;
const MAX_BODY_BYTES: usize = 1024 * 1024;
/// The most source text one answer carries, in characters.
const MAX_SOURCE_CHARS: usize = 200_000;
const READ_TIMEOUT: Duration = Duration::from_secs(10);
const ACCEPT_INTERVAL: Duration = Duration::from_millis(100);

/// The project the tools read: the one open in Kineto, if any.
pub type ProjectSlot = Arc<Mutex<Option<PathBuf>>>;

/// Whether the server is on, and the address it keeps.
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

    /// The address an agent connects to; empty until it has listened.
    #[must_use]
    pub fn address(&self) -> String {
        if self.port == 0 || self.token.is_empty() {
            String::new()
        } else {
            format!("http://127.0.0.1:{}/mcp/{}", self.port, self.token)
        }
    }
}

/// The server, listening until it is dropped.
pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl fmt::Debug for Server {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Server")
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

impl Server {
    /// Listen on `127.0.0.1:port` (any free port for 0) at the path that
    /// holds `token`, reading the project in `project`.
    pub fn start(port: u16, token: &str, project: ProjectSlot) -> io::Result<Self> {
        if token.len() < 32 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the token is not one this server made",
            ));
        }
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port)))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let handler = Arc::new(Handler {
            port,
            path: format!("/mcp/{token}"),
            tools: Tools { project },
        });
        let stopping = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("kineto-mcp-server".to_owned())
            .spawn(move || {
                while !stopping.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let handler = Arc::clone(&handler);
                            let _ = thread::Builder::new()
                                .name("kineto-mcp-request".to_owned())
                                .spawn(move || handler.serve(stream));
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(ACCEPT_INTERVAL);
                        }
                        Err(_) => thread::sleep(ACCEPT_INTERVAL),
                    }
                }
            })?;
        Ok(Self {
            port,
            stop,
            thread: Some(thread),
        })
    }

    /// The port it listens on.
    #[must_use]
    pub const fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Handler {
    port: u16,
    /// `/mcp/<token>`.
    path: String,
    tools: Tools,
}

/// One HTTP request, as far as this server reads one.
struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
    }
}

/// Why a request was not read.
enum Unread {
    TooLarge,
    LengthRequired,
    Malformed,
}

fn read_request(stream: &TcpStream) -> Result<Request, Unread> {
    let mut reader = BufReader::new(stream.take((MAX_HEAD_BYTES + MAX_BODY_BYTES) as u64));
    let mut head = 0;
    let mut line = String::new();
    let mut next_line = |line: &mut String| -> Result<(), Unread> {
        line.clear();
        let read = reader.read_line(line).map_err(|_| Unread::Malformed)?;
        head += read;
        if read == 0 {
            Err(Unread::Malformed)
        } else if head > MAX_HEAD_BYTES {
            Err(Unread::TooLarge)
        } else {
            Ok(())
        }
    };
    next_line(&mut line)?;
    let mut words = line.split_whitespace();
    let (Some(method), Some(path), Some(_)) = (words.next(), words.next(), words.next()) else {
        return Err(Unread::Malformed);
    };
    let (method, path) = (method.to_owned(), path.to_owned());
    let mut headers = Vec::new();
    loop {
        next_line(&mut line)?;
        let header = line.trim_end_matches(['\r', '\n']);
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':').ok_or(Unread::Malformed)?;
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_owned()));
    }
    let mut request = Request {
        method,
        path,
        headers,
        body: Vec::new(),
    };
    if request.header("transfer-encoding").is_some() {
        return Err(Unread::LengthRequired);
    }
    let length = match request.header("content-length") {
        Some(length) => length.parse::<usize>().map_err(|_| Unread::Malformed)?,
        None => 0,
    };
    if length > MAX_BODY_BYTES {
        return Err(Unread::TooLarge);
    }
    request.body = vec![0; length];
    reader
        .read_exact(&mut request.body)
        .map_err(|_| Unread::Malformed)?;
    Ok(request)
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

impl Handler {
    fn serve(&self, mut stream: TcpStream) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
        let _ = stream.set_write_timeout(Some(READ_TIMEOUT));
        let (status, body) = match read_request(&stream) {
            Ok(request) => self.answer(&request),
            Err(Unread::TooLarge) => (413, None),
            Err(Unread::LengthRequired) => (411, None),
            Err(Unread::Malformed) => (400, None),
        };
        let reason = match status {
            200 => "OK",
            202 => "Accepted",
            400 => "Bad Request",
            403 => "Forbidden",
            404 => "Not Found",
            405 => "Method Not Allowed",
            411 => "Length Required",
            413 => "Content Too Large",
            415 => "Unsupported Media Type",
            _ => "Error",
        };
        let body = body.map(|body| body.to_string()).unwrap_or_default();
        let mut head = format!("HTTP/1.1 {status} {reason}\r\nConnection: close\r\n");
        if status == 405 {
            head.push_str("Allow: POST\r\n");
        }
        if !body.is_empty() {
            head.push_str("Content-Type: application/json\r\n");
        }
        head.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body.as_bytes());
        let _ = stream.flush();
    }

    /// Whether `host` names this computer at this server's port.
    fn local_host(&self, host: &str) -> bool {
        ["127.0.0.1", "localhost", "[::1]"]
            .iter()
            .any(|name| host == format!("{name}:{}", self.port))
    }

    fn answer(&self, request: &Request) -> (u16, Option<Value>) {
        if !request
            .header("host")
            .is_some_and(|host| self.local_host(host))
        {
            return (403, None);
        }
        if let Some(origin) = request.header("origin") {
            let local = origin.strip_prefix("http://").is_some_and(|host| {
                self.local_host(host) || matches!(host, "127.0.0.1" | "localhost" | "[::1]")
            });
            if !local {
                return (403, None);
            }
        }
        if !same(&request.path, &self.path) {
            return (404, None);
        }
        if request.method != "POST" {
            return (405, None);
        }
        if !request
            .header("content-type")
            .is_some_and(|kind| kind.starts_with("application/json"))
        {
            return (415, None);
        }
        if request
            .header("mcp-protocol-version")
            .is_some_and(|version| !PROTOCOL_VERSIONS.contains(&version))
        {
            return (
                400,
                Some(error(
                    &Value::Null,
                    -32600,
                    "unsupported MCP protocol version",
                )),
            );
        }
        let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
            return (
                400,
                Some(error(&Value::Null, -32700, "the body is not JSON")),
            );
        };
        match self.tools.respond(&message) {
            Some(answer) => (200, Some(answer)),
            None => (202, None),
        }
    }
}

fn error(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn result(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// The tools, and the project they read.
pub struct Tools {
    project: ProjectSlot,
}

impl Tools {
    #[must_use]
    pub const fn new(project: ProjectSlot) -> Self {
        Self { project }
    }

    /// The answer to one JSON-RPC message; `None` for a notification, or a
    /// response to a request this server never makes.
    #[must_use]
    pub fn respond(&self, message: &Value) -> Option<Value> {
        let Some(message) = message.as_object() else {
            return Some(error(
                &Value::Null,
                -32600,
                "one JSON-RPC message, not a batch",
            ));
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response: this server sends no requests to answer.
            return id.map(|id| error(&id, -32600, "not a request"));
        };
        let id = id?;
        if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
            return Some(error(&id, -32600, "not JSON-RPC 2.0"));
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        Some(match method {
            "initialize" => {
                let asked = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let version = PROTOCOL_VERSIONS
                    .iter()
                    .find(|version| **version == asked)
                    .unwrap_or(&PROTOCOL_VERSIONS[0]);
                result(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {
                            "name": "kineto",
                            "title": "Kineto",
                            "version": env!("CARGO_PKG_VERSION"),
                        },
                        "instructions": "Kineto turns a story into a film, step by step. These tools read the project open in Kineto: where each step stands, the story, its documents and its shots. They change nothing.",
                    }),
                )
            }
            "ping" => result(&id, json!({})),
            "tools/list" => result(&id, json!({"tools": tool_list()})),
            "tools/call" => {
                let name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
                if !tool_list()
                    .iter()
                    .any(|tool| tool["name"].as_str() == Some(name))
                {
                    return Some(error(&id, -32602, &format!("no tool named {name:?}")));
                }
                let (text, is_error) = match self.call(name, &arguments) {
                    Ok(value) => (
                        serde_json::to_string_pretty(&value).unwrap_or_default(),
                        false,
                    ),
                    Err(why) => (why, true),
                };
                result(
                    &id,
                    json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
                )
            }
            _ => error(&id, -32601, &format!("no method {method:?}")),
        })
    }

    fn open(&self) -> Result<CanonicalProject, String> {
        let path = self
            .project
            .lock()
            .map_err(|_| "Kineto's project is not at hand".to_owned())?
            .clone()
            .ok_or_else(|| "No project is open in Kineto.".to_owned())?;
        CanonicalProject::open_read_only(&path)
            .map_err(|error| format!("The project cannot be read: {error}"))
    }

    fn call(&self, name: &str, arguments: &Value) -> Result<Value, String> {
        let project = self.open()?;
        match name {
            "get_project" => project_overview(&project),
            "get_story" => story(&project),
            "get_document" => {
                let kind = arguments
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                document(&project, kind)
            }
            "get_shot" => {
                let text = |field: &str| {
                    arguments
                        .get(field)
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned()
                };
                shot(&project, &text("scene"), &text("shot"))
            }
            _ => Err(format!("no tool named {name:?}")),
        }
    }
}

/// The tools, as `tools/list` gives them.
fn tool_list() -> Vec<Value> {
    let read_only = json!({"readOnlyHint": true, "openWorldHint": false});
    vec![
        json!({
            "name": "get_project",
            "title": "Where the project stands",
            "description": "The project open in Kineto: its title, each step of making the film and where it stands (waiting, ready, awaiting_approval, done, skipped), and each scene's shots with their keyframe and video stage (missing, made, locked, stale).",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": read_only,
        }),
        json!({
            "name": "get_story",
            "title": "The story",
            "description": "The story's source text as the director gave it, and Kineto's analysis of it (characters, locations, events, candidate scenes) when there is one.",
            "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            "annotations": read_only,
        }),
        json!({
            "name": "get_document",
            "title": "A document",
            "description": "The current revision of one of the project's documents: the locked one, or the latest candidate. screenplay: the scenes, action and dialogue. bible: characters, locations and props as they must look. shot_plan: each scene's shots. preferences: the director's standing preferences.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "kind": {"type": "string", "enum": ["screenplay", "bible", "shot_plan", "preferences"]},
                },
                "required": ["kind"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        }),
        json!({
            "name": "get_shot",
            "title": "A shot",
            "description": "One shot: what it is meant to show, its keyframe candidates and which is chosen or locked, and its video, with the image and clip files as paths on this computer. Scene and shot IDs are those get_project lists.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scene": {"type": "string"},
                    "shot": {"type": "string"},
                },
                "required": ["scene", "shot"],
                "additionalProperties": false,
            },
            "annotations": read_only,
        }),
    ]
}

const fn step_state(state: StepState) -> &'static str {
    match state {
        StepState::Waiting => "waiting",
        StepState::Ready => "ready",
        StepState::AwaitingApproval => "awaiting_approval",
        StepState::Done => "done",
        StepState::Skipped => "skipped",
    }
}

const fn stage(stage: Stage) -> &'static str {
    match stage {
        Stage::Missing => "missing",
        Stage::Made => "made",
        Stage::Locked => "locked",
        Stage::Stale => "stale",
    }
}

const fn revision_status(status: RevisionStatus) -> &'static str {
    match status {
        RevisionStatus::Candidate => "candidate",
        RevisionStatus::Locked => "locked",
        RevisionStatus::Superseded => "superseded",
    }
}

fn project_overview(project: &CanonicalProject) -> Result<Value, String> {
    let steps = workflow::status(project).map_err(|error| error.to_string())?;
    let scenes = workflow::scenes(project).map_err(|error| error.to_string())?;
    Ok(json!({
        "title": project.manifest().title,
        "steps": steps.iter().map(|step| json!({
            "id": step.id,
            "description": step.description,
            "state": step_state(step.state),
            "needs_approval": step.approval,
            "waiting_on": step.waiting_on,
            "changed_since": step.stale,
        })).collect::<Vec<_>>(),
        "scenes": scenes.iter().map(|scene| json!({
            "scene": scene.scene_id,
            "complete": scene.complete(),
            "shots": scene.shots.iter().map(|shot| json!({
                "shot": shot.shot_id,
                "keyframe": stage(shot.keyframe),
                "video": stage(shot.video),
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

/// The current revision of `kind`, as stored.
fn current(project: &CanonicalProject, kind: DocumentKind) -> Result<Value, String> {
    let Some(record) = revisions::current(project, kind).map_err(|error| error.to_string())? else {
        return Ok(Value::Null);
    };
    let path = ProjectRelativePath::new(&record.path).map_err(|error| error.to_string())?;
    let bytes = project
        .root()
        .read(&path)
        .map_err(|error| error.to_string())?;
    let document: Value =
        serde_json::from_slice(&bytes).map_err(|_| "a document is not JSON".to_owned())?;
    Ok(json!({
        "revision": record.revision,
        "status": revision_status(record.status),
        "document": document,
    }))
}

fn story(project: &CanonicalProject) -> Result<Value, String> {
    let revision = source::current(project).map_err(|error| error.to_string())?;
    let text = source::read(project, &revision).map_err(|error| error.to_string())?;
    let truncated = text.chars().count() > MAX_SOURCE_CHARS;
    Ok(json!({
        "source": {
            "revision": revision.revision,
            "text": text.chars().take(MAX_SOURCE_CHARS).collect::<String>(),
            "truncated": truncated,
        },
        "analysis": current(project, DocumentKind::StoryAnalysis)?,
    }))
}

fn document(project: &CanonicalProject, kind: &str) -> Result<Value, String> {
    let document = match kind {
        "screenplay" => DocumentKind::Screenplay,
        "bible" => DocumentKind::Bible,
        "shot_plan" => DocumentKind::ShotPlan,
        "preferences" => DocumentKind::Preferences,
        other => {
            return Err(format!(
                "{other:?} is not a document: screenplay, bible, shot_plan or preferences"
            ));
        }
    };
    let current = current(project, document)?;
    Ok(if current.is_null() {
        json!({"kind": kind, "revision": null, "note": "There is none yet."})
    } else {
        let mut current = current;
        current["kind"] = json!(kind);
        current
    })
}

fn shot(project: &CanonicalProject, scene: &str, shot: &str) -> Result<Value, String> {
    let missing =
        || format!("There is no shot {shot:?} in scene {scene:?}: get_project lists them.");
    let key = ShotKey::new(scene, shot).map_err(|_| missing())?;
    if !shots_in_play(project)
        .map_err(|error| error.to_string())?
        .contains(&key)
    {
        return Err(missing());
    }
    let workflow = ShotWorkflow::load(project, &key).map_err(|error| error.to_string())?;
    let snapshot = workflow.snapshot().map_err(|error| error.to_string())?;
    let absolute =
        |path: &ProjectRelativePath| project.root().resolve(path).to_string_lossy().into_owned();
    let candidates = workflow
        .candidate_media()
        .map_err(|error| error.to_string())?
        .iter()
        .map(|path| path.as_ref().map(absolute))
        .collect::<Vec<_>>();
    let video = workflow.video_media().map_err(|error| error.to_string())?;
    Ok(json!({
        "scene": scene,
        "shot": shot,
        "intent": workflow.intent_record().map_err(|error| error.to_string())?,
        "keyframes": {
            "generated": snapshot.generated,
            "chosen": snapshot.selected_index,
            "locked": snapshot.locked,
            "stale": snapshot.stale,
            "candidates": candidates,
        },
        "video": {
            "state": match snapshot.video {
                VideoState::None => "none",
                VideoState::Ready => "made",
                VideoState::Locked => "locked",
                VideoState::Stale => "stale",
            },
            "file": video.as_ref().map(absolute),
        },
    }))
}

#[cfg(test)]
mod tests;
