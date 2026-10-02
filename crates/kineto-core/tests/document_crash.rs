//! Kill the process the moment a metered model has answered a document
//! request, before the document or its receipt is written, then prove that
//! the request is never paid for again unasked: the analysis is unresolved
//! until the director chooses to risk paying twice.
//!
//! The test binary re-runs itself as a child with `KINETO_DOCUMENT_CRASH`
//! set. The child analyses the story and calls `std::process::abort()` as
//! soon as the model answers: no destructors, no rollback. The parent then
//! inspects what is left and asks again.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use kineto_core::{documents, generation::GenerationError, story};
use kineto_jobs::ErrorClass;
use kineto_llm::{
    LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse,
    fake::{FakeLlm, FakeLlmError},
};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    revisions::{self, DocumentKind},
    story::SAMPLE_ANALYSIS_JSON,
};

const CHILD: &str = "KINETO_DOCUMENT_CRASH";

/// A metered model whose answer arrives just as the process dies.
struct AnswersThenDies(FakeLlm);

impl LlmBackend for AnswersThenDies {
    type Error = FakeLlmError;

    fn info(&self) -> LlmBackendInfo {
        self.0.info()
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, FakeLlmError> {
        let _answered = self.0.complete(request)?;
        std::process::abort();
    }

    fn classify(&self, _error: &FakeLlmError) -> ErrorClass {
        ErrorClass::Terminal
    }
}

/// The child half. A no-op unless the parent asked for a crash.
#[test]
fn crash_child() {
    let Ok(path) = env::var(CHILD) else {
        return;
    };
    let project = CanonicalProject::open(path).expect("child opens the project");
    let _ = story::analyze(
        &project,
        &AnswersThenDies(FakeLlm::answering([SAMPLE_ANALYSIS_JSON]).metered()),
    );
    panic!("the child must abort once the model answers");
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // SystemTime is only microsecond-precise on macOS, so tests running in
        // parallel could otherwise share a directory name.
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = format!("{nonce}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let path = env::temp_dir().join(format!(
            "kineto-document-crash-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "document_crash".to_owned(),
        title: "Document Crash".to_owned(),
        created_at: "2026-10-02T00:00:00Z".to_owned(),
        source: ProjectSource {
            kind: "text".to_owned(),
            path: "source/story.txt".to_owned(),
            extra: BTreeMap::new(),
        },
        defaults: ProjectDefaults {
            language: "en".to_owned(),
            extra: BTreeMap::new(),
        },
        workflow: ProjectWorkflow {
            recipe_id: "default-film".to_owned(),
            recipe_version: 1,
            extra: BTreeMap::new(),
        },
        extra: BTreeMap::new(),
    }
}

fn intent(target: &Path) -> serde_json::Value {
    serde_json::from_slice(
        &fs::read(target.join(".kineto/jobs/intents/story_analysis_r0001.json")).unwrap(),
    )
    .unwrap()
}

#[test]
fn killed_as_a_metered_model_answered_the_analysis_waits_for_the_director() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(CanonicalProject::create(&target, manifest(), b"A filmmaker waits.\n").unwrap());

    let status = Command::new(env::current_exe().unwrap())
        .args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, &target)
        // An aborted child may write `core*` into its working directory.
        .current_dir(&temp.0)
        .status()
        .unwrap();
    assert!(!status.success(), "the child must not exit cleanly");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(6), "the child must die by SIGABRT");
    }

    // The window under test: the call answered and counted, nothing kept.
    let intent = intent(&target);
    assert_eq!(intent["state"], "prepared");
    assert_eq!(intent["attempts"], 1);
    assert!(intent["last_error_class"].is_null());
    let generation = target.join(".kineto/generation");
    assert!(
        !generation.exists() || fs::read_dir(&generation).unwrap().count() == 0,
        "no receipt and no refusal"
    );
    assert!(!target.join("story/analysis/revisions.json").exists());

    // Opening the project again never asks a model.
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(
        documents::recover(&project, "fake-llm").unwrap(),
        (Vec::new(), 0)
    );

    let backend = FakeLlm::answering([SAMPLE_ANALYSIS_JSON]).metered();
    assert!(matches!(
        story::analyze(&project, &backend),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert!(backend.requests().is_empty(), "never paid twice unasked");

    // The director decides to risk it.
    assert!(
        documents::risk_paying_twice(&project, "fake-llm", DocumentKind::StoryAnalysis).unwrap()
    );
    let record = story::analyze(&project, &backend).unwrap();
    assert_eq!(record.artifact_id, "story_analysis_r0001");
    assert_eq!(backend.requests().len(), 1);
    assert_eq!(
        revisions::current(&project, DocumentKind::StoryAnalysis)
            .unwrap()
            .unwrap(),
        record
    );
}
