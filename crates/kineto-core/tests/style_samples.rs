//! Style exploration end to end with MiniMax offline: sampling directions,
//! refining one with a note, selecting, rejecting and locking, and every
//! crash window of a paid sample.
//!
//! Crash tests re-run this binary as a child with `KINETO_STYLE_CRASH` set
//! to `<point>:<project>`. The child samples a direction and calls
//! `std::process::abort()` at that point: no destructors, no rollback. The
//! parent then inspects what is left and carries on as the app would.

use std::{
    collections::{BTreeMap, VecDeque},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use kineto_core::{
    bible,
    generation::{
        GenerationError, GenerationHooks, GenerationPoint, ImageProvider, NoHooks, Prerequisite,
    },
    minimax::MinimaxKeyframes,
    screenplay, story,
    style::{self, SampleRequest, Submission},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    bible::SAMPLE_BIBLE_JSON,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    screenplay::SAMPLE_SCREENPLAY_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style::{self as document, SampleStatus},
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::secret::SecretString;

const CHILD: &str = "KINETO_STYLE_CRASH";

/// MiniMax `image-01`, offline. Answers from a script, then with images.
#[derive(Default)]
struct FakeMinimax {
    calls: Arc<AtomicU32>,
    prompts: Arc<Mutex<Vec<String>>>,
    script: Mutex<VecDeque<HttpResponse>>,
}

impl Transport for FakeMinimax {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let body: serde_json::Value = serde_json::from_slice(body).unwrap();
        self.prompts
            .lock()
            .unwrap()
            .push(body["prompt"].as_str().unwrap().to_owned());
        if let Some(scripted) = self.script.lock().unwrap().pop_front() {
            return Ok(scripted);
        }
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x53, u8::try_from(call).unwrap()];
        Ok(HttpResponse {
            status: 200,
            body: format!(
                r#"{{"id":"s{call}","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                base64::engine::general_purpose::STANDARD.encode(jpeg)
            )
            .into_bytes(),
        })
    }

    fn get(&self, url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        unreachable!("style samples make no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("style samples download nothing: {url}")
    }
}

struct Minimax {
    client: MinimaxClient<FakeMinimax>,
    calls: Arc<AtomicU32>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl Minimax {
    fn new(script: impl IntoIterator<Item = HttpResponse>) -> Self {
        let fake = FakeMinimax {
            script: Mutex::new(script.into_iter().collect()),
            ..FakeMinimax::default()
        };
        let calls = Arc::clone(&fake.calls);
        let prompts = Arc::clone(&fake.prompts);
        Self {
            client: MinimaxClient::with_transport(
                fake,
                SecretString::new("sk-test".to_owned()).unwrap(),
                "https://example.test",
            ),
            calls,
            prompts,
        }
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    fn sample(
        &self,
        project: &CanonicalProject,
        preset: &str,
        submission: Submission,
    ) -> Result<document::StyleSample, GenerationError> {
        style::sample_with(
            project,
            MinimaxKeyframes::new(project, Some(&self.client)),
            SampleRequest {
                preset,
                ..SampleRequest::default()
            },
            submission,
            &mut NoHooks,
        )
    }
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
        let path = env::temp_dir().join(format!("kineto-style-{}-{nonce}", std::process::id()));
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
        project_id: "style_samples".to_owned(),
        title: "Style Samples".to_owned(),
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

/// A project with a locked screenplay and, when `with_bible`, a bible.
fn film(target: &Path, with_bible: bool) -> CanonicalProject {
    let project =
        CanonicalProject::create(target, manifest(), b"A filmmaker hears the last line.\n")
            .unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    kineto_project::screenplay::lock(&project, 1).unwrap();
    if with_bible {
        bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).unwrap();
    }
    project
}

fn intent(target: &Path, artifact_id: &str) -> Option<serde_json::Value> {
    fs::read(target.join(format!(".kineto/jobs/intents/{artifact_id}.json")))
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).unwrap())
}

#[test]
fn the_director_samples_refines_and_locks_a_style_that_survives_losing_the_cache() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::new([]);

    for preset in ["film_realism", "film_noir", "soft_fairy_tale"] {
        minimax.sample(&project, preset, Submission::Once).unwrap();
    }
    assert_eq!(minimax.calls(), 3);
    let noir = minimax.prompts.lock().unwrap()[1].clone();
    assert!(noir.starts_with("Black-and-white film noir still"));
    assert!(noir.contains("Scene: INT. SMALL OFFICE - NIGHT, night."));
    assert!(noir.contains("Wardrobe: A dark wool coat over a black sweater."));

    let refined = style::refine_with(
        &project,
        MinimaxKeyframes::new(&project, Some(&minimax.client)),
        "style_s0002",
        "less smoke, more rain on the glass",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(refined.artifact_id, "style_s0004");
    assert_eq!(refined.preset, "film_noir");
    assert_eq!(refined.refines.as_deref(), Some("style_s0002"));
    assert_eq!(
        refined.note.as_deref(),
        Some("less smoke, more rain on the glass")
    );
    assert!(
        minimax.prompts.lock().unwrap()[3]
            .contains("Director's note: less smoke, more rain on the glass.")
    );
    assert_eq!(refined.scene_id, "office_night");
    assert_eq!(refined.provider, "minimax");
    assert_eq!(refined.model, "image-01");
    assert_eq!(refined.media_path, "style/samples/style_s0004.jpg");
    assert_eq!(refined.provenance.compiler_version, "style-sample/1");
    assert_eq!(
        refined.provenance.dropped_constraints[0].constraint,
        "style.avoid"
    );
    let depends_on: Vec<_> = refined
        .dependencies
        .iter()
        .map(|dependency| dependency.artifact_id.as_str())
        .collect();
    assert_eq!(depends_on, ["screenplay_scene.office_night", "bible_r0001"]);
    assert!(
        !target.join(".kineto/generation/style_s0004.json").exists(),
        "the receipt is gone once the sample is recorded"
    );
    assert!(
        !target
            .join(".kineto/generation/style_s0004.style.json")
            .exists()
    );

    document::select(&project, "style_s0004").unwrap();
    document::reject(&project, "style_s0003").unwrap();
    let locked = document::lock(&project, "style_s0004").unwrap();
    assert_eq!(
        locked.note.as_deref(),
        Some("less smoke, more rain on the glass")
    );
    assert!(locked.directives.starts_with("Black-and-white film noir"));
    drop(project);

    fs::remove_dir_all(target.join(".kineto")).unwrap();
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(document::locked(&project).unwrap(), Some(locked));
    let statuses: Vec<_> = document::samples(&project)
        .unwrap()
        .iter()
        .map(|sample| sample.status)
        .collect();
    assert_eq!(
        statuses,
        [
            SampleStatus::Candidate,
            SampleStatus::Candidate,
            SampleStatus::Rejected,
            SampleStatus::Selected
        ]
    );
    // The images are where the records say, unchanged.
    for sample in document::samples(&project).unwrap() {
        let bytes = fs::read(target.join(&sample.media_path)).unwrap();
        assert_eq!(
            kineto_project::ContentHash::of(&bytes).as_str(),
            sample.content_hash
        );
    }
}

#[test]
fn nothing_is_asked_or_written_without_what_a_sample_needs() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = CanonicalProject::create(&target, manifest(), b"A filmmaker waits.\n").unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    let minimax = Minimax::new([]);
    assert!(matches!(
        minimax.sample(&project, "film_noir", Submission::Once),
        Err(GenerationError::Prerequisite(
            Prerequisite::LockedScreenplay
        ))
    ));
    kineto_project::screenplay::lock(&project, 1).unwrap();
    assert!(matches!(
        minimax.sample(&project, "watercolour", Submission::Once),
        Err(GenerationError::Prerequisite(Prerequisite::StylePreset))
    ));
    assert!(matches!(
        style::sample(
            &project,
            &ImageProvider::LocalDemo,
            SampleRequest {
                preset: "film_noir",
                ..SampleRequest::default()
            },
            Submission::Once,
            &mut NoHooks,
        ),
        Err(GenerationError::Prerequisite(
            Prerequisite::RealImageProvider
        ))
    ));
    assert!(matches!(
        style::sample(
            &project,
            &ImageProvider::Minimax(None),
            SampleRequest {
                preset: "film_noir",
                ..SampleRequest::default()
            },
            Submission::Once,
            &mut NoHooks,
        ),
        Err(GenerationError::NotConfigured)
    ));
    assert_eq!(minimax.calls(), 0);
    assert!(!target.join("style").exists());
    assert!(intent(&target, "style_s0001").is_none());

    // Without a bible the sample is made from the screenplay alone.
    let sample = minimax
        .sample(&project, "anime_cel", Submission::Once)
        .unwrap();
    assert_eq!(sample.dependencies.len(), 1);
}

