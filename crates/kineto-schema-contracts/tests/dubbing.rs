//! A scene's dubbed lines, and a package that carries them, as the engine
//! writes them, match the committed schemas.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kineto_core::{
    bible, cast,
    dubbing::{self, DubRequest},
    export::{self, ExportRequest},
    generation::{ImageProvider, NoHooks},
    minimax::MinimaxKeyframes,
    screenplay, shot_plan,
    speech::{LocalDemoVoices, VoiceSettings},
    story,
    style::{self, SampleRequest, Submission},
    video::{self, LocalDemoVideos},
    voice::{self, VoiceChoice},
};
use kineto_json_schema::{ensure_supported_schema, validate_value};
use kineto_llm::fake::FakeLlm;
use kineto_project::{
    bible::SAMPLE_BIBLE_JSON,
    cast as cast_document, dubbing as document,
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    screenplay::SAMPLE_SCREENPLAY_JSON,
    shot::{ShotKey, ShotWorkflow},
    shot_plan::SAMPLE_SHOT_PLAN_JSON,
    story::SAMPLE_ANALYSIS_JSON,
    style as style_document, voice as voice_document,
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::secret::SecretString;
use serde_json::Value;

struct Images;

impl Transport for Images {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        _body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status: 200,
            // base64 of ff d8 ff e0 00 10
            body: br#"{"id":"r","data":{"image_base64":["/9j/4AAQ"]},"base_resp":{"status_code":0,"status_msg":"success"}}"#
                .to_vec(),
        })
    }

    fn get(&self, _url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        unreachable!("image generation makes no GET request")
    }

    fn download(&self, _url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        unreachable!("image generation downloads nothing")
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
            "kineto-dubbing-contract-{}-{nonce}-{}",
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

fn schema(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .join(name);
    let value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    ensure_supported_schema(&value, "$").unwrap();
    value
}

fn json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "dubbing_contract".to_owned(),
        title: "Dubbing".to_owned(),
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

#[test]
fn dubbed_lines_and_the_package_that_carries_them_match_committed_schemas() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project =
        CanonicalProject::create(&target, manifest(), b"A filmmaker hears the last line.\n")
            .unwrap();
    story::analyze(&project, &FakeLlm::answering([SAMPLE_ANALYSIS_JSON])).unwrap();
    screenplay::write(&project, &FakeLlm::answering([SAMPLE_SCREENPLAY_JSON])).unwrap();
    kineto_project::screenplay::lock(&project, 1).unwrap();
    bible::write(&project, &FakeLlm::answering([SAMPLE_BIBLE_JSON])).unwrap();
    let client = MinimaxClient::with_transport(
        Images,
        SecretString::new("sk-contract".to_owned()).unwrap(),
        "https://example.test",
    );
    let adapter = || MinimaxKeyframes::new(&project, Some(&client));
    let sample = style::sample_with(
        &project,
        adapter(),
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
    cast::audition_with(
        &project,
        adapter(),
        "interviewer",
        1,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    cast_document::select(&project, "interviewer", "cast.interviewer.0001").unwrap();
    cast::make_pack_with(
        &project,
        adapter(),
        "interviewer",
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    cast::lock(
        &project,
        &ImageProvider::Minimax(Some(MinimaxClient::shared(
            SecretString::new("sk-contract".to_owned()).unwrap(),
        ))),
        "interviewer",
    )
    .unwrap();
    kineto_project::bible::lock(&project, 1).unwrap();
    shot_plan::write(&project, &FakeLlm::answering([SAMPLE_SHOT_PLAN_JSON])).unwrap();
    kineto_project::shot_plan::lock(&project, 1).unwrap();
    voice::audition(
        &project,
        &LocalDemoVoices,
        "interviewer",
        &[VoiceChoice {
            voice: VoiceSettings::new("demo_low", "en"),
            name: "Low".to_owned(),
        }],
        None,
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    voice_document::select(&project, "interviewer", "voice.interviewer.0001").unwrap();
    voice::lock(&project, "interviewer").unwrap();

    dubbing::dub(
        &project,
        &LocalDemoVoices,
        "office_night",
        1,
        &DubRequest {
            takes: 2,
            note: Some("Let it land.".to_owned()),
            ..DubRequest::default()
        },
        Submission::Once,
        &mut NoHooks,
    )
    .unwrap();
    document::select(
        &project,
        "office_night",
        "line.office_night.01-2c40ae8c8fe4.0001",
    )
    .unwrap();
    document::reject(
        &project,
        "office_night",
        "line.office_night.01-2c40ae8c8fe4.0002",
    )
    .unwrap();
    dubbing::lock(&project, "office_night", 1).unwrap();

    let mut shot = ShotWorkflow::load(
        &project,
        &ShotKey::new("office_night", "office_night_02").unwrap(),
    )
    .unwrap();
    shot.generate(&project).unwrap();
    shot.select(&project, 0).unwrap();
    shot.lock(&project).unwrap();
    let job = video::start(&project, &shot, &LocalDemoVideos).unwrap();
    let done = video::watch(
        &LocalDemoVideos,
        project.root(),
        job,
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    video::finish(&project, &mut shot, done).unwrap();
    shot.lock_video(&project).unwrap();

    let lines_schema = schema("lines.schema.json");
    let lines = json(&target.join("scenes/office_night/voice/lines.json"));
    validate_value(&lines_schema, &lines, "$").unwrap();
    let mut drifted = lines.clone();
    drifted["takes"][0]["status"] = "maybe".into();
    assert!(validate_value(&lines_schema, &drifted, "$").is_err());
    let mut drifted = lines.clone();
    drifted["locks"][0]["line"] = 0.into();
    assert!(validate_value(&lines_schema, &drifted, "$").is_err());
    assert_eq!(lines["schema_version"], 2);
    assert_eq!(
        lines["locks"][0]["text_hash"],
        lines["takes"][0]["text_hash"]
    );
    let mut drifted = lines.clone();
    drifted["locks"][0]["text_hash"] = "Was it worth it?".into();
    assert!(validate_value(&lines_schema, &drifted, "$").is_err());

    let out = temp.0.join("out");
    fs::create_dir_all(&out).unwrap();
    export::export(
        &project,
        &ExportRequest {
            destination: out.clone(),
            name: "dailies".to_owned(),
            partial: true,
            replace: false,
        },
        UNIX_EPOCH + Duration::from_secs(1_791_015_045),
    )
    .unwrap();
    let sidecar = json(&out.join("dailies/office_night_02.json"));
    validate_value(&schema("export-shot.schema.json"), &sidecar, "$").unwrap();
    assert_eq!(sidecar["lines"][0]["file"], "office_night_02_line01.wav");
    let package = json(&out.join("dailies/manifest.json"));
    validate_value(&schema("export-manifest.schema.json"), &package, "$").unwrap();
    let mut drifted = sidecar.clone();
    drifted["lines"][0]["file"] = "../escape.wav".into();
    assert!(validate_value(&schema("export-shot.schema.json"), &drifted, "$").is_err());
}
