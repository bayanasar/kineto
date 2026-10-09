//! The director's own agent CLIs as `llm.complete` backends: Claude Code,
//! Codex, Kimi Code, agy (Antigravity) and Mini-Agent.
//!
//! Kineto finds the CLI the director already installed and signed in to, and
//! drives it; it never signs in for them and never reads a credential the
//! CLI keeps. Each call runs in a fresh empty working directory, removed
//! afterwards, and gets the conversation as one prompt. Source text is
//! untrusted, so every agent is started without tools, as each CLI allows:
//!
//! - Claude Code: `-p` with no tools, no MCP servers, none of the user's
//!   settings (so none of their hooks or plugins) and no saved session;
//! - Codex: `exec` ignoring the user's configuration and rules, read-only,
//!   with every feature that is a tool switched off, and no saved session;
//! - Kimi Code: `-p` with an agent file of its own that allows no tools;
//! - agy: an agent of its own, defined in the working directory, whose only
//!   tool is the one that hands back the answer;
//! - Mini-Agent: its ACP server, started with every tool switched off.
//!
//! What a CLI does not let kineto turn off is said where it is chosen: Kimi
//! Code and agy still run the user's own hooks.

use std::{
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, atomic::AtomicBool},
    time::{Duration, Instant},
};

use kineto_jobs::ErrorClass;
use kineto_providers::LlmCapabilities;
use serde_json::{Value, json};

use crate::{
    Billing, LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse,
    acp::{AcpAgent, AgentCommand, AgentError, DEFAULT_TIMEOUT, prompt_text},
    process::{Finished, Workspace, is_cancelled, run_to_end, search_path},
};

/// An agent CLI kineto knows how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AgentKind {
    ClaudeCode,
    Codex,
    KimiCode,
    Agy,
    MiniAgent,
}

impl AgentKind {
    pub const ALL: [Self; 5] = [
        Self::ClaudeCode,
        Self::Codex,
        Self::KimiCode,
        Self::Agy,
        Self::MiniAgent,
    ];

    /// How a selection names it, and the provider key its jobs are recorded
    /// under.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::KimiCode => "kimi-code",
            Self::Agy => "agy",
            Self::MiniAgent => "mini-agent",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// How the director knows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::KimiCode => "Kimi Code",
            Self::Agy => "Antigravity (agy)",
            Self::MiniAgent => "Mini-Agent",
        }
    }

    /// The command it is installed as.
    const fn command(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
            Self::KimiCode => "kimi",
            Self::Agy => "agy",
            Self::MiniAgent => "mini-agent-acp",
        }
    }

    /// Whether it can make images.
    #[must_use]
    pub const fn makes_images(self) -> bool {
        matches!(self, Self::Agy)
    }

    /// How its calls are paid for. Mini-Agent spends the director's own
    /// MiniMax key; the others a subscription.
    #[must_use]
    pub const fn billing(self) -> Billing {
        match self {
            Self::MiniAgent => Billing::Unpriced,
            _ => Billing::Subscription,
        }
    }
}

/// The user's home directory as `env` gives it.
fn home(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let set = |name: &str| env(name).filter(|value| !value.is_empty());
    if cfg!(windows) {
        set("USERPROFILE").or_else(|| set("HOME"))
    } else {
        set("HOME")
    }
    .map(PathBuf::from)
}

/// Where agent CLIs are installed besides the search path: a desktop app
/// started from the Dock, Finder or Start menu does not get the user's shell
/// path, and installers put CLIs in these places.
fn usual_directories(env: &impl Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = home(env) {
        for relative in [
            ".local/bin",
            ".claude/local",
            ".npm-global/bin",
            ".bun/bin",
            ".volta/bin",
            ".cargo/bin",
            ".kimi-code/bin",
        ] {
            directories.push(home.join(relative));
        }
    }
    if cfg!(windows) {
        for (variable, relative) in [("APPDATA", "npm"), ("LOCALAPPDATA", "Programs")] {
            if let Some(base) = env(variable).filter(|value| !value.is_empty()) {
                directories.push(Path::new(&base).join(relative));
            }
        }
    } else {
        for directory in [
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/snap/bin",
        ] {
            directories.push(PathBuf::from(directory));
        }
    }
    directories
}

fn executable_names(command: &str) -> Vec<String> {
    if cfg!(windows) {
        ["exe", "cmd", "bat"]
            .iter()
            .map(|extension| format!("{command}.{extension}"))
            .collect()
    } else {
        vec![command.to_owned()]
    }
}

