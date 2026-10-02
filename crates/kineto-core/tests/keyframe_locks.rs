//! Keyframes that follow what production has locked, with MiniMax offline:
//! the locked style leads the prompt, the shot's cast character is shown
//! from their canonical portrait sent as identity reference, the candidates
//! depend on both locks and go stale when either changes, and a project
//! that blocks identity drift waits for the director's consent.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use kineto_core::{
    bible, bootstrap, cast,
    generation::{self, Consent, GenerationError, NoHooks},
    minimax::MinimaxKeyframes,
    screenplay, story,
    style::{self, SampleRequest, Submission},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    screenplay::SAMPLE_SCREENPLAY_JSON,
    shot::ShotWorkflow,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document,
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::secret::SecretString;

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
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x4b, u8::try_from(call).unwrap()];
        Ok(HttpResponse {
            status: 200,
            body: format!(
                r#"{{"id":"k{call}","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                base64::engine::general_purpose::STANDARD.encode(jpeg)
            )
            .into_bytes(),
        })
    }

    fn get(&self, url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        unreachable!("keyframes make no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("keyframes download nothing: {url}")
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
        let path = env::temp_dir().join(format!(
            "kineto-keyframe-locks-{}-{nonce}",
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
        project_id: "keyframe_locks".to_owned(),
        title: "Keyframe Locks".to_owned(),
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

/// The bootstrap film, whose shots show the filmmaker, with a locked
/// screenplay, a bible and a locked noir style; the filmmaker cast when
/// `cast_filmmaker`.
fn film(target: &Path, cast_filmmaker: bool) -> CanonicalProject {
    let project = CanonicalProject::create_with_files(
        target,
        manifest(),
        b"A filmmaker hears the last line.\n",
        &bootstrap::scene_files(),
    )
    .unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    kineto_project::screenplay::lock(&project, 1).unwrap();
    bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).unwrap();
    let minimax = Minimax::new();
    lock_style(&project, &minimax, "film_noir");
    if cast_filmmaker {
        cast::audition_with(
            &project,
            minimax.adapter(&project),
            "filmmaker",
            1,
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap();
        cast_document::select(&project, "filmmaker", "cast.filmmaker.0001").unwrap();
        cast::make_pack_with(
            &project,
            minimax.adapter(&project),
            "filmmaker",
            Submission::Once,
            &mut NoHooks,
        )
        .unwrap();
        let provider = kineto_core::generation::ImageProvider::Minimax(Some(
            MinimaxClient::shared(SecretString::new("sk-test".to_owned()).unwrap()),
        ));
        cast::lock(&project, &provider, "filmmaker").unwrap();
    }
    project
}

/// Reopen the project with `identity_policy` set under `[workflow]`.
fn with_policy(target: &Path, project: CanonicalProject, policy: &str) -> CanonicalProject {
    drop(project);
    let path = target.join("project.toml");
    let text = fs::read_to_string(&path).unwrap().replace(
        "recipe_version = 1\n",
        &format!("recipe_version = 1\nidentity_policy = \"{policy}\"\n"),
    );
    fs::write(&path, text).unwrap();
    CanonicalProject::open(target).unwrap()
}

#[test]
fn a_keyframe_shows_the_cast_from_its_portrait_in_the_locked_style() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::new();
    let mut shot = ShotWorkflow::load(&project, 1).unwrap();
    generation::generate_with(&project, &mut shot, minimax.adapter(&project), &mut NoHooks)
        .unwrap();
    assert_eq!(minimax.calls(), 3);

    let portrait = cast_document::locked(&project, "filmmaker")
        .unwrap()
        .unwrap()
        .portrait;
    let identity = format!(
        "data:image/jpeg;base64,{}",
        base64::engine::general_purpose::STANDARD
            .encode(fs::read(target.join(&portrait.media_path)).unwrap())
    );
    for body in minimax.bodies.lock().unwrap().iter() {
        assert_eq!(
            body["subject_reference"][0]["image_file"],
            identity.as_str()
        );
        let prompt = body["prompt"].as_str().unwrap();
        assert!(prompt.starts_with("Black-and-white film noir still"));
        assert!(prompt.contains("The Filmmaker is the person in the reference image."));
        assert!(prompt.contains("The Filmmaker, always: Grey at the temples"));
    }

    let dependencies = shot.candidate_dependencies().unwrap();
    assert_eq!(dependencies.len(), 3);
    assert_eq!(
        dependencies[0]
            .iter()
            .map(|dependency| dependency.artifact_id.as_str())
            .collect::<Vec<_>>(),
        ["style_s0001", "cast.filmmaker.0001"]
    );
    assert!(
        generation::keyframe_staleness(&project, &shot)
            .unwrap()
            .iter()
            .all(Vec::is_empty)
    );

    // Another style is locked: every candidate is stale, and kept.
    lock_style(&project, &minimax, "film_realism");
    let shot = ShotWorkflow::load(&project, 1).unwrap();
    let stale = generation::keyframe_staleness(&project, &shot).unwrap();
    assert_eq!(stale.len(), 3);
    assert!(
        stale
            .iter()
            .all(|causes| causes == &["style_s0001".to_owned(), "style_s0002".to_owned()])
    );
    assert_eq!(shot.candidate_media().unwrap().len(), 3);
}

#[test]
fn a_project_that_blocks_identity_drift_waits_for_consent() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = with_policy(&target, film(&target, false), "block");
    assert_eq!(
        generation::identity_policy(&project),
        generation::IdentityPolicy::Block
    );
    let minimax = Minimax::new();
    let mut shot = ShotWorkflow::load(&project, 1).unwrap();
    let Err(GenerationError::IdentityDrift(lost)) =
        generation::generate_with(&project, &mut shot, minimax.adapter(&project), &mut NoHooks)
    else {
        panic!("an uncast character must wait for consent");
    };
    assert_eq!(lost, ["identity.filmmaker"]);
    assert_eq!(minimax.calls(), 0);
    assert!(
        !target
            .join(".kineto/jobs/intents/shot_001_g0001_candidate_a.json")
            .exists()
    );
    assert!(!shot.snapshot().unwrap().generated);

    generation::generate_consenting(
        &project,
        &mut shot,
        minimax.adapter(&project),
        &mut NoHooks,
        Consent {
            identity_drift: true,
            ..Consent::default()
        },
    )
    .unwrap();
    assert_eq!(minimax.calls(), 3);
    assert!(
        minimax.bodies.lock().unwrap()[0]
            .get("subject_reference")
            .is_none()
    );
}

#[test]
fn a_portrait_changed_on_disk_is_not_sent() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let portrait = cast_document::locked(&project, "filmmaker")
        .unwrap()
        .unwrap()
        .portrait;
    fs::write(target.join(&portrait.media_path), b"\xff\xd8\xff\xe0edited").unwrap();
    let minimax = Minimax::new();
    let mut shot = ShotWorkflow::load(&project, 1).unwrap();
    assert!(
        generation::generate_with(&project, &mut shot, minimax.adapter(&project), &mut NoHooks)
            .is_err()
    );
    assert_eq!(minimax.calls(), 0);
    assert!(!shot.snapshot().unwrap().generated);
}
