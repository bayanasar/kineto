//! Veo through the video jobs, offline: a fake Gemini API answers the
//! submission with an operation, says it is running, then holds the video.
//! The clip is followed across a restart by its operation, never submitted
//! again, and attached. An operation Veo was too busy to finish, or a video
//! no longer on Veo's server, is final, and asking again submits afresh;
//! the busy one is reported as worth asking again.
//!
//! A submission reaches Veo in the frame shape it asks for, with the
//! reference images the model takes, and its parameters name both.
//!
//! The `planned` fixture's `office_night_02` has keyframe candidates with
//! images, which Veo starts from.

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
    video::{self, DEMO_CLIP, VEO_VIDEO_PROVIDER, VideoGenerator, VideoReference, VideoSubmission},
};
use kineto_jobs::ErrorClass;
use kineto_project::{
    manifest::CanonicalProject,
    shot::{ShotKey, ShotWorkflow, VideoState},
};
use kineto_provider_gemini::{
    DEFAULT_BASE_URL, HttpResponse, Transport, TransportFailure,
    veo::{VeoClient, VeoError, VeoModel},
};
use kineto_providers::{AspectRatio, ImageFormat, ReferenceRole, secret::SecretString};

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "kineto-veo-video-{}-{nonce}-{}",
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

/// How a fake operation ends.
#[derive(Debug, Clone, Copy)]
enum Ending {
    Video,
    /// Code 14: too busy.
    Busy,
    /// Its video is gone from the server.
    Expired,
}

/// The Gemini API, offline. Operation `n` ends as the `n`th ending says,
/// after answering once that it is running.
#[derive(Default)]
struct FakeVeo {
    endings: Vec<Ending>,
    submitted: Mutex<Vec<serde_json::Value>>,
    polled: Mutex<Vec<String>>,
}

const MODEL: &str = "veo-3.1-lite-generate-preview";

impl FakeVeo {
    fn ending(endings: &[Ending]) -> Arc<Self> {
        Arc::new(Self {
            endings: endings.to_vec(),
            ..Self::default()
        })
    }

    fn number(name: &str) -> usize {
        name.rsplit_once("/op")
            .and_then(|(_, n)| n.split(':').next())
            .and_then(|n| n.parse().ok())
            .expect("an operation of this fake")
    }
}

fn answer(status: u16, body: &[u8]) -> Result<HttpResponse, TransportFailure> {
    Ok(HttpResponse {
        status,
        body: body.to_vec(),
    })
}

/// The fake, as a client's transport.
#[derive(Clone)]
struct Api(Arc<FakeVeo>);

