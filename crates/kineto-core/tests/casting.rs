//! Casting end to end with MiniMax offline: portrait candidates, the
//! reference pack made from the selected one with it as identity
//! reference, the lock, staleness, and a crash while the pack is paid for.
//!
//! The crash test re-runs this binary as a child with
//! `KINETO_CASTING_CRASH` set to the project. The child makes the pack and
//! calls `std::process::abort()` as soon as MiniMax answers the first call.

use std::{
    collections::BTreeMap,
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
    bible, cast,
    generation::{GenerationError, ImageProvider, NoHooks, Prerequisite},
    minimax::MinimaxKeyframes,
    screenplay, story,
    style::{self, SampleRequest, Submission},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    ContentHash,
    bible::SAMPLE_BIBLE_JSON,
    cast::{self as document, CastImageKind},
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    revisions::{self, DocumentKind},
    screenplay::SAMPLE_SCREENPLAY_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document,
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::{Price, secret::SecretString};

const CHILD: &str = "KINETO_CASTING_CRASH";

/// MiniMax `image-01`, offline: a distinct JPEG for every call, and every
/// request body kept.
#[derive(Default)]
struct FakeMinimax {
    calls: Arc<AtomicU32>,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Transport for FakeMinimax {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        self.bodies
            .lock()
            .unwrap()
            .push(serde_json::from_slice(body).unwrap());
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x43, u8::try_from(call).unwrap()];
        Ok(HttpResponse {
            status: 200,
            body: format!(
                r#"{{"id":"c{call}","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                base64::engine::general_purpose::STANDARD.encode(jpeg)
            )
            .into_bytes(),
        })
    }

    fn get(&self, url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        unreachable!("casting makes no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("casting downloads nothing: {url}")
    }
}

struct Minimax {
    client: MinimaxClient<FakeMinimax>,
    calls: Arc<AtomicU32>,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl Minimax {
    fn new() -> Self {
        let fake = FakeMinimax::default();
        let calls = Arc::clone(&fake.calls);
        let bodies = Arc::clone(&fake.bodies);
        Self {
            client: MinimaxClient::with_transport(
                fake,
                SecretString::new("sk-test".to_owned()).unwrap(),
                "https://example.test",
            ),
            calls,
            bodies,
        }
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    fn adapter<'a>(&'a self, project: &'a CanonicalProject) -> MinimaxKeyframes<'a, FakeMinimax> {
        MinimaxKeyframes::new(project, Some(&self.client))
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
        let path = env::temp_dir().join(format!("kineto-casting-{}-{nonce}", std::process::id()));
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
        project_id: "casting".to_owned(),
        title: "Casting".to_owned(),
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

/// Lock the style of a new sample of `preset`.
fn lock_style(project: &CanonicalProject, minimax: &Minimax, preset: &str) {
    let sample = style::sample_with(
        project,
        minimax.adapter(project),
        SampleRequest {
            preset,
            ..SampleRequest::default()
        },
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    style_document::select(project, &sample.artifact_id).unwrap();
    style_document::lock(project, &sample.artifact_id).unwrap();
}

/// A project with a locked screenplay and a bible; with a locked style when
/// `styled`.
fn film(target: &Path, styled: bool) -> CanonicalProject {
    let project =
        CanonicalProject::create(target, manifest(), b"A filmmaker hears the last line.\n")
            .unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    kineto_project::screenplay::lock(&project, 1).unwrap();
    bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).unwrap();
    if styled {
        lock_style(&project, &Minimax::new(), "film_noir");
    }
    project
}

fn provider() -> ImageProvider {
    ImageProvider::Minimax(Some(MinimaxClient::shared(
        SecretString::new("sk-test".to_owned()).unwrap(),
    )))
}

#[test]
fn a_character_is_cast_from_a_selected_portrait_and_its_reference_pack() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    assert_eq!(
        cast::main_characters(&project).unwrap(),
        ["filmmaker", "interviewer"]
    );
    let minimax = Minimax::new();

    let portraits = cast::audition_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        3,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(minimax.calls(), 3);
    assert_eq!(
        portraits
            .iter()
            .map(|image| image.artifact_id.as_str())
            .collect::<Vec<_>>(),
        [
            "cast.filmmaker.0001",
            "cast.filmmaker.0002",
            "cast.filmmaker.0003"
        ]
    );
    for body in minimax.bodies.lock().unwrap().iter() {
        assert_eq!(body["aspect_ratio"], "1:1");
        assert!(body.get("subject_reference").is_none());
        let prompt = body["prompt"].as_str().unwrap();
        assert!(prompt.starts_with("Black-and-white film noir still"));
        assert!(prompt.contains("Close-up casting portrait of The Filmmaker"));
    }
    let depends_on: Vec<_> = portraits[0]
        .dependencies
        .iter()
        .map(|dependency| dependency.artifact_id.as_str())
        .collect();
    assert_eq!(depends_on, ["bible_character.filmmaker", "style_s0001"]);

    assert!(matches!(
        cast::make_pack_with(
            &project,
            minimax.adapter(&project),
            "filmmaker",
            Submission::Once,
            &mut NoHooks
        ),
        Err(GenerationError::Prerequisite(
            Prerequisite::SelectedPortrait
        ))
    ));
    document::select(&project, "filmmaker", "cast.filmmaker.0002").unwrap();
    document::reject(&project, "filmmaker", "cast.filmmaker.0003").unwrap();
    let pack = cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(minimax.calls(), 10);
    assert_eq!(pack.len(), 7);
    let portrait_bytes = fs::read(target.join(&portraits[1].media_path)).unwrap();
    let identity = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&portrait_bytes)
    );
    for (body, image) in minimax.bodies.lock().unwrap()[3..].iter().zip(&pack) {
        assert_eq!(body["subject_reference"][0]["type"], "character");
        assert_eq!(
            body["subject_reference"][0]["image_file"],
            identity.as_str()
        );
        let tall = matches!(image.kind, CastImageKind::FullBody | CastImageKind::Costume);
        assert_eq!(body["aspect_ratio"], if tall { "9:16" } else { "1:1" });
        assert_eq!(
            image.identity_reference.as_deref(),
            Some("cast.filmmaker.0002")
        );
        assert!(image.media_path.starts_with("characters/filmmaker/refs/"));
        assert!(
            image
                .provenance
                .applied_constraints
                .contains(&"identity.filmmaker".to_owned())
        );
    }

    let locked = cast::lock(&project, &provider(), "filmmaker").unwrap();
    assert_eq!(locked.portrait.artifact_id, "cast.filmmaker.0002");
    assert_eq!(locked.references.len(), 7);
    assert_eq!(locked.provider_reference.provider, "minimax");
    assert_eq!(locked.provider_reference.mechanism, "subject_reference");
    assert_eq!(locked.provider_reference.artifact_id, "cast.filmmaker.0002");
    assert_eq!(
        locked.forbidden_changes,
        ["Grey at the temples.", "Never without the coat."]
    );
    assert!(cast::staleness(&project, "filmmaker").unwrap().is_empty());
    drop(project);

    fs::remove_dir_all(target.join(".kineto")).unwrap();
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(
        document::locked(&project, "filmmaker").unwrap(),
        Some(locked.clone())
    );
    for image in std::iter::once(&locked.portrait).chain(&locked.references) {
        let bytes = fs::read(target.join(&image.media_path)).unwrap();
        assert_eq!(ContentHash::of(&bytes).as_str(), image.content_hash);
    }
}

#[test]
fn the_cast_goes_stale_when_its_entry_or_the_style_changes_and_only_then() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"), true);
    let minimax = Minimax::new();
    cast::audition_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    document::select(&project, "filmmaker", "cast.filmmaker.0001").unwrap();
    cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    cast::lock(&project, &provider(), "filmmaker").unwrap();

    // Another character's entry changes: still fresh.
    let record = revisions::latest(&project, DocumentKind::Bible)
        .unwrap()
        .unwrap();
    let mut edited: kineto_project::bible::Bible = revisions::read(&project, &record).unwrap();
    edited.characters[1].wardrobe = "A raincoat.".to_owned();
    kineto_project::bible::edit(&project, record.revision, &edited).unwrap();
    assert!(cast::staleness(&project, "filmmaker").unwrap().is_empty());

    edited.characters[0].appearance = "A man of sixty, white-haired.".to_owned();
    kineto_project::bible::edit(&project, record.revision + 1, &edited).unwrap();
    assert_eq!(
        cast::staleness(&project, "filmmaker").unwrap(),
        ["bible_character.filmmaker"]
    );

    lock_style(&project, &minimax, "film_realism");
    assert_eq!(
        cast::staleness(&project, "filmmaker").unwrap(),
        ["bible_character.filmmaker", "style_s0001", "style_s0002"]
    );
}

#[test]
fn nothing_is_asked_or_written_without_what_casting_needs() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, false);
    let minimax = Minimax::new();
    let audition = |character| {
        cast::audition_with(
            &project,
            minimax.adapter(&project),
            character,
            3,
            Submission::Once,
            &mut NoHooks,
        )
    };
    assert!(matches!(
        audition("filmmaker"),
        Err(GenerationError::Prerequisite(Prerequisite::LockedStyle))
    ));
    lock_style(&project, &Minimax::new(), "film_noir");
    assert!(matches!(
        audition("waiter"),
        Err(GenerationError::Prerequisite(Prerequisite::Character))
    ));
    assert!(matches!(
        cast::audition(
            &project,
            &ImageProvider::LocalDemo,
            "filmmaker",
            3,
            Submission::Once,
            &mut NoHooks
        ),
        Err(GenerationError::Prerequisite(
            Prerequisite::RealImageProvider
        ))
    ));
    assert!(matches!(
        cast::audition(
            &project,
            &ImageProvider::Minimax(None),
            "filmmaker",
            3,
            Submission::Once,
            &mut NoHooks
        ),
        Err(GenerationError::NotConfigured)
    ));
    assert_eq!(minimax.calls(), 0);
    assert!(!target.join("characters").exists());

    // What the director is shown before paying.
    assert_eq!(
        cast::audition_cost(&provider(), 3),
        Some(Price::usd(10_500))
    );
    assert_eq!(cast::pack_cost(&provider()), Some((7, Price::usd(24_500))));
    assert_eq!(cast::pack_cost(&ImageProvider::LocalDemo), None);
}

