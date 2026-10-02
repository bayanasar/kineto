//! Agent CLIs as `llm.complete` backends, over the Agent Client Protocol.
//!
//! An agent CLI such as Kimi Code signs in on its own and runs its own model;
//! kineto drives the CLI itself and never touches its credentials. The CLI is
//! started as `<executable> acp`, speaks JSON-RPC on stdin and stdout, and
//! gets the conversation as one text prompt. Nothing about the call may act
//! on the machine:
//!
//! - it runs under a data root kineto prepares ([`prepare_kimi_code_home`]):
//!   every built-in tool disabled and denied, no hooks, no telemetry, so the
//!   user's own agent configuration never applies;
//! - kineto answers every permission request with a rejection and every file
//!   or terminal request with an error;
//! - the working directory is a fresh empty directory, removed afterwards,
//!   and the session is deleted from the agent's history.
//!
//! Source text is untrusted. These layers are what keep an instruction hidden
//! in it from running a command or reading a file.

use std::{
    error::Error,
    ffi::OsString,
    fmt, fs,
    io::{self, BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use kineto_jobs::ErrorClass;
use kineto_providers::LlmCapabilities;
use serde_json::{Value, json};

use crate::{Billing, LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse, Role};

/// The provider key Kimi Code jobs are recorded under.
pub const KIMI_CODE_PROVIDER: &str = "kimi-code";
/// How long one completion may take before the agent is stopped.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a waiting call checks whether it was cancelled.
const CANCEL_CHECK: Duration = Duration::from_millis(100);
/// Every Kimi Code built-in tool, as its tool reference names them.
const KIMI_CODE_TOOLS: [&str; 24] = [
    "Read",
    "Write",
    "Edit",
    "Grep",
    "Glob",
    "ReadMediaFile",
    "Bash",
    "WebSearch",
    "FetchURL",
    "EnterPlanMode",
    "ExitPlanMode",
    "TodoList",
    "Agent",
    "AgentSwarm",
    "AskUserQuestion",
    "NotifyUser",
    "Skill",
    "TaskList",
    "TaskOutput",
    "TaskStop",
    "WaitFor",
    "CronCreate",
    "CronList",
    "CronDelete",
];

/// Marks a data root as kineto's own. Kineto writes it when it creates the
/// root and never adopts a root without it, such as the user's own
/// `~/.kimi-code`.
const SANDBOX_MARKER: &str = "kineto-sandbox";

/// Make `config` a sandbox: no tool enabled, every known built-in and MCP
/// tool also disabled, every
/// built-in tool denied, manual permissions, no hooks, no extra agents or
/// skills, no telemetry. Everything else, such as the provider and model
/// settings sign-in writes, is kept. Returns whether anything changed.
fn enforce_sandbox(config: &mut toml::Table) -> bool {
    use toml::Value;
    let before = config.clone();
    config.insert("telemetry".to_owned(), Value::Boolean(false));
    config.insert(
        "default_permission_mode".to_owned(),
        Value::String("manual".to_owned()),
    );
    config.insert(
        "merge_all_available_skills".to_owned(),
        Value::Boolean(false),
    );
    config.remove("hooks");
    config.remove("extra_agent_dirs");

    let disabled = KIMI_CODE_TOOLS
        .iter()
        .copied()
        .chain(["mcp__*"])
        .map(|tool| Value::String(tool.to_owned()))
        .collect();
    let mut tools = toml::Table::new();
    // An allowlist naming no tool leaves none: Kimi Code documents that a
    // bare `*` matches nothing, so `enabled = ["*"]` disables every tool,
    // including ones a later release adds. The denylist repeats the known
    // tools in case that ever changes.
    tools.insert(
        "enabled".to_owned(),
        Value::Array(vec![Value::String("*".to_owned())]),
    );
    tools.insert("disabled".to_owned(), Value::Array(disabled));
    config.insert("tools".to_owned(), Value::Table(tools));

    let rules = KIMI_CODE_TOOLS
        .iter()
        .map(|tool| {
            let mut rule = toml::Table::new();
            rule.insert("decision".to_owned(), Value::String("deny".to_owned()));
            rule.insert("pattern".to_owned(), Value::String((*tool).to_owned()));
            rule.insert(
                "reason".to_owned(),
                Value::String("kineto runs Kimi Code without tools".to_owned()),
            );
            Value::Table(rule)
        })
        .collect();
    let mut permission = toml::Table::new();
    permission.insert("rules".to_owned(), Value::Array(rules));
    config.insert("permission".to_owned(), Value::Table(permission));

    *config != before
}

/// Make `home` a Kimi Code data root kineto can use, and keep it one.
///
/// A new root is created with kineto's marker and sandbox configuration.
/// An existing root must carry the marker; its configuration is checked on
/// every call and the sandbox restored if anything (sign-in rewrites the
/// file) dropped part of it. Signing in (`KIMI_CODE_HOME=<home> kimi login`)
/// is the user's step.
pub fn prepare_kimi_code_home(home: &Path) -> Result<(), AgentError> {
    let setup = |error: io::Error| AgentError::Setup(error.kind());
    match fs::symlink_metadata(home) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(home).map_err(setup)?;
            fs::write(home.join(SANDBOX_MARKER), "kineto agent sandbox v1\n").map_err(setup)?;
        }
        Err(error) => return Err(setup(error)),
        Ok(metadata) if !metadata.is_dir() => {
            return Err(AgentError::Setup(io::ErrorKind::NotADirectory));
        }
        Ok(_) if !home.join(SANDBOX_MARKER).is_file() => return Err(AgentError::ForeignConfig),
        Ok(_) => {}
    }
    // MCP servers would bring tools of their own.
    match fs::remove_file(home.join("mcp.json")) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(setup(error)),
        _ => {}
    }
    let path = home.join("config.toml");
    let mut config = match fs::read_to_string(&path) {
        Ok(text) => text
            .parse::<toml::Table>()
            .map_err(|_| AgentError::Setup(io::ErrorKind::InvalidData))?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => toml::Table::new(),
        Err(error) => return Err(setup(error)),
    };
    if enforce_sandbox(&mut config) || !path.exists() {
        let text =
            toml::to_string(&config).map_err(|_| AgentError::Setup(io::ErrorKind::InvalidData))?;
        let staged = home.join("config.toml.kineto-tmp");
        fs::write(&staged, text).map_err(setup)?;
        fs::rename(&staged, &path).map_err(setup)?;
    }
    Ok(())
}