impl Transport for Api {
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        assert_eq!(api_key.expose(), "AQ.test");
        assert_eq!(
            url,
            format!("{DEFAULT_BASE_URL}/models/{MODEL}:predictLongRunning")
        );
        let mut submitted = self.0.submitted.lock().unwrap();
        submitted.push(serde_json::from_slice(body).unwrap());
        answer(
            200,
            format!(
                r#"{{"name":"models/{MODEL}/operations/op{}"}}"#,
                submitted.len()
            )
            .as_bytes(),
        )
    }

    fn get(
        &self,
        url: &str,
        api_key: &SecretString,
        _max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        assert_eq!(api_key.expose(), "AQ.test");
        let name = url
            .strip_prefix(&format!("{DEFAULT_BASE_URL}/"))
            .expect("an operation on the API");
        let mut polled = self.0.polled.lock().unwrap();
        let first = !polled.iter().any(|seen| seen == name);
        polled.push(name.to_owned());
        if first {
            return answer(200, format!(r#"{{"name":"{name}"}}"#).as_bytes());
        }
        let n = FakeVeo::number(name);
        let body = match self.0.endings[n - 1] {
            Ending::Busy => format!(
                r#"{{"name":"{name}","done":true,"error":{{"code":14,"message":"The service is currently unavailable."}}}}"#
            ),
            Ending::Video | Ending::Expired => format!(
                r#"{{"name":"{name}","done":true,"response":{{"generateVideoResponse":{{"generatedSamples":[{{"video":{{"uri":"{DEFAULT_BASE_URL}/files/op{n}:download?alt=media"}}}}]}}}}}}"#
            ),
        };
        answer(200, body.as_bytes())
    }

    fn download(
        &self,
        url: &str,
        api_key: &SecretString,
        _max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        assert_eq!(api_key.expose(), "AQ.test");
        match self.0.endings[FakeVeo::number(url) - 1] {
            Ending::Expired => answer(404, b""),
            Ending::Video | Ending::Busy => answer(200, DEMO_CLIP),
        }
    }
}

fn client(fake: &Arc<FakeVeo>) -> VeoClient<Api> {
    VeoClient::with_transport(
        Api(Arc::clone(fake)),
        SecretString::new("AQ.test".to_owned()).unwrap(),
        DEFAULT_BASE_URL,
        VeoModel::Lite,
    )
}

fn follow(
    project: &CanonicalProject,
    client: &VeoClient<Api>,
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

fn veo_error(error: GenerationError) -> (ErrorClass, VeoError) {
    let GenerationError::Provider { source, class } = error else {
        panic!("a provider failure: {error:?}");
    };
    (class, source.downcast_ref::<VeoError>().unwrap().clone())
}

#[test]
fn a_veo_clip_is_followed_across_a_restart_by_its_operation_and_attached() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    let project = film(&target);
    let fake = FakeVeo::ending(&[Ending::Video]);
    let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
    let job = video::start(&project, &shot, &client(&fake)).unwrap();
    assert_eq!(job.provider, VEO_VIDEO_PROVIDER);
    assert_eq!(job.task.as_str(), format!("models/{MODEL}/operations/op1"));

    // The locked keyframe, as base64; 5 s planned is a 4 s clip, the
    // nearer of 4 and 6 and the cheaper; no speech is asked for.
    let (_, keyframe) = shot.first_frame().unwrap().unwrap();
    let keyframe = fs::read(project.root().resolve(&keyframe.unwrap())).unwrap();
    {
        let submitted = fake.submitted.lock().unwrap();
        let instance = &submitted[0]["instances"][0];
        assert_eq!(
            instance["image"]["bytesBase64Encoded"],
            base64::engine::general_purpose::STANDARD.encode(&keyframe)
        );
        assert!(instance.get("lastFrame").is_none());
        assert!(
            instance["prompt"]
                .as_str()
                .unwrap()
                .contains("No speech or dialogue; only the sound of the scene."),
            "{instance}"
        );
        assert_eq!(
            submitted[0]["parameters"],
            serde_json::json!({
                "aspectRatio": "16:9",
                "durationSeconds": 4,
                "resolution": "720p",
                "personGeneration": "allow_adult"
            })
        );
    }
    drop((shot, project));

    // A new session follows the same operation; nothing is submitted.
    let project = CanonicalProject::open(&target).unwrap();
    let mut found = video::in_flight(&project, VEO_VIDEO_PROVIDER).unwrap();
    assert_eq!(found.jobs, [job]);
    assert!(found.unresolved.is_empty());
    follow(&project, &client(&fake), found.jobs.remove(0)).unwrap();
    assert_eq!(fake.submitted.lock().unwrap().len(), 1);
    assert_eq!(fake.polled.lock().unwrap().len(), 2);

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
    assert_eq!(record["provider"], VEO_VIDEO_PROVIDER);
    assert_eq!(record["model"], MODEL);
    assert_eq!(record["compiler_version"], "video-prompt/7");
    assert!(
        record["applied_constraints"]
            .as_array()
            .unwrap()
            .contains(&"audio.no_speech".into())
    );
    assert!(
        video::in_flight(&project, VEO_VIDEO_PROVIDER)
            .unwrap()
            .jobs
            .is_empty()
    );
}

#[test]
fn a_busy_or_expired_operation_is_final_and_asking_again_submits_afresh() {
    let temp = TempDir::new();
    let project = film(&temp.0.join("film"));
    let fake = FakeVeo::ending(&[Ending::Busy, Ending::Expired, Ending::Video]);
    let veo = client(&fake);
    let start = || {
        let shot = ShotWorkflow::load(&project, &shot_key()).unwrap();
        video::start(&project, &shot, &veo).unwrap()
    };

    // Over, but worth asking again: the director is told to try again.
    let (class, busy) = veo_error(follow(&project, &veo, start()).unwrap_err());
    assert_eq!(class, ErrorClass::Retryable);
    assert!(matches!(busy, VeoError::Failed { code: 14, .. }));
    assert!(busy.to_string().contains("Nothing was charged; ask again"));

    // Nothing may have been charged unseen, so no consent is asked for.
    let (class, expired) = veo_error(follow(&project, &veo, start()).unwrap_err());
    assert_eq!(class, ErrorClass::Terminal);
    assert_eq!(expired, VeoError::Download { status: 404 });
    assert!(expired.to_string().contains("2 days"));

    follow(&project, &veo, start()).unwrap();
    assert_eq!(fake.submitted.lock().unwrap().len(), 3);
    assert_eq!(
        ShotWorkflow::load(&project, &shot_key())
            .unwrap()
            .snapshot()
            .unwrap()
            .video,
        VideoState::Ready
    );
}

/// Any model's submissions, kept as sent.
#[derive(Clone, Default)]
struct Sent(Arc<Mutex<Vec<(String, serde_json::Value)>>>);

impl Transport for Sent {
    fn post_json(
        &self,
        url: &str,
        _api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let mut sent = self.0.lock().unwrap();
        sent.push((url.to_owned(), serde_json::from_slice(body).unwrap()));
        answer(
            200,
            format!(r#"{{"name":"models/m/operations/op{}"}}"#, sent.len()).as_bytes(),
        )
    }

    fn get(
        &self,
        _url: &str,
        _api_key: &SecretString,
        _max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        unreachable!("only submissions are made")
    }

    fn download(
        &self,
        _url: &str,
        _api_key: &SecretString,
        _max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        unreachable!("only submissions are made")
    }
}

#[test]
fn a_tall_clip_and_its_references_reach_only_a_model_that_takes_them() {
    let sent = Sent::default();
    let veo = |model| {
        VeoClient::with_transport(
            sent.clone(),
            SecretString::new("AQ.test".to_owned()).unwrap(),
            DEFAULT_BASE_URL,
            model,
        )
    };
    let portrait = vec![0x89, b'P', b'N', b'G', 1];
    let submission = VideoSubmission {
        prompt: "She turns to the window.".to_owned(),
        first_frame: Some((vec![0xff, 0xd8, 0xff, 0xe0], ImageFormat::Jpeg)),
        clip_seconds: 8,
        lines: Some(720),
        last_frame: None,
        aspect_ratio: AspectRatio::new(9, 16).unwrap(),
        references: vec![VideoReference {
            role: ReferenceRole::Identity,
            image: portrait.clone(),
            format: ImageFormat::Png,
        }],
    };

    // Its parameters name the frame shape and each reference by its hash,
    // so a different shape or portrait is a different input.
    let fast = veo(VeoModel::Fast);
    let params = fast.canonical_params(&submission);
    assert_eq!(
        params,
        format!(
            "duration=8;resolution=720p;aspect=9:16;reference={}",
            kineto_project::ContentHash::of(&portrait).as_str()
        )
    );
    let wide = VideoSubmission {
        aspect_ratio: AspectRatio::new(16, 9).unwrap(),
        references: Vec::new(),
        ..submission.clone()
    };
    assert_eq!(
        fast.canonical_params(&wide),
        "duration=8;resolution=720p;aspect=16:9",
        "a clip without references keeps the parameters it was made with"
    );

    VideoGenerator::submit(&fast, &submission).unwrap();
    {
        let sent = sent.0.lock().unwrap();
        let (url, body) = &sent[0];
        assert!(url.ends_with("/models/veo-3.1-fast-generate-preview:predictLongRunning"));
        assert_eq!(body["parameters"]["aspectRatio"], "9:16");
        assert_eq!(
            body["instances"][0]["referenceImages"],
            serde_json::json!([{
                "image": {
                    "bytesBase64Encoded":
                        base64::engine::general_purpose::STANDARD.encode(&portrait),
                    "mimeType": "image/png"
                },
                "referenceType": "asset"
            }])
        );
    }

    // Lite takes no reference, Veo keeps no style from one, and neither
    // reaches the API.
    assert!(matches!(
        VideoGenerator::submit(&veo(VeoModel::Lite), &submission),
        Err(VeoError::InvalidRequest(_))
    ));
    let styled = VideoSubmission {
        references: vec![VideoReference {
            role: ReferenceRole::Style,
            ..submission.references[0].clone()
        }],
        ..submission.clone()
    };
    assert!(matches!(
        VideoGenerator::submit(&veo(VeoModel::Standard), &styled),
        Err(VeoError::InvalidRequest(_))
    ));
    let plain = VideoSubmission {
        references: Vec::new(),
        ..submission
    };
    VideoGenerator::submit(&veo(VeoModel::Lite), &plain).unwrap();
    let sent = sent.0.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[1].1["parameters"]["aspectRatio"], "9:16");
    assert!(sent[1].1["instances"][0].get("referenceImages").is_none());
}