#[test]
fn a_call_minimax_refused_can_be_made_again_unasked() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let refused = HttpResponse {
        status: 200,
        body: br#"{"base_resp":{"status_code":1026,"status_msg":"sensitive content"}}"#.to_vec(),
    };
    let minimax = Minimax::new([refused]);
    assert!(
        minimax
            .sample(&project, "film_noir", Submission::Once)
            .is_err()
    );
    assert!(
        target
            .join(".kineto/generation/style_s0001.refused.json")
            .exists()
    );
    let sample = minimax
        .sample(&project, "film_realism", Submission::Once)
        .unwrap();
    assert_eq!(sample.artifact_id, "style_s0001");
    assert_eq!(sample.preset, "film_realism");
    assert_eq!(minimax.calls(), 2);
    assert!(
        !target
            .join(".kineto/generation/style_s0001.refused.json")
            .exists()
    );
}

/// Abort the process at one point of a sample.
struct AbortAt(GenerationPoint);

impl GenerationHooks for AbortAt {
    fn reached(&mut self, point: GenerationPoint) {
        if point == self.0 {
            std::process::abort();
        }
    }
}

/// The child half. A no-op unless the parent asked for a crash.
#[test]
fn crash_child() {
    let Ok(value) = env::var(CHILD) else {
        return;
    };
    let (point, path) = value.split_once(':').unwrap();
    let project = CanonicalProject::open(path).expect("child opens the project");
    let minimax = Minimax::new([]);
    let abort = || std::process::abort();
    let mut adapter = MinimaxKeyframes::new(&project, Some(&minimax.client));
    let mut hooks = AbortAt(match point {
        "prepared" => GenerationPoint::Prepared { index: 0 },
        "invoked" => GenerationPoint::Invoked,
        "attached" => GenerationPoint::Attached,
        _ => GenerationPoint::Prepared { index: 99 },
    });
    if point == "answered" {
        adapter = adapter.when_answered(&abort);
    }
    let _ = style::sample_with(
        &project,
        adapter,
        SampleRequest {
            preset: "film_noir",
            ..SampleRequest::default()
        },
        Submission::Once,
        &mut hooks,
    );
    panic!("the child must abort at {point}");
}

