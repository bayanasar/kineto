//! Kill the process the moment MiniMax has accepted a video, before the job's
//! intent records the task, then prove that the next session follows that
//! task and never submits (pays for) the video again.
//!
//! The test binary re-runs itself as a child with `KINETO_VIDEO_CRASH` set.
//! The child submits the shot's video and calls `std::process::abort()` at
//! `VideoPoint::Accepted`: no destructors, no rollback. The parent then opens
//! the project the way the app does.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::Engine as _;
use kineto_core::{
    generation::ImageProvider,
    session::{
        OpenRequest, ProjectSession, Recovery, SessionError, SessionOptions, SessionSink,
        ShotCommand,
    },
    video::{self, VideoHooks, VideoPoint},
};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::{ShotWorkflow, VideoState},
};
use kineto_provider_minimax::{
    HttpResponse, MinimaxClient, SharedTransport, Transport, TransportFailure,
};
use kineto_providers::secret::SecretString;

const CHILD: &str = "KINETO_VIDEO_CRASH";
const WAIT: Duration = Duration::from_secs(10);
const MP4: &[u8] = b"\0\0\0\x18ftypisom\0\0\x02\0isommp41-video";

/// MiniMax offline: `image-01` keyframes, a Hailuo task `777` that has
/// finished, and its file.
#[derive(Default)]
struct FakeMinimax {
    images: AtomicU32,
    submits: AtomicU32,
}

fn answer(body: impl Into<Vec<u8>>) -> Result<HttpResponse, TransportFailure> {
    Ok(HttpResponse {
        status: 200,
        body: body.into(),
    })
}

impl Transport for FakeMinimax {
    fn post_json(
        &self,
        url: &str,
        _api_key: &SecretString,
        _body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        if url.ends_with("/v1/video_generation") {
            self.submits.fetch_add(1, Ordering::SeqCst);
            return answer(r#"{"task_id":"777","base_resp":{"status_code":0}}"#);
        }
        let call = self.images.fetch_add(1, Ordering::SeqCst);
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x51, u8::try_from(call).unwrap()];
        answer(format!(
            r#"{{"id":"i","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
            base64::engine::general_purpose::STANDARD.encode(jpeg)
        ))
    }

