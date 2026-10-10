//! Keyframes that follow what production has locked, with MiniMax offline:
//! the locked style leads the prompt, the shot's cast character is shown
//! from their canonical portrait sent as identity reference, the candidates
//! depend on both locks and go stale when either changes, a stale one is
//! not locked, a project that blocks identity drift waits for the
//! director's consent, and a shot that continues from another is sent that
//! shot's locked keyframe where the provider takes a composition reference.

use kineto_project::shot::ShotKey;
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
    generation::{
        self, CandidateGenerator, CandidateOwner, CandidateRequest, Consent, GeneratedCandidate,
        GenerationError, KEYFRAME_ASPECT, LocalDemoAdapter, LocalDemoError, NoHooks,
        ReferenceImage,
    },
    minimax::MinimaxKeyframes,
    screenplay, story,
    style::{self, SampleRequest, Submission},
};
use kineto_job_runtime::{
    Invocation, JobRuntimePolicy, PaidJobAdapter, ProviderKey, Reconciliation,
};
use kineto_jobs::{CostEstimate, ErrorClass, IdempotencyKey, ProviderJobId};
use kineto_llm::fake::FakeLlm;
use kineto_project::{ArtifactId, InputHash};
use kineto_project::{
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    screenplay::SAMPLE_SCREENPLAY_JSON,
    shot::{ShotWorkflow, ShotWorkflowError},
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document,
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::{ImageCapabilities, ReferenceRole, secret::SecretString};

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
        cast_character(&project, &minimax, "filmmaker");
    }
    project
}

/// Cast `character` from one portrait and its pack, and lock it.
fn cast_character(project: &CanonicalProject, minimax: &Minimax, character: &str) {
    let portraits = cast::audition_with(
        project,
        minimax.adapter(project),
        character,
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    cast_document::select(project, character, &portraits[0].artifact_id).unwrap();
    let provider = kineto_core::generation::ImageProvider::Minimax(Some(MinimaxClient::shared(
        SecretString::new("sk-test".to_owned()).unwrap(),
    )));
    cast::lock(project, &provider, character).unwrap();
}

/// The demo generator, declaring it takes two references, of identity and,
/// when `composition`, of composition too, and keeping every request it is
/// asked to invoke. With `composition` its candidates are its own and record
/// what their prompt depends on, as a provider's do.
#[derive(Clone, Default)]
struct TwoFaces {
    seen: Arc<Mutex<Vec<CandidateRequest>>>,
    composition: bool,
}

impl PaidJobAdapter for TwoFaces {
    type Request = CandidateRequest;
    type Output = GeneratedCandidate;
    type Error = LocalDemoError;

    fn estimate_cost(&self, request: &CandidateRequest) -> Result<CostEstimate, LocalDemoError> {
        LocalDemoAdapter.estimate_cost(request)
    }

    fn invoke(
        &mut self,
        request: &CandidateRequest,
        key: &IdempotencyKey,
    ) -> Result<Invocation<GeneratedCandidate>, LocalDemoError> {
        self.seen.lock().unwrap().push(request.clone());
        let invocation = LocalDemoAdapter.invoke(request, key)?;
        Ok(match invocation {
            Invocation::Completed(mut candidate) if self.composition => {
                candidate.output.provider = self.provider().as_str().to_owned();
                candidate.output.dependencies = request
                    .prompt
                    .as_ref()
                    .map(|prompt| prompt.dependencies.clone())
                    .unwrap_or_default();
                Invocation::Completed(candidate)
            }
            other => other,
        })
    }

    fn reconcile(
        &mut self,
        id: &ProviderJobId,
    ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
        LocalDemoAdapter.reconcile(id)
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        key: &IdempotencyKey,
    ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
        LocalDemoAdapter.reconcile_by_idempotency_key(key)
    }

    fn classify_error(&self, error: &LocalDemoError) -> ErrorClass {
        LocalDemoAdapter.classify_error(error)
    }
}

impl CandidateGenerator for TwoFaces {
    fn provider(&self) -> ProviderKey {
        ProviderKey::new("fake-two-faces").unwrap()
    }

    fn policy(&self) -> JobRuntimePolicy {
        LocalDemoAdapter.policy()
    }

    fn needs_prompt(&self) -> bool {
        true
    }

    fn capabilities(&self) -> ImageCapabilities {
        let mut roles: std::collections::BTreeSet<_> = [ReferenceRole::Identity].into();
        if self.composition {
            roles.insert(ReferenceRole::Composition);
        }
        ImageCapabilities {
            max_reference_images: 2,
            reference_roles: roles,
            ..generation::local_demo_capabilities()
        }
    }

    fn recover(
        &mut self,
        request: &CandidateRequest,
    ) -> Result<GeneratedCandidate, LocalDemoError> {
        LocalDemoAdapter.recover(request)
    }
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
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
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
    // Candidates made from locks are fresh, so the director can lock one.
    assert!(!shot.snapshot().unwrap().stale);
    shot.select(&project, 1).unwrap();
    generation::lock(&project, &mut shot).unwrap();
    assert!(shot.snapshot().unwrap().locked);

    // Another style is locked: every candidate is stale, and kept.
    lock_style(&project, &minimax, "film_realism");
    let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
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
fn a_keyframe_made_before_its_character_was_cast_is_not_locked() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, false);
    let minimax = Minimax::new();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    generation::generate_with(&project, &mut shot, minimax.adapter(&project), &mut NoHooks)
        .unwrap();
    shot.select(&project, 0).unwrap();

    cast_character(&project, &minimax, "filmmaker");
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    assert_eq!(
        generation::keyframe_staleness(&project, &shot).unwrap()[0],
        ["cast.filmmaker.0001"]
    );
    // The shot's intent is unchanged, so only the locks tell it is stale.
    assert!(!shot.snapshot().unwrap().stale);
    assert!(matches!(
        generation::lock(&project, &mut shot),
        Err(GenerationError::Shot(ShotWorkflowError::StaleSelection))
    ));
    assert!(!shot.snapshot().unwrap().locked);

    generation::generate_with(&project, &mut shot, minimax.adapter(&project), &mut NoHooks)
        .unwrap();
    shot.select(&project, 0).unwrap();
    generation::lock(&project, &mut shot).unwrap();
    assert!(shot.snapshot().unwrap().locked);
}