/// How to start an agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    /// Variables set for the agent on top of kineto's environment.
    pub env: Vec<(OsString, OsString)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentError {
    /// The agent's data root could not be prepared.
    Setup(io::ErrorKind),
    /// The data root was not created by kineto.
    ForeignConfig,
    /// The agent could not be started.
    Spawn(io::ErrorKind),
    /// The agent is not signed in under kineto's data root.
    NotSignedIn,
    /// The agent took longer than the call allows and was stopped.
    Timeout,
    /// The caller cancelled the call, for example by closing the project;
    /// the agent was stopped.
    Cancelled,
    /// The agent stopped before answering.
    Exited,
    /// The agent answered a request with an error.
    Agent { code: i64, message: String },
    /// The agent spoke something other than the protocol.
    Protocol(&'static str),
    /// The answer stopped at the agent's token limit.
    Truncated,
    /// The agent declined to answer.
    Refused,
    /// The agent finished without saying anything.
    Empty,
}

impl fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setup(kind) => {
                write!(formatter, "agent data root could not be prepared: {kind:?}")
            }
            Self::ForeignConfig => formatter.write_str(
                "the agent data root was not created by kineto; kineto only runs agents in a \
                 data root of its own",
            ),
            Self::Spawn(kind) => write!(formatter, "agent could not be started: {kind:?}"),
            Self::NotSignedIn => formatter.write_str(
                "the agent is not signed in under kineto's data root; sign in once with \
                 KIMI_CODE_HOME set to it",
            ),
            Self::Timeout => formatter.write_str("the agent took too long and was stopped"),
            Self::Cancelled => formatter.write_str("the call was cancelled and the agent stopped"),
            Self::Exited => formatter.write_str("the agent stopped before answering"),
            Self::Agent { code, message } => {
                write!(formatter, "the agent reported an error ({code}): {message}")
            }
            Self::Protocol(what) => write!(formatter, "agent protocol error: {what}"),
            Self::Truncated => formatter.write_str("the answer stopped at the agent's token limit"),
            Self::Refused => formatter.write_str("the agent declined to answer"),
            Self::Empty => formatter.write_str("the agent finished without an answer"),
        }
    }
}

