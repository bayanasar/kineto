//! Gemini Omni through the video jobs, offline: a fake Interactions API
//! answers the submission with an interaction, says it is running, then
//! holds the video inline. The clip is followed across a restart by its
//! interaction, never submitted again, and attached; the text sent binds
//! the keyframe and says the clip's length, which its parameters name. A
//! failed interaction is final, worth asking again, and asking again
//! submits afresh.
//!
//! The `planned` fixture's `office_night_02` has keyframe candidates with
//! images, which Omni starts from.

use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use kineto_core::{
    generation::GenerationError,
    video::{self, DEMO_CLIP, OMNI_VIDEO_PROVIDER},
};
use kineto_jobs::ErrorClass;
use kineto_project::{
    manifest::CanonicalProject,
    shot::{ShotKey, ShotWorkflow, VideoState},
};
use kineto_provider_gemini::{
    DEFAULT_BASE_URL, HttpResponse, Transport, TransportFailure,
    omni::{OmniClient, OmniError},
};
use kineto_providers::secret::SecretString;

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-omni-video-{}-{nonce}-{}",
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

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn shot_key() -> ShotKey {
    ShotKey::new("office_night", "office_night_02").unwrap()
}

/// The planned film with both shots' keyframes locked: `office_night_02`'s
/// at its first candidate, which has an image.
fn film(target: &Path) -> CanonicalProject {
    copy_tree(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/projects/planned"),
        target,
    );
    let project = CanonicalProject::open(target).unwrap();
    let mut first = ShotWorkflow::load(
        &project,
        &ShotKey::new("office_night", "office_night_01").unwrap(),
    )
    .unwrap();
    first.generate(&project).unwrap();
    first.select(&project, 0).unwrap();
    first.lock(&project).unwrap();
    let mut shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    shot.select(&project, 0).unwrap();
    shot.lock(&project).unwrap();
    project
}

/// The Interactions API, offline. Interaction `n` fails when `failing`
/// names it, after answering once that it is running; otherwise it holds
/// the demo clip.
#[derive(Default)]
struct FakeOmni {
    failing: Vec<usize>,
    submitted: Mutex<Vec<serde_json::Value>>,
    polled: Mutex<Vec<String>>,
}

#[derive(Clone)]
struct Api(Arc<FakeOmni>);

fn answer(status: u16, body: &str) -> Result<HttpResponse, TransportFailure> {
    Ok(HttpResponse {
        status,
        body: body.as_bytes().to_vec(),
    })
}

