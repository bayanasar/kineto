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
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use kineto_core::{
    bible, cast,
    generation::{
        GenerationError, GenerationHooks, GenerationPoint, ImageProvider, NoHooks, Prerequisite,
    },
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
/// request body kept. Each call takes `pause`, and the most calls ever in
/// flight at once are counted.
#[derive(Default)]
struct FakeMinimax {
    calls: Arc<AtomicU32>,
    bodies: Arc<Mutex<Vec<serde_json::Value>>>,
    pause: Duration,
    in_flight: Arc<AtomicU32>,
    most: Arc<AtomicU32>,
}

impl Transport for FakeMinimax {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.most.fetch_max(now, Ordering::SeqCst);
        std::thread::sleep(self.pause);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
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
    most: Arc<AtomicU32>,
}

impl Minimax {
    fn new() -> Self {
        Self::taking(Duration::ZERO)
    }

    /// A MiniMax whose every call takes `pause`.
    fn taking(pause: Duration) -> Self {
        let fake = FakeMinimax {
            pause,
            ..FakeMinimax::default()
        };
        let calls = Arc::clone(&fake.calls);
        let bodies = Arc::clone(&fake.bodies);
        let most = Arc::clone(&fake.most);
        Self {
            client: MinimaxClient::with_transport(
                fake,
                SecretString::new("sk-test".to_owned()).unwrap(),
                "https://example.test",
            ),
            calls,
            bodies,
            most,
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
    // The pack's calls run at once, so they reach MiniMax in any order.
    let bodies = minimax.bodies.lock().unwrap()[3..].to_vec();
    for body in &bodies {
        assert_eq!(body["subject_reference"][0]["type"], "character");
        assert_eq!(
            body["subject_reference"][0]["image_file"],
            identity.as_str()
        );
    }
    let tall = |image: &&document::CastImage| {
        matches!(image.kind, CastImageKind::FullBody | CastImageKind::Costume)
    };
    assert_eq!(
        bodies
            .iter()
            .filter(|body| body["aspect_ratio"] == "9:16")
            .count(),
        pack.iter().filter(tall).count()
    );
    for image in &pack {
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
fn an_audition_and_a_pack_ask_for_their_images_at_once() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::taking(Duration::from_millis(400));
    let portraits = cast::audition_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        3,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(portraits.len(), 3);
    assert!(
        minimax.most.load(Ordering::SeqCst) >= 2,
        "a portrait does not wait for the one before"
    );

    document::select(&project, "filmmaker", &portraits[0].artifact_id).unwrap();
    let minimax = Minimax::taking(Duration::from_millis(100));
    let pack = cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(pack.len(), 7);
    let most = minimax.most.load(Ordering::SeqCst);
    assert!(
        (2..=3).contains(&most),
        "at once, within MiniMax's limit: {most}"
    );
}

#[test]
fn every_candidate_of_an_audition_is_another_actor() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::new();
    let audition = |count| {
        cast::audition_with(
            &project,
            minimax.adapter(&project),
            "filmmaker",
            count,
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap()
    };
    let looks = |portraits: &[document::CastImage]| {
        portraits
            .iter()
            .map(|image| image.look.clone().unwrap())
            .collect::<Vec<_>>()
    };

    let portraits = audition(3);
    assert_eq!(looks(&portraits), ["hooded", "wide-set", "downturned"]);
    let prompts: Vec<String> = minimax
        .bodies
        .lock()
        .unwrap()
        .iter()
        .map(|body| body["prompt"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(prompts.len(), 3);
    // The calls run at once, so they arrive in any order.
    for look in &cast::LOOKS[..3] {
        let asked = format!("Played by an actor with {}.", look.features);
        assert_eq!(
            prompts
                .iter()
                .filter(|prompt| prompt.contains(&asked))
                .count(),
            1
        );
    }
    assert!(prompts[0] != prompts[1] && prompts[1] != prompts[2] && prompts[0] != prompts[2]);
    let hashes: std::collections::BTreeSet<_> = portraits
        .iter()
        .map(|image| image.input_hash.as_str())
        .collect();
    assert_eq!(hashes.len(), 3, "each is its own request");

    // A rejected face is not asked for again, and neither is a kept one.
    document::reject(&project, "filmmaker", "cast.filmmaker.0002").unwrap();
    let portraits = audition(3);
    assert_eq!(looks(&portraits), ["hooded", "downturned", "almond"]);
    let portraits = audition(4);
    assert_eq!(
        looks(&portraits),
        ["hooded", "downturned", "almond", "heavy-lidded"]
    );
    assert_eq!(minimax.calls(), 5);
}

#[test]
fn an_imported_portrait_is_cast_like_a_generated_one() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::new();
    let capabilities = provider().capabilities();
    // The director's own photo of the actor, a JPEG.
    let photo = [
        0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0x42,
    ];

    // Not an image, or too large for MiniMax's identity reference: refused,
    // nothing written.
    for refused in [b"not an image".to_vec(), {
        let mut big = photo.to_vec();
        big.resize(11 * 1024 * 1024, 0);
        big
    }] {
        assert!(matches!(
            cast::import_portrait(&project, &capabilities, "filmmaker", &refused),
            Err(GenerationError::Prerequisite(
                Prerequisite::IdentityReference
            ))
        ));
    }
    assert!(document::images(&project, "filmmaker").unwrap().is_empty());

    let imported = cast::import_portrait(&project, &capabilities, "filmmaker", &photo).unwrap();
    assert_eq!(imported.artifact_id, "cast.filmmaker.0001");
    assert_eq!(
        (imported.provider.as_str(), imported.kind),
        ("import", CastImageKind::Portrait)
    );
    assert_eq!(imported.status, Some(document::PortraitStatus::Candidate));
    assert_eq!(fs::read(target.join(&imported.media_path)).unwrap(), photo);
    assert_eq!(imported.content_hash, ContentHash::of(&photo).as_str());
    let depends_on: Vec<_> = imported
        .dependencies
        .iter()
        .map(|dependency| dependency.artifact_id.as_str())
        .collect();
    assert_eq!(depends_on, ["bible_character.filmmaker", "style_s0001"]);
    // The record keeps to the casting schema.
    let schema: serde_json::Value =
        serde_json::from_str(include_str!("../../../schemas/casting.schema.json")).unwrap();
    let log: serde_json::Value = serde_json::from_slice(
        &fs::read(target.join("characters/filmmaker/casting.json")).unwrap(),
    )
    .unwrap();
    kineto_json_schema::validate(&schema, &log).unwrap();
    // The same photo again records nothing new.
    assert_eq!(
        cast::import_portrait(&project, &capabilities, "filmmaker", &photo).unwrap(),
        imported
    );
    assert_eq!(document::images(&project, "filmmaker").unwrap().len(), 1);
    assert_eq!(minimax.calls(), 0, "importing is free");

    // Selected, its pack made from it, and locked: the pack shows the photo.
    document::select(&project, "filmmaker", "cast.filmmaker.0001").unwrap();
    cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    let identity = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(photo)
    );
    for body in minimax.bodies.lock().unwrap().iter() {
        assert_eq!(
            body["subject_reference"][0]["image_file"],
            identity.as_str()
        );
    }
    let locked = cast::lock(&project, &provider(), "filmmaker").unwrap();
    assert_eq!(locked.portrait.artifact_id, "cast.filmmaker.0001");
    assert!(cast::staleness(&project, "filmmaker").unwrap().is_empty());
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
///
/// `KINETO_CASTING_CRASH` is `<action>-<point>-<n>:<project>`: `action` is
/// `pack` or `audition` (of three portraits); the child dies as MiniMax
/// answers its `n`th call (`answered`), or as the intent of its `n`th image
/// is written (`prepared`, counted from zero).
#[test]
fn crash_child() {
    let Ok(value) = env::var(CHILD) else {
        return;
    };
    let (mode, path) = value.split_once(':').unwrap();
    let mut parts = mode.split('-');
    let (action, point, n) = (
        parts.next().unwrap(),
        parts.next().unwrap(),
        parts.next().unwrap().parse::<u32>().unwrap(),
    );
    let project = CanonicalProject::open(path).expect("child opens the project");
    let minimax = Minimax::new();
    let answered = AtomicU32::new(0);
    let abort_on_answer = || {
        if answered.fetch_add(1, Ordering::SeqCst) + 1 == n {
            std::process::abort();
        }
    };
    let mut adapter = minimax.adapter(&project);
    if point == "answered" {
        adapter = adapter.when_answered(&abort_on_answer);
    }
    let mut hooks = AbortAt(match point {
        "prepared" => GenerationPoint::Prepared {
            index: usize::try_from(n).unwrap(),
        },
        "attached" => GenerationPoint::Attached,
        _ => GenerationPoint::Prepared { index: usize::MAX },
    });
    let _ = match action {
        "pack" => {
            cast::make_pack_with(&project, adapter, "filmmaker", Submission::Once, &mut hooks)
        }
        _ => cast::audition_with(
            &project,
            adapter,
            "filmmaker",
            3,
            Submission::Once,
            &mut hooks,
        ),
    };
    panic!("the child must abort at {mode}");
}

/// Abort the process at one point of an image.
struct AbortAt(GenerationPoint);

impl GenerationHooks for AbortAt {
    fn reached(&mut self, point: GenerationPoint) {
        if point == self.0 {
            std::process::abort();
        }
    }
}

/// Run the child in `mode` on `target`; it must die by SIGABRT.
fn crash(temp: &TempDir, target: &Path, mode: &str) {
    let status = Command::new(env::current_exe().unwrap())
        .args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, format!("{mode}:{}", target.display()))
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

/// A styled film whose filmmaker has one selected portrait.
fn selected(target: &Path) -> CanonicalProject {
    let project = film(target, true);
    cast::audition_with(
        &project,
        Minimax::new().adapter(&project),
        "filmmaker",
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    document::select(&project, "filmmaker", "cast.filmmaker.0001").unwrap();
    project
}

fn references(project: &CanonicalProject) -> Vec<CastImageKind> {
    document::images(project, "filmmaker")
        .unwrap()
        .into_iter()
        .filter(|image| image.kind != CastImageKind::Portrait)
        .map(|image| image.kind)
        .collect()
}

#[test]
fn a_pack_cut_short_before_its_calls_pays_for_each_image_once() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(selected(&target));
    // Every image of the pack is prepared before the first call, and the
    // process dies before any is called.
    crash(&temp, &target, "pack-prepared-3");
    let project = CanonicalProject::open(&target).unwrap();
    assert!(references(&project).is_empty());

    let minimax = Minimax::new();
    let pack = cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(minimax.calls(), 7, "none was called before");
    assert_eq!(pack.len(), 7);
    let mut made = references(&project);
    assert_eq!(made.len(), 7, "no kind made twice");
    made.sort();
    made.dedup();
    assert_eq!(made.len(), 7);
}

#[test]
fn a_pack_cut_short_after_its_calls_keeps_every_paid_image() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(selected(&target));
    // Every call was answered and kept; the process dies as the first
    // image is recorded.
    crash(&temp, &target, "pack-attached-1");
    let project = CanonicalProject::open(&target).unwrap();
    assert_eq!(cast::recover(&project).unwrap().unresolved.len(), 0);
    assert_eq!(
        references(&project).len(),
        7,
        "recorded from their receipts"
    );
    let minimax = Minimax::new();
    let pack = cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(pack.len(), 7);
    assert_eq!(minimax.calls(), 0, "nothing is paid for again");
}

#[test]
fn a_pack_cut_short_by_an_unknown_answer_pays_again_only_for_what_it_lacks() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(selected(&target));
    // MiniMax answers the fourth call and the process dies before it is
    // kept. The calls run at once, so which others had answered and been
    // kept by then varies; none of them is paid for again.
    crash(&temp, &target, "pack-answered-4");
    let project = CanonicalProject::open(&target).unwrap();
    assert!(!cast::recover(&project).unwrap().unresolved.is_empty());
    let kept = references(&project).len();
    assert!(kept < 7);
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
    assert_eq!(minimax.calls(), 0);
    make(Submission::RiskPayingTwice).unwrap();
    // Only what the pack lacks, the lost answer among it, resubmitted on
    // the director's decision.
    assert_eq!(minimax.calls(), u32::try_from(7 - kept).unwrap());
    let mut made = references(&project);
    assert_eq!(made.len(), 7);
    made.sort();
    made.dedup();
    assert_eq!(made.len(), 7, "no kind made twice");
}

#[test]
fn an_audition_cut_short_makes_only_the_portraits_it_lacked() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(film(&target, true));
    // Every portrait is prepared before the first call; the process dies
    // as the last is, before any is called.
    crash(&temp, &target, "audition-prepared-2");
    let project = CanonicalProject::open(&target).unwrap();
    assert!(document::images(&project, "filmmaker").unwrap().is_empty());
    let minimax = Minimax::new();
    let audition = |count| {
        cast::audition_with(
            &project,
            minimax.adapter(&project),
            "filmmaker",
            count,
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap()
    };
    assert_eq!(audition(3).len(), 3);
    assert_eq!(minimax.calls(), 3);
    // Asking again for what is there pays nothing; a rejected portrait is
    // replaced, and more are made only when more are asked for.
    assert_eq!(audition(3).len(), 3);
    assert_eq!(minimax.calls(), 3);
    document::reject(&project, "filmmaker", "cast.filmmaker.0001").unwrap();
    assert_eq!(audition(3).len(), 3);
    assert_eq!(minimax.calls(), 4);
    assert_eq!(audition(5).len(), 5);
    assert_eq!(minimax.calls(), 6);
}

#[test]
fn an_audition_cut_short_after_its_calls_keeps_every_paid_portrait() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(film(&target, true));
    crash(&temp, &target, "audition-attached-1");
    let project = CanonicalProject::open(&target).unwrap();
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
    assert_eq!(portraits.len(), 3, "recorded from their receipts");
    assert_eq!(minimax.calls(), 0, "nothing is paid for again");
}

#[test]
fn a_complete_pack_is_made_again_only_where_the_director_asks() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = selected(&target);
    let minimax = Minimax::new();
    let first = cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(minimax.calls(), 7);
    let again = cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(again, first);
    assert_eq!(minimax.calls(), 7, "a complete pack costs nothing");

    let remade = cast::remake_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        &[CastImageKind::Front],
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(minimax.calls(), 8);
    assert_ne!(remade[0].artifact_id, first[0].artifact_id);
    assert_eq!(remade[1..], first[1..]);
    let locked = cast::lock(&project, &provider(), "filmmaker").unwrap();
    assert_eq!(locked.references[0].artifact_id, remade[0].artifact_id);
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

    crash(&temp, &target, "pack-answered-1");

    // The pack's calls run at once: the first answer is lost, and whatever
    // else was sent and not kept by then is unresolved with it.
    let project = CanonicalProject::open(&target).unwrap();
    let unresolved = cast::recover(&project).unwrap().unresolved;
    assert!(!unresolved.is_empty());
    assert!(
        unresolved
            .iter()
            .all(|id| id.starts_with("cast.filmmaker."))
    );
    let kept = references(&project).len();
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
    assert_eq!(minimax.calls(), u32::try_from(7 - kept).unwrap());
    assert!(cast::recover(&project).unwrap().unresolved.is_empty());
}

#[test]
fn risking_paying_twice_for_one_character_leaves_anothers_unresolved_image_guarded() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(selected(&target));
    crash(&temp, &target, "pack-answered-1");

    let project = CanonicalProject::open(&target).unwrap();
    let minimax = Minimax::new();
    let audition = |character, submission| {
        cast::audition_with(
            &project,
            minimax.adapter(&project),
            character,
            1,
            submission,
            &mut NoHooks,
        )
    };
    // The filmmaker's unresolved image is not the interviewer's to answer
    // for: it neither stops the interviewer's casting nor is retired by it.
    audition("interviewer", Submission::Once).unwrap();
    let portraits = audition("interviewer", Submission::RiskPayingTwice).unwrap();
    assert_eq!(portraits[0].artifact_id, "cast.interviewer.0001");
    assert_eq!(minimax.calls(), 1);
    let unresolved = cast::recover(&project).unwrap().unresolved;
    assert!(!unresolved.is_empty());
    assert!(
        unresolved
            .iter()
            .all(|id| id.starts_with("cast.filmmaker."))
    );
    let kept = references(&project).len();
    assert!(matches!(
        audition("filmmaker", Submission::Once),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert_eq!(minimax.calls(), 1, "never paid twice unasked");

    // Only the director's decision for the filmmaker retires it.
    let pack = cast::make_pack_with(
        &project,
        minimax.adapter(&project),
        "filmmaker",
        Submission::RiskPayingTwice,
        &mut NoHooks,
    )
    .unwrap();
    assert_eq!(pack.len(), 7);
    assert_eq!(minimax.calls(), 1 + u32::try_from(7 - kept).unwrap());
    assert!(cast::recover(&project).unwrap().unresolved.is_empty());
}
