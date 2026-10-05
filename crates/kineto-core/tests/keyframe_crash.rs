//! Kill the process the moment MiniMax has answered a paid keyframe call,
//! before the keyframe or its receipt is written, then prove that the call is
//! never paid for again unasked: the shot's generation is unresolved until
//! the director chooses to risk paying twice.
//!
//! The test binary re-runs itself as a child with `KINETO_KEYFRAME_CRASH`
//! set. The child generates the shot's keyframes and calls
//! `std::process::abort()` as soon as MiniMax answers: no destructors, no
//! rollback. The parent then inspects what is left and asks again.

use kineto_project::shot::ShotKey;
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
    generation::{self, GenerationError, ImageProvider, NoHooks},
    minimax::MinimaxKeyframes,
    session::{OpenRequest, ProjectSession, Recovery, SessionError, SessionSink},
};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::{AttachOutcome, ShotWorkflow},
};
use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
use kineto_providers::secret::SecretString;

const CHILD: &str = "KINETO_KEYFRAME_CRASH";
/// How long an expected event may take. Only a hang reaches it.
const WAIT: Duration = Duration::from_secs(60);

/// MiniMax `image-01`, offline.
#[derive(Default)]
struct FakeMinimax {
    calls: Arc<AtomicU32>,
}

impl Transport for FakeMinimax {
    fn post_json(
        &self,
        _url: &str,
        _api_key: &SecretString,
        _body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x51, u8::try_from(call).unwrap()];
        Ok(HttpResponse {
            status: 200,
            body: format!(
                r#"{{"id":"i","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
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

fn client() -> (MinimaxClient<FakeMinimax>, Arc<AtomicU32>) {
    let fake = FakeMinimax::default();
    let calls = Arc::clone(&fake.calls);
    (
        MinimaxClient::with_transport(
            fake,
            SecretString::new("sk-test".to_owned()).unwrap(),
            "https://example.test",
        ),
        calls,
    )
}

/// The child half. A no-op unless the parent asked for a crash.
#[test]
fn crash_child() {
    let Ok(path) = env::var(CHILD) else {
        return;
    };
    let project = CanonicalProject::open(path).expect("child opens the project");
    let mut shot =
        ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).expect("child loads shot 1");
    let (client, _) = client();
    let abort = || std::process::abort();
    let _ = generation::generate_with(
        &project,
        &mut shot,
        MinimaxKeyframes::new(&project, Some(&client)).when_answered(&abort),
        &mut NoHooks,
    );
    panic!("the child must abort once MiniMax answers");
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
        let path = env::temp_dir().join(format!(
            "kineto-keyframe-crash-{}-{nonce}",
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

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Recovered(String),
    Opened(Result<(), String>),
}

struct Sink(Mutex<mpsc::Sender<Event>>);

impl SessionSink for Sink {
    fn recovered(&self, recovery: &Recovery) {
        let _ = self
            .0
            .lock()
            .unwrap()
            .send(Event::Recovered(format!("{recovery:?}")));
    }

    fn opened(&self, result: &Result<(), SessionError>) {
        let _ = self.0.lock().unwrap().send(Event::Opened(
            result
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("{error:?}")),
        ));
    }

    fn completed(&self, _token: u64, _shot_index: u32, _result: &Result<(), SessionError>) {}

    fn video_finished(&self, _shot_index: u32, _revision: u16, _result: &Result<(), SessionError>) {
    }
    fn document_completed(&self, _token: u64, _result: &Result<(), SessionError>) {}
}

/// Open the project as the app does, here without a MiniMax key.
fn open(request: OpenRequest) -> ProjectSession {
    let (sender, events) = mpsc::channel();
    let session = ProjectSession::spawn(
        request,
        ImageProvider::Minimax(None),
        Box::new(Sink(Mutex::new(sender))),
    )
    .unwrap();
    assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
    session
}

fn manifest() -> ProjectManifest {
    ProjectManifest {
        format_version: PROJECT_FORMAT_VERSION,
        project_id: "keyframe_crash".to_owned(),
        title: "Keyframe Crash".to_owned(),
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

fn intents(target: &Path) -> Vec<serde_json::Value> {
    fs::read_dir(target.join(".kineto/jobs/intents"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| serde_json::from_slice(&fs::read(path).unwrap()).unwrap())
        .collect()
}

#[test]
fn killed_as_minimax_answered_the_keyframes_wait_for_the_director() {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    drop(open(OpenRequest::CreateText {
        path: target.clone(),
        manifest: Box::new(manifest()),
        source: b"A filmmaker waits.\n".to_vec(),
    }));

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

    // The window under test: at least one call answered, nothing written.
    let intents = intents(&target);
    assert_eq!(intents.len(), 3);
    assert!(intents.iter().all(|intent| intent["state"] == "prepared"));
    assert!(intents.iter().any(|intent| intent["attempts"] == 1));
    assert!(
        !target.join(".kineto/generation").exists() || {
            fs::read_dir(target.join(".kineto/generation"))
                .unwrap()
                .count()
                == 0
        }
    );

    // A reopened session cannot know whether MiniMax charged.
    let session = open(OpenRequest::Open {
        path: target.clone(),
    });
    assert!(session.keyframes_unresolved(0).unwrap());
    assert!(!session.snapshot(0).unwrap().generated);
    drop(session);

    let project = CanonicalProject::open(&target).unwrap();
    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
    let (client, calls) = client();
    let minimax = || MinimaxKeyframes::new(&project, Some(&client));
    assert!(matches!(
        generation::generate_with(&project, &mut shot, minimax(), &mut NoHooks),
        Err(GenerationError::UnresolvedSubmission)
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "never paid twice unasked");

    // The director decides to risk it.
    let outcome = generation::resubmit_with(&project, &mut shot, minimax(), &mut NoHooks).unwrap();
    assert_eq!(outcome, AttachOutcome::Attached);
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert!(shot.snapshot().unwrap().generated);
}