impl Transport for Api {
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        assert_eq!(api_key.expose(), "AQ.test");
        assert_eq!(url, format!("{DEFAULT_BASE_URL}/interactions"));
        let mut submitted = self.0.submitted.lock().unwrap();
        submitted.push(serde_json::from_slice(body).unwrap());
        answer(
            200,
            &format!(
                r#"{{"id":"v1_op{}","status":"in_progress"}}"#,
                submitted.len()
            ),
        )
    }

    fn get(
        &self,
        url: &str,
        api_key: &SecretString,
        _max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        assert_eq!(api_key.expose(), "AQ.test");
        let id = url
            .strip_prefix(&format!("{DEFAULT_BASE_URL}/interactions/"))
            .expect("an interaction on the API");
        let mut polled = self.0.polled.lock().unwrap();
        let first = !polled.iter().any(|seen| seen == id);
        polled.push(id.to_owned());
        if first {
            return answer(200, &format!(r#"{{"id":"{id}","status":"in_progress"}}"#));
        }
        let n: usize = id.trim_start_matches("v1_op").parse().unwrap();
        if self.0.failing.contains(&n) {
            return answer(
                200,
                &format!(
                    r#"{{"id":"{id}","status":"failed","error":{{"message":"The service is overloaded."}}}}"#
                ),
            );
        }
        answer(
            200,
            &format!(
                r#"{{"id":"{id}","status":"completed","steps":[{{"type":"thought","signature":"x"}},{{"type":"model_output","content":[{{"type":"video","mime_type":"video/mp4","data":"{}"}}]}}]}}"#,
                base64::engine::general_purpose::STANDARD.encode(DEMO_CLIP)
            ),
        )
    }

    fn download(
        &self,
        _url: &str,
        _api_key: &SecretString,
        _max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        unreachable!("Omni's video comes inline")
    }
}

fn client(fake: &Arc<FakeOmni>) -> OmniClient<Api> {
    OmniClient::with_transport(
        Api(Arc::clone(fake)),
        SecretString::new("AQ.test".to_owned()).unwrap(),
        DEFAULT_BASE_URL,
    )
}

fn follow(
    project: &CanonicalProject,
    client: &OmniClient<Api>,
    job: video::VideoJob,
) -> Result<(), GenerationError> {
    let done = video::watch(
        client,
        project.root(),
        job,
        &AtomicBool::new(false),
        Duration::ZERO,
    )
    .unwrap();
    let mut shot = ShotWorkflow::load(project, &shot_key()).unwrap();
    video::finish(project, &mut shot, done).map(|_| ())
}

#[test]
fn an_omni_clip_is_followed_across_a_restart_by_its_interaction_and_attached() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target);
    let fake = Arc::new(FakeOmni::default());
    let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    let job = video::start(&project, &shot, &client(&fake)).unwrap();
    assert_eq!(job.provider, OMNI_VIDEO_PROVIDER);
    assert_eq!(job.task.as_str(), "v1_op1");

    // The locked keyframe, bound as the first frame; 5 s planned is a 5 s
    // clip, which the text asks for.
    let (_, keyframe) = shot.first_frame().unwrap().unwrap();
    let keyframe = fs::read(project.root().resolve(&keyframe.unwrap())).unwrap();
    {
        let submitted = fake.submitted.lock().unwrap();
        let body = &submitted[0];
        assert_eq!(body["model"], "gemini-omni-1.1-flash");
        assert_eq!(
            (&body["background"], &body["store"]),
            (&true.into(), &true.into())
        );
        let input = body["input"].as_array().unwrap();
        assert_eq!(input.len(), 2);
        assert_eq!(
            input[0]["data"],
            base64::engine::general_purpose::STANDARD.encode(&keyframe)
        );
        let text = input[1]["text"].as_str().unwrap();
        assert!(text.starts_with("<FIRST_FRAME> "), "{text}");
        assert!(text.ends_with(" The shot lasts 5 seconds."), "{text}");
        assert!(
            text.contains("No speech or dialogue; only the sound of the scene."),
            "{text}"
        );
        assert_eq!(
            body["response_format"],
            serde_json::json!({"type": "video", "aspect_ratio": "16:9", "resolution": "720p"})
        );
        assert!(body.get("generation_config").is_none());
    }
    drop((shot, project));

    // A new session follows the same interaction; nothing is submitted.
    let project = CanonicalProject::open(&target).unwrap();
    let mut found = video::in_flight(&project, OMNI_VIDEO_PROVIDER).unwrap();
    assert_eq!(found.jobs, [job]);
    follow(&project, &client(&fake), found.jobs.remove(0)).unwrap();
    assert_eq!(fake.submitted.lock().unwrap().len(), 1);

    let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    assert_eq!(shot.snapshot().unwrap().video, VideoState::Ready);
    assert_eq!(
        project
            .root()
            .read(&shot.video_media().unwrap().unwrap())
            .unwrap(),
        DEMO_CLIP
    );
    let artifacts: serde_json::Value = serde_json::from_slice(
        &fs::read(target.join("scenes/office_night/shots/office_night_02/artifacts.json")).unwrap(),
    )
    .unwrap();
    let record = artifacts
        .as_array()
        .unwrap()
        .iter()
        .find(|artifact| artifact["type"] == "shot_video")
        .unwrap();
    assert_eq!(record["provider"], OMNI_VIDEO_PROVIDER);
    assert_eq!(record["model"], "gemini-omni-1.1-flash");
}

#[test]
fn a_failed_interaction_is_final_worth_asking_again_and_asking_again_submits_afresh() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"));
    let fake = Arc::new(FakeOmni {
        failing: vec![1],
        ..FakeOmni::default()
    });
    let omni = client(&fake);
    let start = || {
        let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
        video::start(&project, &shot, &omni).unwrap()
    };

    let GenerationError::Provider { source, class } = follow(&project, &omni, start()).unwrap_err()
    else {
        panic!("a provider failure");
    };
    assert_eq!(class, ErrorClass::Retryable);
    assert_eq!(
        source.downcast_ref::<OmniError>(),
        Some(&OmniError::Failed("The service is overloaded.".to_owned()))
    );

    follow(&project, &omni, start()).unwrap();
    assert_eq!(fake.submitted.lock().unwrap().len(), 2);
    assert_eq!(
        ShotWorkflow::load(&project, &shot_key())
            .unwrap()
            .snapshot()
            .unwrap()
            .video,
        VideoState::Ready
    );
}