impl Error for AgentError {}

impl AgentError {
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Timeout | Self::Cancelled | Self::Exited | Self::Agent { .. } => {
                ErrorClass::Retryable
            }
            Self::Setup(_)
            | Self::ForeignConfig
            | Self::Spawn(_)
            | Self::NotSignedIn
            | Self::Protocol(_)
            | Self::Truncated
            | Self::Refused
            | Self::Empty => ErrorClass::Terminal,
        }
    }
}

/// An agent CLI driven over ACP.
#[derive(Debug, Clone)]
pub struct AcpAgent {
    command: AgentCommand,
    provider: String,
    timeout: Duration,
    workspace_root: PathBuf,
    home: Option<PathBuf>,
    cancel: Option<Arc<AtomicBool>>,
}

impl AcpAgent {
    /// Kimi Code at `executable`, under the kineto data root `home`.
    #[must_use]
    pub fn kimi_code(executable: PathBuf, home: PathBuf) -> Self {
        let mut agent = Self::with_command(
            AgentCommand {
                executable,
                args: vec!["acp".into()],
                env: vec![
                    ("KIMI_CODE_HOME".into(), home.clone().into_os_string()),
                    ("KIMI_DISABLE_TELEMETRY".into(), "1".into()),
                ],
            },
            KIMI_CODE_PROVIDER,
        );
        agent.home = Some(home);
        agent
    }

    /// Any ACP agent started by `command`. Its data root, if it has one, is
    /// the caller's to sandbox.
    #[must_use]
    pub fn with_command(command: AgentCommand, provider: &str) -> Self {
        Self {
            command,
            provider: provider.to_owned(),
            timeout: DEFAULT_TIMEOUT,
            workspace_root: std::env::temp_dir(),
            home: None,
            cancel: None,
        }
    }

    /// Stop any call in progress, and refuse new ones, once `cancel` is set.
    #[must_use]
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }

    #[must_use]
    pub const fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Where each call's empty working directory is created.
    #[must_use]
    pub fn with_workspace_root(mut self, root: PathBuf) -> Self {
        self.workspace_root = root;
        self
    }
}

/// The conversation as one prompt: agents take a single turn of text.
fn prompt_text(request: &LlmRequest) -> String {
    let mut text = String::new();
    for message in &request.messages {
        let heading = match message.role {
            Role::System => "Instructions",
            Role::User => "Request",
            Role::Assistant => "Your earlier reply",
        };
        text.push_str(&format!("## {heading}\n\n{}\n\n", message.content));
    }
    if let Some(schema) = &request.schema {
        text.push_str(&format!(
            "## Answer format\n\nAnswer with a single JSON value that satisfies the JSON \
             Schema `{}` below. No prose, no code fence. Do not use any tools.\n\n{}\n",
            schema.name, schema.schema_json
        ));
    }
    text
}

/// One running agent process and its JSON-RPC channel.
struct Session {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<io::Result<String>>,
    deadline: Instant,
    cancel: Option<Arc<AtomicBool>>,
    next_id: i64,
    answer: String,
}

impl Session {
    fn send(&mut self, message: &Value) -> Result<(), AgentError> {
        let mut line = serde_json::to_vec(message).map_err(|_| AgentError::Protocol("encode"))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .and_then(|()| self.stdin.flush())
            .map_err(|_| AgentError::Exited)
    }