#[test]
fn a_provider_that_takes_two_references_is_sent_the_first_two_cast_of_the_shot() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::new();
    cast_character(&project, &minimax, "interviewer");
    // Shot 1 now shows both cast characters and one who is not cast.
    let path = target.join("scenes/scene_001/shots/shot_001/shot.json");
    let mut intent: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    intent["characters"] = serde_json::json!(["filmmaker", "interviewer", "waiter"]);
    fs::write(&path, serde_json::to_vec_pretty(&intent).unwrap()).unwrap();

    let generator = TwoFaces::default();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    generation::generate_with(&project, &mut shot, generator.clone(), &mut NoHooks).unwrap();
    let seen = generator.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    let portrait = |character: &str| {
        cast_document::locked(&project, character)
            .unwrap()
            .unwrap()
            .portrait
    };
    for request in seen.iter() {
        let sent: Vec<_> = request
            .identities
            .iter()
            .map(|identity| identity.artifact_id.as_str())
            .collect();
        assert_eq!(
            sent,
            [
                portrait("filmmaker").artifact_id.as_str(),
                portrait("interviewer").artifact_id.as_str()
            ]
        );
        let interviewer = fs::read(target.join(portrait("interviewer").media_path)).unwrap();
        assert_eq!(&*request.identities[1].bytes, interviewer.as_slice());
        let prompt = request.prompt.as_ref().unwrap();
        let dropped: Vec<_> = prompt
            .compiled
            .dropped_constraints()
            .iter()
            .filter(|dropped| dropped.constraint.as_str().starts_with("identity."))
            .map(|dropped| dropped.constraint.as_str())
            .collect();
        assert_eq!(dropped, ["identity.waiter"]);
    }
}

#[test]
fn minimax_is_never_sent_more_identity_references_than_it_takes() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, false);
    let minimax = Minimax::new();
    let reference = ReferenceImage {
        artifact_id: ArtifactId::new("cast.filmmaker.0001").unwrap(),
        bytes: Arc::from(&[0xff_u8, 0xd8, 0xff, 0xe0][..]),
    };
    let request = CandidateRequest {
        artifact_id: ArtifactId::new("shot_001_g0001_candidate_a").unwrap(),
        input_hash: InputHash::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
        owner: CandidateOwner::Shot(ShotKey::bootstrap(1)),
        aspect_ratio: KEYFRAME_ASPECT,
        identities: vec![reference.clone(), reference],
        composition: None,
        style: None,
        prompt: Some(Arc::new(
            kineto_core::keyframe_prompt::compile(
                None,
                Some(
                    &ShotWorkflow::load(&project, &ShotKey::bootstrap(1))
                        .unwrap()
                        .intent()
                        .unwrap()
                        .unwrap(),
                ),
                kineto_project::shot::ShotDirection::Reaction,
                kineto_core::keyframe_prompt::Locks::default(),
                &kineto_provider_minimax::image_capabilities(),
            )
            .unwrap(),
        )),
    };
    let mut adapter = minimax.adapter(&project);
    let error = adapter
        .invoke(
            &request,
            &IdempotencyKey::new("image.generate:x:y").unwrap(),
        )
        .unwrap_err();
    assert!(adapter.certainly_not_charged(&error), "{error}");
    assert_eq!(minimax.calls(), 0);
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
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
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
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    assert!(
        generation::generate_with(&project, &mut shot, minimax.adapter(&project), &mut NoHooks)
            .is_err()
    );
    assert_eq!(minimax.calls(), 0);
    assert!(!shot.snapshot().unwrap().generated);
}