/// Where `command` is installed: on the search path `env` gives, then in the
/// usual places.
fn find_command(command: &str, env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let names = executable_names(command);
    let path = env("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(usual_directories(env))
        .flat_map(|directory| names.iter().map(move |name| directory.join(name)))
        .find(|candidate| candidate.is_file())
}

/// The Python that runs Mini-Agent: the interpreter its `mini-agent-acp`
/// launcher names, or the one in uv's tool directory.
fn mini_agent_python(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    if let Some(launcher) = find_command(AgentKind::MiniAgent.command(), env)
        && let Some(python) = interpreter_of(&launcher)
    {
        return Some(python);
    }
    let set = |name: &str| {
        env(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    };
    let tool_directories = [
        set("UV_TOOL_DIR"),
        set("XDG_DATA_HOME").map(|data| data.join("uv/tools")),
        home(env).map(|home| home.join(".local/share/uv/tools")),
        set("APPDATA").map(|data| data.join("uv/tools")),
    ];
    tool_directories
        .into_iter()
        .flatten()
        .flat_map(|tools| {
            [
                tools.join("mini-agent/bin/python"),
                tools.join("mini-agent/Scripts/python.exe"),
            ]
        })
        .find(|candidate| candidate.is_file())
}

/// The interpreter a script's `#!` line names, when it is an absolute path
/// to a file.
fn interpreter_of(script: &Path) -> Option<PathBuf> {
    let mut head = [0_u8; 512];
    let read = fs::File::open(script).ok()?.read(&mut head).ok()?;
    let first = head[..read].split(|byte| *byte == b'\n').next()?;
    let line = std::str::from_utf8(first.strip_prefix(b"#!")?).ok()?.trim();
    let interpreter = PathBuf::from(line.split_whitespace().next()?);
    (interpreter.is_absolute() && interpreter.is_file()).then_some(interpreter)
}

/// Where `kind` is installed, as `env` describes the user's environment;
/// `None` when it is not. For Mini-Agent this is the Python that runs it.
pub fn locate(kind: AgentKind, env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    match kind {
        AgentKind::MiniAgent => mini_agent_python(env),
        _ => find_command(kind.command(), env),
    }
}

/// Every feature Codex offers that is a tool, switched off for kineto's
/// calls. Measured on Codex 0.160.0: without this, a read-only sandbox still
/// lets it run `cat` on any file.
const CODEX_TOOL_FEATURES: [&str; 16] = [
    "shell_tool",
    "unified_exec",
    "apps",
    "browser_use",
    "browser_use_external",
    "computer_use",
    "multi_agent",
    "plugins",
    "hooks",
    "view_image",
    "image_generation",
    "sleep_tool",
    "skill_search",
    "tool_suggest",
    "goals",
    "memories",
];

/// Kimi Code's agent for kineto's calls: no tools at all, MCP tools
/// included, and no subagents.
const KIMI_WRITER: &str = "---
name: kineto-writer
description: Answers one request for Kineto from the text it is given, without tools.
tools: []
disallowedTools: \"*\"
subagents: []
---
Answer the request from the text it gives you. You have no tools: never run commands, read \
files, browse or ask questions.
";

/// agy's agent for kineto's calls, found in the working directory: its own
/// prompt instead of the default one, no MCP servers, no rules, and only the
/// tool that hands back the answer.
const AGY_WRITER: &str = "---
name: kineto-writer
description: Answers one request for Kineto from the text it is given, without tools.
mainAgent: true
tools: [finish]
excludeDefaultComponents: true
inheritMcp: false
rules: []
---
# kineto-writer

Answer the request from the text it gives you, then finish. You have no other tools: never run \
commands, read files, browse or ask questions.
";

/// What agy is asked to answer with when kineto wants text.
const AGY_TEXT_SCHEMA: &str =
    r#"{"type":"object","properties":{"answer":{"type":"string"}},"required":["answer"]}"#;

/// The agy agent that makes one image and nothing else.
const AGY_ARTIST: &str = "---
name: kineto-artist
description: Makes one image for Kineto from the description it is given.
mainAgent: true
tools: [generate_image, finish]
excludeDefaultComponents: true
inheritMcp: false
rules: []
---
# kineto-artist

Make exactly one image from the request with generate_image, passing every reference image it \
names in ImagePaths, then finish. Never run commands, read other files or browse.
";

/// What Mini-Agent's ACP server is started with: its own configuration,
/// which holds the director's key, with every tool switched off.
const MINI_AGENT_BOOTSTRAP: &str = "import asyncio
from mini_agent.config import Config
from mini_agent.acp import run_acp_server
config = Config.load()
tools = config.tools
tools.enable_file_tools = False
tools.enable_bash = False
tools.enable_note = False
tools.enable_skills = False
tools.enable_mcp = False
asyncio.run(run_acp_server(config))
";

/// The longest prompt Kimi Code can take as a command-line argument here.
const fn longest_argument() -> usize {
    if cfg!(windows) {
        // The whole command line is limited to 32,767 characters.
        28_000
    } else if cfg!(target_os = "macos") {
        512 * 1024
    } else {
        // Linux limits one argument to 128 KiB.
        120 * 1024
    }
}

/// Words that mean the CLI wants the director to sign in.
fn asks_to_sign_in(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "not logged in",
        "please run /login",
        "log in",
        "login required",
        "sign in",
        "unauthorized",
        "401",
        "invalid api key",
        "authentication",
        "auth error",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

fn agent_error(message: &str) -> AgentError {
    if asks_to_sign_in(message) {
        AgentError::NotSignedIn
    } else {
        AgentError::Agent {
            code: 0,
            message: message
                .chars()
                .filter(|character| !character.is_control())
                .take(200)
                .collect(),
        }
    }
}

fn parse_lines(lines: &[String]) -> impl DoubleEndedIterator<Item = Value> + '_ {
    lines
        .iter()
        .filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
}

/// Claude Code's answer: the structured output its `result` carries, or its
/// text.
fn claude_answer(finished: &Finished) -> Result<String, AgentError> {
    let Some(result) = parse_lines(&finished.lines).rfind(|event| event["type"] == "result") else {
        return Err(failed_without_answer(finished));
    };
    let text = result["result"].as_str().unwrap_or_default();
    if result["is_error"] == true || result["subtype"] != "success" {
        return Err(match result["subtype"].as_str() {
            Some("error_max_turns") => AgentError::Truncated,
            _ => agent_error(text),
        });
    }
    match &result["structured_output"] {
        Value::Null => Ok(text.to_owned()),
        structured => Ok(structured.to_string()),
    }
}

/// Codex's answer: its last message.
fn codex_answer(finished: &Finished) -> Result<String, AgentError> {
    let mut answer = None;
    for event in parse_lines(&finished.lines) {
        match event["type"].as_str() {
            Some("item.completed") if event["item"]["type"] == "agent_message" => {
                answer = event["item"]["text"].as_str().map(str::to_owned);
            }
            Some("turn.failed") => {
                return Err(agent_error(
                    event["error"]["message"].as_str().unwrap_or_default(),
                ));
            }
            Some("error") => {
                return Err(agent_error(event["message"].as_str().unwrap_or_default()));
            }
            _ => {}
        }
    }
    answer.ok_or_else(|| failed_without_answer(finished))
}

/// Kimi Code's answer: the last thing the assistant said. The user's hooks
/// speak before it, as the assistant.
fn kimi_answer(finished: &Finished) -> Result<String, AgentError> {
    parse_lines(&finished.lines)
        .filter(|event| event["role"] == "assistant")
        .filter_map(|event| match &event["content"] {
            Value::String(text) => Some(text.clone()),
            Value::Array(parts) => Some(
                parts
                    .iter()
                    .filter_map(|part| part["text"].as_str())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .rfind(|text| !text.trim().is_empty())
        .ok_or_else(|| failed_without_answer(finished))
}

/// agy's answer: the response its `result` carries. Its answer tool adds two
/// fields of its own to a JSON answer; they are taken off.
fn agy_answer(finished: &Finished) -> Result<String, AgentError> {
    let Some(result) = parse_lines(&finished.lines).rfind(|event| event["event"] == "result")
    else {
        return Err(failed_without_answer(finished));
    };
    let result = &result["result"];
    if result["status"] != "SUCCESS" {
        let error = result["error"].as_str().unwrap_or_default();
        return Err(if error.to_ascii_lowercase().contains("timed out") {
            AgentError::Timeout
        } else {
            agent_error(error)
        });
    }
    let response = result["response"].as_str().unwrap_or_default().trim();
    if response.starts_with('{')
        && let Some(Ok(Value::Object(mut answer))) = serde_json::Deserializer::from_str(response)
            .into_iter::<Value>()
            .next()
    {
        answer.remove("toolAction");
        answer.remove("toolSummary");
        return Ok(Value::Object(answer).to_string());
    }
    Ok(response.to_owned())
}

/// Why an agent that gave no answer failed: what it last said on its error
/// stream, when it said anything.
fn failed_without_answer(finished: &Finished) -> AgentError {
    let said = finished
        .stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty());
    if asks_to_sign_in(&finished.stderr) {
        AgentError::NotSignedIn
    } else if finished.status.success() {
        AgentError::Empty
    } else if let Some(said) = said {
        agent_error(said)
    } else {
        AgentError::Exited
    }
}

/// A schema as Claude Code takes it: its validator knows no draft
/// declaration beyond draft-07, and refuses a schema that names one, so the
/// declaration and the schema's own ID are left out. What the answer must
/// satisfy is unchanged; kineto checks it against the whole schema.
fn schema_for_claude(schema: &str) -> String {
    match serde_json::from_str::<Value>(schema) {
        Ok(Value::Object(mut object)) => {
            object.remove("$schema");
            object.remove("$id");
            Value::Object(object).to_string()
        }
        _ => schema.to_owned(),
    }
}

/// An agent CLI the director signed in to, as an `llm.complete` backend.
#[derive(Debug, Clone)]
pub struct AgentCli {
    kind: AgentKind,
    /// The CLI, or for Mini-Agent the Python that runs it.
    executable: PathBuf,
    timeout: Duration,
    workspace_root: PathBuf,
    cancel: Option<Arc<AtomicBool>>,
}

impl AgentCli {
    #[must_use]
    pub fn new(kind: AgentKind, executable: PathBuf) -> Self {
        Self {
            kind,
            executable,
            timeout: DEFAULT_TIMEOUT,
            workspace_root: std::env::temp_dir(),
            cancel: None,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AgentKind {
        self.kind
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

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        if let Some(path) = search_path(&self.executable, &[]) {
            command.env("PATH", path);
        }
        // Started from inside a Claude Code session, as in development, it
        // would refuse to start another.
        command.env_remove("CLAUDECODE");
        command.env_remove("CLAUDE_CODE_ENTRYPOINT");
        command
    }

    fn mini_agent(&self) -> AcpAgent {
        let mut agent = AcpAgent::with_command(
            AgentCommand {
                executable: self.executable.clone(),
                args: vec!["-I".into(), "-c".into(), MINI_AGENT_BOOTSTRAP.into()],
                env: search_path(&self.executable, &[])
                    .map(|path| (OsString::from("PATH"), path))
                    .into_iter()
                    .collect(),
            },
            self.kind.name(),
        )
        .with_timeout(self.timeout)
        .with_workspace_root(self.workspace_root.clone());
        if let Some(cancel) = &self.cancel {
            agent = agent.with_cancel(Arc::clone(cancel));
        }
        agent
    }

    /// Ask once, as a one-shot run of the CLI.
    fn ask(&self, request: &LlmRequest) -> Result<String, AgentError> {
        let workspace = Workspace::create(&self.workspace_root)?;
        let prompt = prompt_text(request);
        let mut command = self.command();
        let stdin = match self.kind {
            AgentKind::ClaudeCode => {
                command.args([
                    "-p",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--tools",
                    "",
                    "--strict-mcp-config",
                    "--setting-sources",
                    "",
                    "--disable-slash-commands",
                    "--no-session-persistence",
                ]);
                if let Some(schema) = &request.schema {
                    command
                        .arg("--json-schema")
                        .arg(schema_for_claude(&schema.schema_json));
                }
                Some(prompt.into_bytes())
            }
            AgentKind::Codex => {
                command.args([
                    "exec",
                    "--json",
                    "--ignore-user-config",
                    "--ignore-rules",
                    "--ephemeral",
                    "--skip-git-repo-check",
                    "--sandbox",
                    "read-only",
                    "-c",
                    "approval_policy=\"never\"",
                    "-c",
                    "web_search=\"disabled\"",
                ]);
                for feature in CODEX_TOOL_FEATURES {
                    command.args(["--disable", feature]);
                }
                command.arg("-");
                Some(prompt.into_bytes())
            }
            AgentKind::KimiCode => {
                if prompt.len() > longest_argument() {
                    return Err(AgentError::PromptTooLong);
                }
                let agent = workspace.write("kineto-writer.md", KIMI_WRITER.as_bytes())?;
                command
                    .arg("-p")
                    .arg(&prompt)
                    .arg("--agent-file")
                    .arg(agent)
                    .args(["--output-format", "stream-json"]);
                None
            }
            AgentKind::Agy => {
                workspace.write(
                    ".agents/agents/kineto-writer/agent.md",
                    AGY_WRITER.as_bytes(),
                )?;
                command.args(["--agent", "kineto-writer"]);
                self.agy_arguments(&mut command);
                // Its answer tool hands back JSON only: a request for text
                // gets an object with the text in it.
                let schema = request
                    .schema
                    .as_ref()
                    .map_or(AGY_TEXT_SCHEMA, |schema| schema.schema_json.as_str());
                let path = workspace.write("schema.json", schema.as_bytes())?;
                command.arg("--json-schema").arg(path).arg("-p=");
                Some(agy_message(&prompt))
            }
            AgentKind::MiniAgent => unreachable!("Mini-Agent is driven over ACP"),
        };
        let finished = run_to_end(
            &mut command,
            &workspace,
            stdin,
            Instant::now() + self.timeout,
            self.cancel.as_ref(),
        )?;
        match self.kind {
            AgentKind::ClaudeCode => claude_answer(&finished),
            AgentKind::Codex => codex_answer(&finished),
            AgentKind::KimiCode => kimi_answer(&finished),
            AgentKind::Agy if request.schema.is_none() => {
                let answer = agy_answer(&finished)?;
                serde_json::from_str::<Value>(&answer)
                    .ok()
                    .and_then(|value| value["answer"].as_str().map(str::to_owned))
                    .ok_or(AgentError::Empty)
            }
            AgentKind::Agy => agy_answer(&finished),
            AgentKind::MiniAgent => unreachable!("Mini-Agent is driven over ACP"),
        }
    }

    fn agy_arguments(&self, command: &mut Command) {
        command.args([
            "--output-format",
            "stream-json",
            "--input-format",
            "stream-json",
            "--sandbox",
            "--disable-slash-commands",
            "--print-timeout",
        ]);
        command.arg(format!("{}s", self.timeout.as_secs().max(1)));
    }

    /// Make one image with agy: `prompt`, with `references` (file names and
    /// bytes) copied into the working directory for it to look at. Returns
    /// the image's bytes.
    ///
    /// agy keeps what it makes under its own data directory, named by the
    /// conversation; `images_root` is that directory's parent
    /// (`~/.gemini/antigravity-cli/brain`).
    pub fn make_image(
        &self,
        prompt: &str,
        references: &[(String, Vec<u8>)],
        images_root: &Path,
    ) -> Result<Vec<u8>, AgentError> {
        if self.kind != AgentKind::Agy {
            return Err(AgentError::Protocol("this agent makes no images"));
        }
        if is_cancelled(self.cancel.as_ref()) {
            return Err(AgentError::Cancelled);
        }
        let workspace = Workspace::create(&self.workspace_root)?;
        workspace.write(
            ".agents/agents/kineto-artist/agent.md",
            AGY_ARTIST.as_bytes(),
        )?;
        let mut names = Vec::with_capacity(references.len());
        for (name, bytes) in references {
            if name.contains(['/', '\\']) || name.starts_with('.') {
                return Err(AgentError::Protocol("a reference is not a plain file name"));
            }
            workspace.write(&format!("references/{name}"), bytes)?;
            names.push(format!("references/{name}"));
        }
        let mut text = format!("Make one image named keyframe with generate_image.\n\n{prompt}\n");
        if !names.is_empty() {
            text.push_str(&format!(
                "\nPass these reference images in ImagePaths, in this order: {}.\n",
                serde_json::to_string(&names).unwrap_or_default()
            ));
        }
        let mut command = self.command();
        command.args(["--agent", "kineto-artist"]);
        self.agy_arguments(&mut command);
        command.arg("-p=");
        let finished = run_to_end(
            &mut command,
            &workspace,
            Some(agy_message(&text)),
            Instant::now() + self.timeout,
            self.cancel.as_ref(),
        )?;
        agy_answer(&finished)?;
        let conversation = parse_lines(&finished.lines)
            .rfind(|event| event["event"] == "result")
            .and_then(|result| {
                result["result"]["conversation_id"]
                    .as_str()
                    .map(str::to_owned)
            })
            .filter(|id| !id.is_empty() && !id.contains(['/', '\\', '.']))
            .ok_or(AgentError::Protocol("no conversation"))?;
        newest_image(&images_root.join(conversation)).ok_or(AgentError::Empty)
    }
}

/// agy's input: one user message.
fn agy_message(text: &str) -> Vec<u8> {
    let mut line = json!({"event": "user", "message": {"content": text}}).to_string();
    line.push('\n');
    line.into_bytes()
}

/// The newest image file in `directory`.
fn newest_image(directory: &Path) -> Option<Vec<u8>> {
    let newest = fs::read_dir(directory)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    matches!(
                        extension.to_ascii_lowercase().as_str(),
                        "jpg" | "jpeg" | "png" | "webp"
                    )
                })
        })
        .max_by_key(|entry| entry.metadata().and_then(|meta| meta.modified()).ok())?;
    fs::read(newest.path()).ok()
}

/// Where agy keeps what it makes, as `env` describes the user's home.
#[must_use]
pub fn agy_images_root(env: &impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    home(env).map(|home| home.join(".gemini/antigravity-cli/brain"))
}

impl LlmBackend for AgentCli {
    type Error = AgentError;

    fn info(&self) -> LlmBackendInfo {
        LlmBackendInfo {
            provider: self.kind.name().to_owned(),
            // The agent uses the model its own configuration selects.
            model: "agent-default".to_owned(),
            capabilities: LlmCapabilities {
                context_window_tokens: None,
                tools: false,
                structured_output: false,
                vision_input: false,
            },
            billing: self.kind.billing(),
        }
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, AgentError> {
        if is_cancelled(self.cancel.as_ref()) {
            return Err(AgentError::Cancelled);
        }
        if self.kind == AgentKind::MiniAgent {
            return self.mini_agent().complete(request);
        }
        let text = self.ask(request)?;
        if text.trim().is_empty() {
            return Err(AgentError::Empty);
        }
        Ok(LlmResponse {
            text,
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

    fn finished(lines: &[&str], code: i32) -> Finished {
        #[cfg(unix)]
        let status = {
            use std::os::unix::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code << 8)
        };
        #[cfg(windows)]
        let status = {
            use std::os::windows::process::ExitStatusExt;
            std::process::ExitStatus::from_raw(code as u32)
        };
        Finished {
            lines: lines.iter().map(|line| (*line).to_owned()).collect(),
            stderr: String::new(),
            status,
        }
    }

    #[test]
    fn every_agent_is_named_once_and_found_by_its_name() {
        for kind in AgentKind::ALL {
            assert_eq!(AgentKind::from_name(kind.name()), Some(kind));
        }
        assert_eq!(AgentKind::from_name("gpt"), None);
        assert!(AgentKind::Agy.makes_images());
        assert!(!AgentKind::ClaudeCode.makes_images());
    }

    #[test]
    fn claude_code_answers_with_its_structured_output_or_its_text() {
        let structured = finished(
            &[
                r#"{"type":"system","subtype":"init","tools":["StructuredOutput"]}"#,
                r#"{"type":"result","subtype":"success","is_error":false,"result":"{\"a\":1}","structured_output":{"a":1}}"#,
            ],
            0,
        );
        assert_eq!(claude_answer(&structured).unwrap(), r#"{"a":1}"#);
        let text = finished(
            &[r#"{"type":"result","subtype":"success","is_error":false,"result":"plain"}"#],
            0,
        );
        assert_eq!(claude_answer(&text).unwrap(), "plain");
        let signed_out = finished(
            &[
                r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#,
            ],
            1,
        );
        assert_eq!(claude_answer(&signed_out), Err(AgentError::NotSignedIn));
    }

    #[test]
    fn claude_code_is_given_a_schema_without_its_draft_declaration() {
        let schema = r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"https://kineto.example/x","type":"object","required":["a"]}"#;
        let given: Value = serde_json::from_str(&schema_for_claude(schema)).unwrap();
        assert_eq!(given, json!({"type": "object", "required": ["a"]}));
        assert_eq!(schema_for_claude("not json"), "not json");
    }

    #[test]
    fn an_agent_that_exits_says_why_in_its_own_words() {
        let mut failed = finished(&[], 1);
        failed.stderr =
            "Error: --json-schema is not a valid JSON Schema: no schema with key\n".to_owned();
        assert!(matches!(
            claude_answer(&failed),
            Err(AgentError::Agent { message, .. }) if message.contains("--json-schema")
        ));
        assert_eq!(claude_answer(&finished(&[], 1)), Err(AgentError::Exited));
    }

    #[test]
    fn codex_answers_with_its_last_message_and_reports_a_failed_turn() {
        let answered = finished(
            &[
                r#"{"type":"thread.started"}"#,
                r#"{"type":"item.completed","item":{"type":"error","message":"Model metadata not found"}}"#,
                r#"{"type":"item.completed","item":{"type":"agent_message","text":"first"}}"#,
                r#"{"type":"item.completed","item":{"type":"agent_message","text":"{\"a\":1}"}}"#,
                r#"{"type":"turn.completed"}"#,
            ],
            0,
        );
        assert_eq!(codex_answer(&answered).unwrap(), r#"{"a":1}"#);
        let failed = finished(
            &[r#"{"type":"turn.failed","error":{"message":"The model is not supported"}}"#],
            1,
        );
        assert!(matches!(
            codex_answer(&failed),
            Err(AgentError::Agent { .. })
        ));
    }

    #[test]
    fn kimi_code_answers_after_the_users_hooks_speak() {
        let answered = finished(
            &[
                r#"{"role":"meta","type":"system.version","version":"2.1.1"}"#,
                r#"{"role":"assistant","content":"UserPromptSubmit hook\n\nDo something else first."}"#,
                r#"{"role":"assistant","content":"{\"a\":1}"}"#,
                r#"{"role":"meta","type":"session.resume_hint"}"#,
            ],
            0,
        );
        assert_eq!(kimi_answer(&answered).unwrap(), r#"{"a":1}"#);
        assert_eq!(kimi_answer(&finished(&[], 0)), Err(AgentError::Empty));
        assert_eq!(kimi_answer(&finished(&[], 1)), Err(AgentError::Exited));
    }

    #[test]
    fn agy_answers_without_its_answer_tools_own_fields() {
        let answered = finished(
            &[
                r#"{"event":"init","init":{}}"#,
                r#"{"event":"result","result":{"status":"SUCCESS","conversation_id":"c1","response":"{\"a\":1,\"toolAction\":\"Finishing task\",\"toolSummary\":\"Task completion\"}\nDone."}}"#,
            ],
            0,
        );
        assert_eq!(agy_answer(&answered).unwrap(), r#"{"a":1}"#);
        let failed = finished(
            &[
                r#"{"event":"result","result":{"status":"ERROR","error":"Print mode: auth error: token expired"}}"#,
            ],
            1,
        );
        assert_eq!(agy_answer(&failed), Err(AgentError::NotSignedIn));
    }

    #[test]
    fn agents_are_found_where_they_are_installed() {
        let root =
            std::env::temp_dir().join(format!("kineto-agents-locate-{}", std::process::id()));
        let bin = root.join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let name = executable_names("claude").remove(0);
        fs::write(bin.join(&name), "").unwrap();
        let home = root.to_string_lossy().into_owned();
        let env = |variable: &str| match variable {
            "HOME" | "USERPROFILE" => Some(home.clone()),
            "PATH" => Some(String::new()),
            _ => None,
        };
        assert_eq!(
            locate(AgentKind::ClaudeCode, &env),
            Some(bin.join(&name)),
            "found in the usual place without the shell's search path"
        );
        assert_eq!(locate(AgentKind::Codex, &env), None);
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn mini_agent_runs_on_the_python_its_launcher_names() {
        let root = std::env::temp_dir().join(format!("kineto-agents-mini-{}", std::process::id()));
        let bin = root.join(".local/bin");
        let python = root.join("tools/mini-agent/bin/python");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(python.parent().unwrap()).unwrap();
        fs::write(&python, "").unwrap();
        fs::write(
            bin.join("mini-agent-acp"),
            format!("#!{}\nimport sys\n", python.display()),
        )
        .unwrap();
        let home = root.to_string_lossy().into_owned();
        let env = |variable: &str| match variable {
            "HOME" => Some(home.clone()),
            _ => None,
        };
        assert_eq!(locate(AgentKind::MiniAgent, &env), Some(python));
        let _ = fs::remove_dir_all(&root);
    }
}
