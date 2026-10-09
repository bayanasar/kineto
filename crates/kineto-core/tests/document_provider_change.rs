//! A director whose model failed chooses another and asks again. The failed
//! attempt was recorded under the first provider; it must not refuse the
//! second one's request, and an attempt the first may have been paid for
//! still waits for the director.

use std::{
    collections::BTreeMap,
    env, fs,
    path::PathBuf,
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
    revisions::DocumentKind,
    story::SAMPLE_ANALYSIS_JSON,
};

/// A fake model recorded under a provider of the test's choosing.
struct Named(&'static str, FakeLlm);

impl LlmBackend for Named {
    type Error = FakeLlmError;

    fn info(&self) -> LlmBackendInfo {
        LlmBackendInfo {
            provider: self.0.to_owned(),
            ..self.1.info()
        }
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, FakeLlmError> {
        self.1.complete(request)
    }

    fn classify(&self, error: &FakeLlmError) -> ErrorClass {
        self.1.classify(error)
    }

    fn may_have_lost_an_answer(&self, error: &FakeLlmError) -> bool {
        self.1.may_have_lost_an_answer(error)
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-provider-change-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
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

fn project(temp: &TempDir) -> CanonicalProject {
    let manifest = ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "provider_change".to_owned(),
        title: "Provider Change".to_owned(),
        created_at: "2026-10-08T00:00:00Z".to_owned(),
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
    };
    let target = temp.0.join("film");
    drop(CanonicalProject::create(&target, manifest, b"A filmmaker waits.\n").unwrap());
    CanonicalProject::open(&target).unwrap()
}

#[test]
fn a_model_refused_by_one_provider_is_asked_of_the_next() {
    let temp = TempDir::new();
    let project = project(&temp);
    let first = Named("mini-agent", FakeLlm::failing(FakeLlmError::Terminal));
    assert!(story::analyze(&project, &first).is_err());

    let second = Named("codex", FakeLlm::answering([SAMPLE_ANALYSIS_JSON]));
    let record = story::analyze(&project, &second).expect("the next provider is asked");
    assert_eq!(record.artifact_id, "story_analysis_r0001");
    assert_eq!(record.provider.as_deref(), Some("codex"));
    assert_eq!(second.1.requests().len(), 1);
}

#[test]
fn an_attempt_another_provider_may_have_been_paid_for_waits_for_the_director() {
    let temp = TempDir::new();
    let project = project(&temp);
    let first = Named("mini-agent", FakeLlm::failing(FakeLlmError::Retryable));
    assert!(story::analyze(&project, &first).is_err());

    // Whether the first provider charged for a call it never answered is not
    // known, so asking again is the director's call, even of a free model.
    let second = Named("codex", FakeLlm::answering([SAMPLE_ANALYSIS_JSON]));
    assert!(matches!(
        story::analyze(&project, &second),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert!(second.1.requests().is_empty());

    assert!(documents::risk_paying_twice(&project, "codex", DocumentKind::StoryAnalysis).unwrap());
    let record = story::analyze(&project, &second).expect("asked once the director decided");
    assert_eq!(record.provider.as_deref(), Some("codex"));
    assert_eq!(second.1.requests().len(), 1);
}