#[test]
fn a_shot_continuing_from_another_is_sent_its_locked_keyframe_as_composition() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target, true);
    let minimax = Minimax::new();
    // Shot 2 continues from shot 1, whose keyframe is locked.
    let path = target.join("scenes/scene_001/shots/shot_002/shot.json");
    let mut intent: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    intent["continuity_from"] = "shot_001".into();
    fs::write(&path, serde_json::to_vec_pretty(&intent).unwrap()).unwrap();
    let mut first = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    generation::generate_with(
        &project,
        &mut first,
        minimax.adapter(&project),
        &mut NoHooks,
    )
    .unwrap();
    first.select(&project, 2).unwrap();
    generation::lock(&project, &mut first).unwrap();
    let (keyframe, media) = first.first_frame().unwrap().unwrap();
    let frame = fs::read(target.join(media.unwrap().as_path())).unwrap();

    let generator = TwoFaces {
        composition: true,
        ..TwoFaces::default()
    };
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(2)).unwrap();
    generation::generate_with(&project, &mut shot, generator.clone(), &mut NoHooks).unwrap();
    let seen = generator.seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    for request in seen.iter() {
        let composition = request.composition.as_ref().unwrap();
        assert_eq!(composition.artifact_id, keyframe.artifact_id);
        assert_eq!(&*composition.bytes, frame.as_slice());
        // The cast's identity comes first, in the other reference.
        assert_eq!(request.identities.len(), 1);
        let prompt = request.prompt.as_ref().unwrap();
        assert!(
            prompt
                .compiled
                .prompt()
                .contains("The composition reference image is the keyframe of shot shot 001.")
        );
        assert!(
            prompt
                .compiled
                .dropped_constraints()
                .iter()
                .all(|dropped| dropped.constraint.as_str() != "continuity.previous_frame")
        );
    }
    // Every candidate depends on the frame, by its bytes, and is fresh.
    for recorded in shot.candidate_dependencies().unwrap() {
        let previous = recorded
            .iter()
            .find(|dependency| dependency.field.as_deref() == Some("continuity.previous_frame"))
            .unwrap();
        assert_eq!(
            (&previous.artifact_id, &previous.content_hash),
            (&keyframe.artifact_id, &keyframe.content_hash)
        );
    }
    assert!(
        generation::keyframe_staleness(&project, &shot)
            .unwrap()
            .iter()
            .all(Vec::is_empty)
    );
    // Continuing from nothing, they no longer follow the frame they were
    // sent, and say so.
    let continuing = fs::read(&path).unwrap();
    let mut intent: serde_json::Value = serde_json::from_slice(&continuing).unwrap();
    intent.as_object_mut().unwrap().remove("continuity_from");
    fs::write(&path, serde_json::to_vec_pretty(&intent).unwrap()).unwrap();
    let detached = ShotWorkflow::load(&project, &ShotKey::bootstrap(2)).unwrap();
    for causes in generation::keyframe_staleness(&project, &detached).unwrap() {
        assert!(
            causes.contains(&"intent.continuity_from".to_owned()),
            "{causes:?}"
        );
        assert!(
            causes.contains(&keyframe.artifact_id.as_str().to_owned()),
            "{causes:?}"
        );
    }
    fs::write(&path, continuing).unwrap();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(2)).unwrap();

    // MiniMax takes no composition reference: only the portrait is sent,
    // the drop says why, and its candidates do not follow the frame.
    generation::generate_with(&project, &mut shot, minimax.adapter(&project), &mut NoHooks)
        .unwrap();
    let bodies = minimax.bodies.lock().unwrap();
    let last = bodies.last().unwrap();
    assert_eq!(last["subject_reference"].as_array().unwrap().len(), 1);
    assert!(
        !last["prompt"]
            .as_str()
            .unwrap()
            .contains("composition reference")
    );
    for recorded in shot.candidate_dependencies().unwrap() {
        assert!(
            recorded
                .iter()
                .all(|dependency| dependency.field.as_deref() != Some("continuity.previous_frame"))
        );
    }
    assert!(
        generation::keyframe_staleness(&project, &shot)
            .unwrap()
            .iter()
            .all(Vec::is_empty)
    );
}