/// Sample in a child that dies at `point`.
fn crash(temp: &TempDir, target: &Path, point: &str) {
    let status = Command::new(env::current_exe().unwrap())
        .args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, format!("{point}:{}", target.display()))
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
}

#[test]
fn killed_as_minimax_answered_a_sample_waits_for_the_director() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(film(&target, true));
    crash(&temp, &target, "answered");

    let intent = intent(&target, "style_s0001").unwrap();
    assert_eq!(intent["state"], "prepared");
    assert_eq!(intent["attempts"], 1);
    assert!(!target.join(".kineto/generation/style_s0001.json").exists());
    assert!(!target.join("style/samples.json").exists());

    let project = CanonicalProject::open(&target).unwrap();
    let recovery = style::recover(&project).unwrap();
    assert_eq!(recovery.unresolved, ["style_s0001"]);
    assert!(recovery.attached.is_empty());

    let minimax = Minimax::new([]);
    assert!(matches!(
        minimax.sample(&project, "film_realism", Submission::Once),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert_eq!(minimax.calls(), 0, "never paid twice unasked");

    // The director decides to risk it.
    let sample = minimax
        .sample(&project, "film_realism", Submission::RiskPayingTwice)
        .unwrap();
    assert_eq!(sample.artifact_id, "style_s0001");
    assert_eq!(minimax.calls(), 1);
    assert!(style::recover(&project).unwrap().unresolved.is_empty());
}

#[test]
fn a_sample_answered_before_a_crash_is_kept_without_paying_again() {
    // Completed when the project opens, or by the next sample on its own.
    for (point, opened) in [
        ("invoked", true),
        ("invoked", false),
        ("attached", true),
        ("attached", false),
    ] {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        drop(film(&target, true));
        crash(&temp, &target, point);

        let project = CanonicalProject::open(&target).unwrap();
        if opened {
            let recovery = style::recover(&project).unwrap();
            assert!(recovery.unresolved.is_empty(), "{point}");
            assert_eq!(recovery.needs_attention, 0, "{point}");
            assert_eq!(document::samples(&project).unwrap().len(), 1, "{point}");
        }

        // The next sample takes the next ID and the only call is its own.
        let minimax = Minimax::new([]);
        let next = minimax
            .sample(&project, "film_realism", Submission::Once)
            .unwrap();
        assert_eq!(next.artifact_id, "style_s0002", "{point} {opened}");
        assert_eq!(minimax.calls(), 1);
        let samples = document::samples(&project).unwrap();
        assert_eq!(samples[0].preset, "film_noir");
        assert!(target.join(&samples[0].media_path).exists());
        assert_eq!(
            intent(&target, "style_s0001").unwrap()["state"],
            "reconciled"
        );
        assert!(
            !target
                .join(".kineto/generation/style_s0001.style.json")
                .exists()
        );
    }
}

#[test]
fn a_sample_interrupted_before_its_call_costs_nothing_and_makes_way() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(film(&target, true));
    crash(&temp, &target, "prepared");
    assert_eq!(intent(&target, "style_s0001").unwrap()["attempts"], 0);

    let project = CanonicalProject::open(&target).unwrap();
    assert!(style::recover(&project).unwrap().unresolved.is_empty());
    // Another direction takes the same ID; the unsent one is retired.
    let minimax = Minimax::new([]);
    let sample = minimax
        .sample(&project, "soft_fairy_tale", Submission::Once)
        .unwrap();
    assert_eq!(sample.artifact_id, "style_s0001");
    assert_eq!(sample.preset, "soft_fairy_tale");
    assert_eq!(minimax.calls(), 1);
}