/// The child half. A no-op unless the parent asked for a crash.
#[test]
fn crash_child() {
    let Ok(path) = env::var(CHILD) else {
        return;
    };
    let project = CanonicalProject::open(path).expect("child opens the project");
    let minimax = Minimax::new();
    let abort = || std::process::abort();
    let _ = cast::make_pack_with(
        &project,
        minimax.adapter(&project).when_answered(&abort),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    );
    panic!("the child must abort once MiniMax answers");
}

#[test]
fn killed_as_minimax_answered_a_pack_image_the_pack_waits_for_the_director() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::new();
    cast::audition_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    document::select(&project, "filmmaker", "cast.filmmaker.0001").unwrap();
    drop(project);

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

    let project = CanonicalProject::open(&target).unwrap();
    assert!(cast::recover(&project).unwrap().unresolved);
    assert_eq!(document::images(&project, "filmmaker").unwrap().len(), 1);
    let minimax = Minimax::new();
    let make = |submission| {
        cast::make_pack_with(
            &project,
            minimax.adapter(&project),
            "filmmaker",
            submission,
            &mut NoHooks,
        )
    };
    assert!(matches!(
        make(Submission::Once),
        Err(GenerationError::UnresolvedSubmission)
    ));
    // Nor is a portrait paid for while it waits.
    assert!(matches!(
        cast::audition_with(
            &project,
            minimax.adapter(&project),
            "filmmaker",
            1,
            Submission::Once,
            &mut NoHooks,
        ),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert_eq!(minimax.calls(), 0, "never paid twice unasked");

    let pack = make(Submission::RiskPayingTwice).unwrap();
    assert_eq!(pack.len(), 7);
    assert_eq!(pack[0].artifact_id, "cast.filmmaker.0002");
    assert_eq!(minimax.calls(), 7);
}
