//! The director's preferences as later proposals read them, offline: shot
//! planning and keyframes are made with them, record which profile they
//! followed, and when it changes say so without going stale.

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
    bible,
    generation::{self, Consent, NoHooks},
    minimax::MinimaxKeyframes,
    screenplay, shot_plan, story,
    style::{self, SampleRequest, Submission},
};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    DependencyImpact,
    bible::SAMPLE_BIBLE_JSON,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    preferences::{self as document, Dimension, Preference, Preferences, Source, Stance},
    revisions::{self, DocumentKind},
    screenplay::SAMPLE_SCREENPLAY_JSON,
    shot::{ShotKey, ShotWorkflow},
    shot_plan::SAMPLE_SHOT_PLAN_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document,
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::secret::SecretString;

/// MiniMax `image-01`, offline: a distinct JPEG for every call, and every
/// prompt kept.
#[derive(Default)]
struct FakeImages {
    calls: AtomicU32,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl Transport for FakeImages {
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
        unreachable!("no GET request: {url}")
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("nothing is downloaded: {url}")
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = env::temp_dir().join(format!(
            "kineto-preferences-{}-{nonce}-{}",
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

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "preferences".to_owned(),
        title: "Preferences".to_owned(),
        created_at: "2026-10-03T00:00:00Z".to_owned(),
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
            recipe_version: 2,
            extra: BTreeMap::new(),
        },
        extra: BTreeMap::new(),
    }
}

fn entry(dimension: Dimension, stance: Stance, value: &str) -> Preference {
    Preference {
        dimension,
        stance,
        value: value.to_owned(),
        strength_permille: None,
        source: Source::Director,
    }
}

const PLANNING: &str = "The director's preferences, where the locked style allows: prefer slow \
dolly camera motion, low-key lighting, long lens; avoid handheld camera motion";
const STILL: &str = "The director's preferences, where the locked style allows: prefer low-key \
lighting, long lens.";

fn film(target: &Path, images: &MinimaxClient<FakeImages>) -> CanonicalProject {
    let project =
        CanonicalProject::create(target, manifest(), b"A filmmaker hears the last line.\n")
            .unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    kineto_project::screenplay::lock(&project, 1).unwrap();
    bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).unwrap();
    kineto_project::bible::lock(&project, 1).unwrap();
    let sample = style::sample_with(
        &project,
        MinimaxKeyframes::new(&project, Some(images)),
        SampleRequest {
            preset: "film_noir",
            ..SampleRequest::default()
        },
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    style_document::select(&project, &sample.artifact_id).unwrap();
    style_document::lock(&project, &sample.artifact_id).unwrap();
    project
}

#[test]
fn plans_and_keyframes_follow_the_preferences_in_force_and_say_when_they_changed() {
    let temp = TempDir::new();
    let fake = FakeImages::default();
    let prompts = Arc::clone(&fake.prompts);
    let images = MinimaxClient::with_transport(
        fake,
        SecretString::new("sk-test".to_owned()).unwrap(),
        "https://example.test",
    );
    let project = film(&temp.0.join("film"), &images);
    let first = document::save(
        &project,
        None,
        &Preferences {
            entries: vec![
                entry(Dimension::CameraMotion, Stance::Avoid, "handheld"),
                entry(Dimension::CameraMotion, Stance::Prefer, "slow dolly"),
                entry(Dimension::Lighting, Stance::Prefer, "low-key"),
                entry(Dimension::Lens, Stance::Prefer, "long"),
            ],
        },
    )
    .unwrap();

    // Shot planning reads every dimension, and records the profile it read
    // without depending on it.
    let planner = FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON]);
    let plan = shot_plan::write(&project, &planner).unwrap();
    let request = &planner.requests()[0];
    let input = &request.messages[1].content;
    assert!(input.contains(&format!("<preferences>\n{PLANNING}\n</preferences>")));
    assert!(request.messages[0].content.contains("favour them wherever"));
    let preference = plan
        .dependencies
        .iter()
        .find(|dependency| dependency.field.as_deref() == Some("preferences"))
        .unwrap();
    assert_eq!(preference.artifact_id, first.artifact_id);
    assert_eq!(preference.impact, DependencyImpact::Cosmetic);
    assert!(
        plan.provenance
            .as_ref()
            .unwrap()
            .compiled_prompt
            .contains(&format!("<preferences {} ", first.artifact_id))
    );
    kineto_project::shot_plan::lock(&project, plan.revision).unwrap();

    // A keyframe reads what a still can show: not motion.
    let key = ShotKey::new("office_night", "office_night_02").unwrap();
    let mut shot = ShotWorkflow::load(&project, &key).unwrap();
    generation::generate_noted(
        &project,
        &mut shot,
        MinimaxKeyframes::new(&project, Some(&images)),
        &mut NoHooks,
        Consent::default(),
        None,
    )
    .unwrap();
    let prompt = prompts.lock().unwrap().last().unwrap().clone();
    assert!(prompt.contains(STILL), "{prompt}");
    assert!(!prompt.contains("dolly"));
    assert!(
        prompt.find(STILL).unwrap() > prompt.find("Black-and-white").unwrap(),
        "the locked style comes first"
    );
    let changed = |project: &CanonicalProject, shot: &ShotWorkflow| {
        generation::preferences_changed(project, shot).unwrap()
    };
    assert_eq!(changed(&project, &shot), [false, false, false]);

    // The director changes the profile: nothing goes stale, but what was
    // made with the old one says so.
    document::save(
        &project,
        Some(first.revision),
        &Preferences {
            entries: vec![entry(Dimension::Lighting, Stance::Prefer, "high-key")],
        },
    )
    .unwrap();
    assert_eq!(changed(&project, &shot), [true, true, true]);
    assert!(
        generation::keyframe_staleness(&project, &shot)
            .unwrap()
            .iter()
            .all(Vec::is_empty)
    );
    let plan_record = revisions::latest(&project, DocumentKind::ShotPlan)
        .unwrap()
        .unwrap();
    assert!(
        kineto_project::shot_plan::staleness(&project, &plan_record)
            .unwrap()
            .is_empty()
    );
    let (now, _) = document::current(&project).unwrap().unwrap();
    assert!(document::revision_changed_since(
        &plan_record.dependencies,
        Some(&now)
    ));
    shot.select(&project, 1).unwrap();
    generation::lock(&project, &mut shot).unwrap();
    assert!(shot.snapshot().unwrap().locked);
}
