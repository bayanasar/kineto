//! The one-shot agent CLIs against a scripted stand-in: what each is
//! started with, what it is given, and how its answers and failures read.

use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use kineto_llm::{
    LlmBackend, LlmRequest, Message, OutputSchema,
    acp::AgentError,
    agents::{AgentCli, AgentKind},
};
use serde_json::Value;

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kineto-agent-cli-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("workspaces")).unwrap();
        Self(path)
    }

    fn record(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.0.join("agent.log")).unwrap()).unwrap()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The scripted CLI as `kind`, playing `scenario`. The scenario and log
/// travel in this process's environment, which every agent inherits; tests
/// in this file therefore run one at a time.
fn agent(sandbox: &Sandbox, kind: AgentKind, scenario: &str) -> AgentCli {
    // SAFETY: tests in this file hold LOCK while they change the environment.
    unsafe {
        std::env::set_var("FAKE_AGENT_SCENARIO", scenario);
        std::env::set_var("FAKE_AGENT_LOG", sandbox.0.join("agent.log"));
        std::env::set_var("CLAUDECODE", "1");
    }
    AgentCli::new(kind, PathBuf::from(env!("CARGO_BIN_EXE_fake_agent_cli")))
        .with_workspace_root(sandbox.0.join("workspaces"))
        .with_timeout(Duration::from_secs(5))
}

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn request() -> LlmRequest {
    LlmRequest {
        messages: vec![
            Message::system("You analyse."),
            Message::user("Source text."),
        ],
        schema: Some(OutputSchema {
            name: "result".to_owned(),
            schema_json: r#"{"type":"object"}"#.to_owned(),
        }),
        max_output_tokens: None,
    }
}

