//! The ACP client against a scripted agent process.

use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

use kineto_jobs::ErrorClass;
use kineto_llm::{
    LlmBackend, LlmRequest, Message, OutputSchema,
    acp::{AcpAgent, AgentCommand, AgentError},
};

struct Sandbox(PathBuf);

impl Sandbox {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "kineto-acp-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn log(&self) -> String {
        fs::read_to_string(self.0.join("agent.log")).unwrap_or_default()
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn agent(sandbox: &Sandbox, scenario: &str) -> AcpAgent {
    AcpAgent::with_command(
        AgentCommand {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_fake_acp_agent")),
            args: Vec::new(),
            env: vec![
                ("FAKE_ACP_SCENARIO".into(), scenario.into()),
                (
                    "FAKE_ACP_LOG".into(),
                    sandbox.0.join("agent.log").into_os_string(),
                ),
            ],
        },
        "fake-agent",
    )
    .with_workspace_root(sandbox.0.join("workspaces"))
    .with_timeout(Duration::from_secs(3))
}

fn request() -> LlmRequest {
    LlmRequest {
        messages: vec![Message::system("You analyse."), Message::user("Text.")],
        schema: Some(OutputSchema {
            name: "result".to_owned(),
            schema_json: r#"{"type":"object"}"#.to_owned(),
        }),
        max_output_tokens: None,
    }
}

#[test]
fn an_agent_answers_without_being_granted_anything() {
    let sandbox = Sandbox::new();
    let response = agent(&sandbox, "answer").complete(&request()).unwrap();
    assert_eq!(
        response.text, r#"{"ok":true}"#,
        "thoughts are not part of the answer"
    );
    let log = sandbox.log();
    assert!(log.contains("permission granted false"), "{log}");
    assert!(log.contains("file read false"), "{log}");
    assert!(log.contains("deleted s1"), "the session is removed: {log}");
    // It ran in an empty directory of its own, removed afterwards.
    let cwd = log
        .lines()
        .find_map(|line| line.strip_prefix("cwd "))
        .unwrap();
    assert!(cwd.contains("kineto-agent-"));
    assert!(!PathBuf::from(cwd).exists());
}

#[test]
fn agent_failures_are_classified() {
    for (scenario, expected) in [
        ("unauthenticated", AgentError::NotSignedIn),
        ("crash", AgentError::Exited),
        ("refusal", AgentError::Refused),
        ("garbage", AgentError::Protocol("not JSON-RPC")),
    ] {
        let sandbox = Sandbox::new();
        let error = agent(&sandbox, scenario).complete(&request()).unwrap_err();
        assert_eq!(error, expected, "{scenario}");
    }
    assert_eq!(AgentError::NotSignedIn.class(), ErrorClass::Terminal);
    assert_eq!(AgentError::Exited.class(), ErrorClass::Retryable);
}

#[test]
fn a_hanging_agent_is_stopped_at_its_deadline() {
    let sandbox = Sandbox::new();
    let started = Instant::now();
    let error = agent(&sandbox, "hang")
        .with_timeout(Duration::from_millis(500))
        .complete(&request())
        .unwrap_err();
    assert_eq!(error, AgentError::Timeout);
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn a_cancelled_call_stops_the_agent_at_once() {
    let sandbox = Sandbox::new();
    let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let agent = agent(&sandbox, "hang").with_cancel(std::sync::Arc::clone(&cancel));
    let started = Instant::now();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
    });
    let error = agent.complete(&request()).unwrap_err();
    canceller.join().unwrap();
    assert_eq!(error, AgentError::Cancelled);
    assert_eq!(error.class(), ErrorClass::Retryable);
    assert!(started.elapsed() < Duration::from_secs(5));
    // A cancelled agent starts nothing more.
    assert_eq!(
        agent.complete(&request()).unwrap_err(),
        AgentError::Cancelled
    );
}

#[test]
fn a_missing_executable_is_terminal() {
    let sandbox = Sandbox::new();
    let error = AcpAgent::with_command(
        AgentCommand {
            executable: sandbox.0.join("no-such-agent"),
            args: Vec::new(),
            env: Vec::new(),
        },
        "fake-agent",
    )
    .with_workspace_root(sandbox.0.clone())
    .complete(&request())
    .unwrap_err();
    assert!(matches!(error, AgentError::Spawn(_)));
    assert_eq!(error.class(), ErrorClass::Terminal);
}