    /// Send a request and serve the agent until its answer to it arrives.
    fn call(&mut self, method: &str, params: Value) -> Result<Value, AgentError> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        loop {
            let message = self.receive()?;
            if message.get("method").is_some() {
                self.serve(&message)?;
                continue;
            }
            if message.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                let code = error
                    .get("code")
                    .and_then(Value::as_i64)
                    .unwrap_or_default();
                let text = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if code == -32000 || text.to_ascii_lowercase().contains("auth") {
                    return Err(AgentError::NotSignedIn);
                }
                return Err(AgentError::Agent {
                    code,
                    message: text
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(200)
                        .collect(),
                });
            }
            return Ok(message.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    fn receive(&mut self) -> Result<Value, AgentError> {
        loop {
            if is_cancelled(self.cancel.as_ref()) {
                return Err(AgentError::Cancelled);
            }
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .ok_or(AgentError::Timeout)?;
            match self.lines.recv_timeout(remaining.min(CANCEL_CHECK)) {
                Ok(Ok(line)) => {
                    return serde_json::from_str(&line)
                        .map_err(|_| AgentError::Protocol("not JSON-RPC"));
                }
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(AgentError::Exited);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    /// Answer what the agent asks of kineto: never yes.
    fn serve(&mut self, message: &Value) -> Result<(), AgentError> {
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = message.get("id").cloned() else {
            // A notification.
            if method == "session/update" {
                let update = &params["update"];
                if update["sessionUpdate"] == "agent_message_chunk"
                    && update["content"]["type"] == "text"
                    && let Some(text) = update["content"]["text"].as_str()
                {
                    self.answer.push_str(text);
                }
            }
            return Ok(());
        };
        let reply = if method == "session/request_permission" {
            let reject = params["options"].as_array().and_then(|options| {
                options.iter().find(|option| {
                    option["kind"]
                        .as_str()
                        .is_some_and(|kind| kind.starts_with("reject"))
                })
            });
            let outcome = match reject.and_then(|option| option["optionId"].as_str()) {
                Some(option) => json!({"outcome": "selected", "optionId": option}),
                None => json!({"outcome": "cancelled"}),
            };
            json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": outcome}})
        } else {
            json!({"jsonrpc": "2.0", "id": id, "error": {
                "code": -32601,
                "message": "kineto gives agents no access to files, terminals or questions",
            }})
        };
        self.send(&reply)
    }

    /// Let the agent exit; stop it if it does not, at once if it already
    /// overran its deadline.
    fn close(mut self, graceful: bool) {
        drop(self.stdin);
        let deadline = Instant::now()
            + if graceful {
                SHUTDOWN_TIMEOUT
            } else {
                Duration::ZERO
            };
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn is_cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel.is_some_and(|cancel| cancel.load(Ordering::SeqCst))
}

static NEXT_WORKSPACE: AtomicU64 = AtomicU64::new(0);

/// The agent deletes the session but keeps an empty directory named after the
/// working directory; remove it so kineto's data root does not grow a
/// directory per call.
fn remove_empty_session_dirs(home: &Path, workspace: &Workspace) {
    let Some(name) = workspace.0.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let Ok(entries) = fs::read_dir(home.join("sessions")) else {
        return;
    };
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .is_some_and(|entry| entry.starts_with(&format!("wd_{name}")))
        {
            // Only an empty directory goes; anything left in it is kept.
            let _ = fs::remove_dir(entry.path());
        }
    }
}

/// An empty directory for one call, removed when dropped.
struct Workspace(PathBuf);

impl Workspace {
    fn create(root: &Path) -> Result<Self, AgentError> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = root.join(format!(
            "kineto-agent-{}-{nanos}-{}",
            std::process::id(),
            NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).map_err(|error| AgentError::Setup(error.kind()))?;
        Ok(Self(path))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl AcpAgent {
    fn start(&self, workspace: &Workspace) -> Result<Session, AgentError> {
        let mut child = Command::new(&self.command.executable)
            .args(&self.command.args)
            .envs(self.command.env.iter().map(|(key, value)| (key, value)))
            .current_dir(&workspace.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| AgentError::Spawn(error.kind()))?;
        let stdin = child
            .stdin
            .take()
            .ok_or(AgentError::Spawn(io::ErrorKind::Other))?;
        let stdout = child
            .stdout
            .take()
            .ok_or(AgentError::Spawn(io::ErrorKind::Other))?;
        let (sender, lines) = mpsc::channel();
        thread::Builder::new()
            .name("kineto-agent-reader".to_owned())
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let stop = line.is_err();
                    if sender.send(line).is_err() || stop {
                        break;
                    }
                }
            })
            .map_err(|error| AgentError::Spawn(error.kind()))?;
        Ok(Session {
            child,
            stdin,
            lines,
            deadline: Instant::now() + self.timeout,
            cancel: self.cancel.clone(),
            next_id: 0,
            answer: String::new(),
        })
    }

    fn converse(
        session: &mut Session,
        workspace: &Workspace,
        prompt: String,
    ) -> Result<(), AgentError> {
        session.call(
            "initialize",
            json!({
                "protocolVersion": 1,
                // File access is routed to kineto, which refuses it.
                "clientCapabilities": {
                    "fs": {"readTextFile": true, "writeTextFile": true},
                    "terminal": false,
                },
            }),
        )?;
        let created = session.call("session/new", json!({"cwd": workspace.0, "mcpServers": []}))?;
        let session_id = created["sessionId"]
            .as_str()
            .ok_or(AgentError::Protocol("no session ID"))?
            .to_owned();
        let prompted = session.call(
            "session/prompt",
            json!({"sessionId": session_id, "prompt": [{"type": "text", "text": prompt}]}),
        );
        // Leave nothing in the agent's history, whatever the answer was.
        let _ = session.call("session/delete", json!({"sessionId": session_id}));
        match prompted?["stopReason"].as_str() {
            Some("end_turn") => Ok(()),
            Some("max_tokens" | "max_turn_requests") => Err(AgentError::Truncated),
            Some("refusal") => Err(AgentError::Refused),
            Some("cancelled") => Err(AgentError::Exited),
            _ => Err(AgentError::Protocol("unknown stop reason")),
        }
    }
}

impl LlmBackend for AcpAgent {
    type Error = AgentError;

    fn info(&self) -> LlmBackendInfo {
        LlmBackendInfo {
            provider: self.provider.clone(),
            // The agent uses the model its own configuration selects.
            model: "agent-default".to_owned(),
            capabilities: LlmCapabilities {
                context_window_tokens: None,
                tools: false,
                structured_output: false,
                vision_input: false,
            },
            billing: Billing::Subscription,
        }
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, AgentError> {
        if is_cancelled(self.cancel.as_ref()) {
            return Err(AgentError::Cancelled);
        }
        if let Some(home) = &self.home {
            prepare_kimi_code_home(home)?;
        }
        let workspace = Workspace::create(&self.workspace_root)?;
        let mut session = self.start(&workspace)?;
        let result = Self::converse(&mut session, &workspace, prompt_text(request));
        let answer = std::mem::take(&mut session.answer);
        session.close(!matches!(
            result,
            Err(AgentError::Timeout | AgentError::Cancelled)
        ));
        if let Some(home) = &self.home {
            remove_empty_session_dirs(home, &workspace);
        }
        result?;
        if answer.trim().is_empty() {
            return Err(AgentError::Empty);
        }
        Ok(LlmResponse {
            text: answer,
            usage: None,
            model: "agent-default".to_owned(),
        })
    }

    fn classify(&self, error: &AgentError) -> ErrorClass {
        error.class()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, OutputSchema};

    fn sandbox_root() -> PathBuf {
        std::env::temp_dir().join(format!(
            "kineto-acp-home-test-{}-{}",
            std::process::id(),
            NEXT_WORKSPACE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn assert_sandboxed(config: &toml::Table) {
        assert_eq!(config["telemetry"].as_bool(), Some(false));
        assert_eq!(config["default_permission_mode"].as_str(), Some("manual"));
        assert_eq!(
            config["tools"]["enabled"].as_array().unwrap(),
            &vec![toml::Value::String("*".to_owned())]
        );
        assert!(!config.contains_key("hooks"));
        let disabled = config["tools"]["disabled"].as_array().unwrap();
        let rules = config["permission"]["rules"].as_array().unwrap();
        for tool in KIMI_CODE_TOOLS {
            assert!(
                disabled.iter().any(|entry| entry.as_str() == Some(tool)),
                "{tool}"
            );
            assert!(
                rules
                    .iter()
                    .any(|rule| rule["pattern"].as_str() == Some(tool)
                        && rule["decision"].as_str() == Some("deny")),
                "{tool}"
            );
        }
        assert!(
            rules
                .iter()
                .all(|rule| rule["decision"].as_str() == Some("deny"))
        );
        assert!(
            disabled
                .iter()
                .any(|entry| entry.as_str() == Some("mcp__*"))
        );
    }

    #[test]
    fn a_new_data_root_is_sandboxed_and_marked() {
        let root = sandbox_root();
        prepare_kimi_code_home(&root).unwrap();
        assert!(root.join(SANDBOX_MARKER).is_file());
        let config: toml::Table = fs::read_to_string(root.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_sandboxed(&config);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_rewritten_configuration_is_sandboxed_again_and_keeps_sign_in() {
        let root = sandbox_root();
        prepare_kimi_code_home(&root).unwrap();
        // What sign-in or a person might leave behind.
        fs::write(
            root.join("config.toml"),
            "telemetry = true\ndefault_permission_mode = \"auto\"\n\
             [tools]\ndisabled = [\"Read\"]\n\
             [[permission.rules]]\ndecision = \"allow\"\npattern = \"Bash\"\n\
             [[hooks]]\nevent = \"Stop\"\ncommand = \"echo\"\n\
             [providers.\"managed:kimi-code\"]\ntype = \"kimi\"\n",
        )
        .unwrap();
        fs::write(root.join("mcp.json"), "{}").unwrap();
        prepare_kimi_code_home(&root).unwrap();
        let config: toml::Table = fs::read_to_string(root.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        assert_sandboxed(&config);
        assert_eq!(
            config["providers"]["managed:kimi-code"]["type"].as_str(),
            Some("kimi"),
            "sign-in settings are kept"
        );
        assert!(!root.join("mcp.json").exists());
        // Unchanged when already sandboxed.
        let written = fs::read(root.join("config.toml")).unwrap();
        prepare_kimi_code_home(&root).unwrap();
        assert_eq!(fs::read(root.join("config.toml")).unwrap(), written);
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_data_root_kineto_did_not_create_is_refused() {
        let root = sandbox_root();
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("config.toml"), "[[hooks]]\nevent = \"Stop\"\n").unwrap();
        assert_eq!(
            prepare_kimi_code_home(&root),
            Err(AgentError::ForeignConfig)
        );
        assert_eq!(
            fs::read_to_string(root.join("config.toml")).unwrap(),
            "[[hooks]]\nevent = \"Stop\"\n",
            "a foreign configuration is never touched"
        );
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_prompt_carries_every_message_and_the_schema() {
        let text = prompt_text(&LlmRequest {
            messages: vec![
                Message::system("You analyse stories."),
                Message::user("Source text."),
            ],
            schema: Some(OutputSchema {
                name: "analysis".to_owned(),
                schema_json: r#"{"type":"object"}"#.to_owned(),
            }),
            max_output_tokens: None,
        });
        assert!(text.contains("## Instructions\n\nYou analyse stories."));
        assert!(text.contains("## Request\n\nSource text."));
        assert!(text.contains("`analysis`"));
        assert!(text.ends_with("{\"type\":\"object\"}\n"));
    }
}