    fn get(&self, url: &str, _api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        if url.contains("/v1/query/video_generation?task_id=777") {
            return answer(r#"{"status":"Success","file_id":"999","base_resp":{"status_code":0}}"#);
        }
        assert!(url.ends_with("/v1/files/retrieve?file_id=999"), "{url}");
        answer(
            r#"{"file":{"download_url":"https://cdn.example.test/v.mp4"},"base_resp":{"status_code":0}}"#,
        )
    }

    fn download(&self, url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        assert_eq!(url, "https://cdn.example.test/v.mp4");
        answer(MP4)
    }
}

fn client(fake: &Arc<FakeMinimax>) -> MinimaxClient<SharedTransport> {
    MinimaxClient::with_transport(
        Arc::clone(fake) as SharedTransport,
        SecretString::new("sk-test".to_owned()).unwrap(),
        "https://example.test",
    )
}

/// The child half. A no-op unless the parent asked for a crash.
#[test]
fn crash_child() {
    let Ok(path) = env::var(CHILD) else {
        return;
    };

    struct AbortWhenAccepted;
    impl VideoHooks for AbortWhenAccepted {
        fn reached(&mut self, point: VideoPoint) {
            if point == VideoPoint::Accepted {
                std::process::abort();
            }
        }
    }

    let project = CanonicalProject::open(&path).expect("child opens the project");
    let shot = ShotWorkflow::load(&project, 1).expect("child loads shot 1");
    let fake = Arc::new(FakeMinimax::default());
    let _ = video::start_with(&project, &shot, &client(&fake), &mut AbortWhenAccepted);
    panic!("the child must abort once the video is accepted");
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
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!("{nonce}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        let path =
            env::temp_dir().join(format!("kineto-video-crash-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Recovered(String),
    Opened(Result<(), String>),
    Completed(Result<(), String>),
    VideoFinished(Result<(), String>),
}

struct Sink(Mutex<mpsc::Sender<Event>>);

fn describe(result: &Result<(), SessionError>) -> Result<(), String> {
    result
        .as_ref()
        .map(|_| ())
        .map_err(|error| format!("{error:?}"))
}

impl SessionSink for Sink {
    fn recovered(&self, recovery: &Recovery) {
        let _ = self
            .0
            .lock()
            .unwrap()
            .send(Event::Recovered(format!("{recovery:?}")));
    }

    fn opened(&self, result: &Result<(), SessionError>) {
        let _ = self.0.lock().unwrap().send(Event::Opened(describe(result)));
    }

    fn completed(&self, _token: u64, _shot_index: u32, result: &Result<(), SessionError>) {
        let _ = self
            .0
            .lock()
            .unwrap()
            .send(Event::Completed(describe(result)));
    }

    fn video_finished(&self, _shot_index: u32, _revision: u16, result: &Result<(), SessionError>) {
        let _ = self
            .0
            .lock()
            .unwrap()
            .send(Event::VideoFinished(describe(result)));
    }

    fn document_completed(&self, _token: u64, _result: &Result<(), SessionError>) {}
}

fn spawn(request: OpenRequest, fake: &Arc<FakeMinimax>) -> (ProjectSession, mpsc::Receiver<Event>) {
    let (sender, events) = mpsc::channel();
    let session = ProjectSession::spawn_with(
        request,
        ImageProvider::Minimax(Some(client(fake))),
        Box::new(Sink(Mutex::new(sender))),
        SessionOptions {
            video_poll_interval: Duration::from_millis(20),
        },
    )
    .unwrap();
    (session, events)
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "video_crash".to_owned(),
        title: "Video Crash".to_owned(),
        created_at: "2026-10-01T00:00:00Z".to_owned(),
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

/// A project whose shot 1 has a locked MiniMax keyframe.
fn locked_keyframe(target: &Path) {
    let fake = Arc::new(FakeMinimax::default());
    let (session, events) = spawn(
        OpenRequest::CreateText {
            path: target.to_path_buf(),
            manifest: Box::new(manifest()),
            source: b"A filmmaker waits.\n".to_vec(),
        },
        &fake,
    );
    assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
    for (token, command) in [
        ShotCommand::Generate,
        ShotCommand::Select(1),
        ShotCommand::Lock,
    ]
    .into_iter()
    .enumerate()
    {
        session.submit(0, command, token as u64).unwrap();
        assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Completed(Ok(())));
    }
}

fn video_intent(target: &Path) -> serde_json::Value {
    fs::read_dir(target.join(".kineto/jobs/intents"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice::<serde_json::Value>(&fs::read(path).unwrap()).unwrap())
        .find(|intent| intent["provider"] == video::VIDEO_PROVIDER)
        .expect("a video intent exists")
}

#[test]
fn killed_after_minimax_accepted_the_video_the_task_is_followed_not_resubmitted() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    locked_keyframe(&target);

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

    // The window under test: accepted and written down, intent not updated.
    let intent = video_intent(&target);
    assert_eq!(intent["state"], "prepared");
    assert!(intent["provider_job_id"].is_null(), "{intent}");
    let artifact = intent["artifact_id"].as_str().unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(
        &fs::read(target.join(format!(".kineto/generation/{artifact}.json"))).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["task_id"], "777");

    let fake = Arc::new(FakeMinimax::default());
    let (session, events) = spawn(
        OpenRequest::Open {
            path: target.clone(),
        },
        &fake,
    );
    assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
    assert_eq!(
        events.recv_timeout(WAIT).unwrap(),
        Event::VideoFinished(Ok(()))
    );
    assert_eq!(session.snapshot(0).unwrap().video, VideoState::Ready);
    assert_eq!(
        fs::read(session.video_media(0).unwrap().unwrap()).unwrap(),
        MP4
    );
    assert_eq!(fake.submits.load(Ordering::SeqCst), 0, "never paid twice");
    drop(session);
    assert_eq!(video_intent(&target)["state"], "reconciled");
}