fn args(record: &Value) -> Vec<String> {
    record["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap().to_owned())
        .collect()
}

fn follows(args: &[String], flag: &str, value: &str) -> bool {
    args.windows(2)
        .any(|pair| pair[0] == flag && pair[1] == value)
}

fn assert_workspace_removed(sandbox: &Sandbox) {
    assert_eq!(
        fs::read_dir(sandbox.0.join("workspaces")).unwrap().count(),
        0,
        "the call's working directory is removed"
    );
}

#[test]
fn claude_code_gets_no_tools_settings_mcp_or_saved_session() {
    let _lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sandbox = Sandbox::new();
    let response = agent(&sandbox, AgentKind::ClaudeCode, "answer")
        .complete(&request())
        .unwrap();
    assert_eq!(response.text, r#"{"ok":true}"#);
    let record = sandbox.record();
    let args = args(&record);
    assert!(follows(&args, "--tools", ""), "{args:?}");
    assert!(follows(&args, "--setting-sources", ""), "{args:?}");
    assert!(
        follows(&args, "--json-schema", r#"{"type":"object"}"#),
        "{args:?}"
    );
    for flag in [
        "-p",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--no-session-persistence",
    ] {
        assert!(args.iter().any(|arg| arg == flag), "{flag}: {args:?}");
    }
    let stdin = record["stdin"].as_str().unwrap();
    assert!(
        stdin.contains("Source text."),
        "the prompt goes in on stdin"
    );
    assert_eq!(
        record["claudecode"], false,
        "a nested-session guard is cleared"
    );
    assert_workspace_removed(&sandbox);
}

#[test]
fn codex_ignores_the_users_configuration_and_runs_without_tools() {
    let _lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sandbox = Sandbox::new();
    let response = agent(&sandbox, AgentKind::Codex, "answer")
        .complete(&request())
        .unwrap();
    assert_eq!(response.text, r#"{"ok":true}"#);
    let record = sandbox.record();
    let args = args(&record);
    for flag in [
        "exec",
        "--json",
        "--ignore-user-config",
        "--ignore-rules",
        "--ephemeral",
        "--skip-git-repo-check",
    ] {
        assert!(args.iter().any(|arg| arg == flag), "{flag}: {args:?}");
    }
    assert!(follows(&args, "--sandbox", "read-only"), "{args:?}");
    for feature in ["shell_tool", "unified_exec", "apps", "plugins", "hooks"] {
        assert!(follows(&args, "--disable", feature), "{feature}: {args:?}");
    }
    assert_eq!(args.last().map(String::as_str), Some("-"));
    assert!(record["stdin"].as_str().unwrap().contains("Source text."));
    assert_workspace_removed(&sandbox);
}

#[test]
fn kimi_code_answers_through_an_agent_that_allows_no_tools() {
    let _lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sandbox = Sandbox::new();
    let response = agent(&sandbox, AgentKind::KimiCode, "answer")
        .complete(&request())
        .unwrap();
    assert_eq!(
        response.text, r#"{"ok":true}"#,
        "the hook's words are not the answer"
    );
    let record = sandbox.record();
    let args = args(&record);
    let prompt = &args[args.iter().position(|arg| arg == "-p").unwrap() + 1];
    assert!(prompt.contains("Source text."));
    let files = record["files"].as_array().unwrap();
    assert!(
        files.iter().any(|file| {
            let file = file.as_str().unwrap();
            file.starts_with("kineto-writer.md = ") && file.contains("tools: []")
        }),
        "{files:?}"
    );
    assert_workspace_removed(&sandbox);
}

#[test]
fn a_prompt_too_long_for_kimi_codes_command_line_is_refused_before_it_starts() {
    let _lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sandbox = Sandbox::new();
    let mut long = request();
    long.messages.push(Message::user("x".repeat(600 * 1024)));
    assert_eq!(
        agent(&sandbox, AgentKind::KimiCode, "answer").complete(&long),
        Err(AgentError::PromptTooLong)
    );
    assert!(
        !sandbox.0.join("agent.log").exists(),
        "it was never started"
    );
}

#[test]
fn agy_answers_through_its_own_agent_with_only_the_answer_tool() {
    let _lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sandbox = Sandbox::new();
    let response = agent(&sandbox, AgentKind::Agy, "answer")
        .complete(&request())
        .unwrap();
    assert_eq!(response.text, r#"{"ok":true}"#);
    let record = sandbox.record();
    let args = args(&record);
    assert!(follows(&args, "--agent", "kineto-writer"), "{args:?}");
    assert!(args.iter().any(|arg| arg == "--sandbox"), "{args:?}");
    let files = record["files"].as_array().unwrap();
    assert!(
        files.iter().any(|file| {
            let file = file.as_str().unwrap();
            file.starts_with(".agents/agents/kineto-writer/agent.md = ")
                && file.contains("tools: [finish]")
                && file.contains("excludeDefaultComponents: true")
                && file.contains("inheritMcp: false")
        }),
        "{files:?}"
    );
    let message: Value = serde_json::from_str(record["stdin"].as_str().unwrap().trim()).unwrap();
    assert_eq!(message["event"], "user");
    assert!(
        message["message"]["content"]
            .as_str()
            .unwrap()
            .contains("Source text.")
    );
    assert_workspace_removed(&sandbox);
}

#[test]
fn an_agent_that_is_not_signed_in_says_so() {
    let _lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for kind in [
        AgentKind::ClaudeCode,
        AgentKind::Codex,
        AgentKind::KimiCode,
        AgentKind::Agy,
    ] {
        let sandbox = Sandbox::new();
        let failure = agent(&sandbox, kind, "signed-out")
            .complete(&request())
            .unwrap_err();
        assert_eq!(failure, AgentError::NotSignedIn, "{kind:?}");
    }
}

#[test]
fn a_hanging_agent_is_stopped_at_its_deadline_and_a_cancelled_one_at_once() {
    let _lock = LOCK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let sandbox = Sandbox::new();
    let started = Instant::now();
    let hanging = agent(&sandbox, AgentKind::ClaudeCode, "hang")
        .with_timeout(Duration::from_millis(500))
        .complete(&request());
    assert_eq!(hanging, Err(AgentError::Timeout));
    assert!(started.elapsed() < Duration::from_secs(5));

    let cancel = Arc::new(AtomicBool::new(false));
    let agent = agent(&sandbox, AgentKind::Codex, "hang").with_cancel(Arc::clone(&cancel));
    let started = Instant::now();
    let canceller = thread::spawn(move || {
        thread::sleep(Duration::from_millis(300));
        cancel.store(true, Ordering::SeqCst);
    });
    assert_eq!(agent.complete(&request()), Err(AgentError::Cancelled));
    assert!(started.elapsed() < Duration::from_secs(3));
    canceller.join().unwrap();
    assert_workspace_removed(&sandbox);
}
