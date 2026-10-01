//! Kill the process at each durable step of a generation, then prove that the
//! next session completes that generation exactly once.
//!
//! The test binary re-runs itself as a child with `KINETO_GENERATION_CRASH`
//! set. The child starts a generation and calls `std::process::abort()` at the
//! requested point: no destructors, no rollback, the single-writer lock
//! released only because the OS reclaims it. The parent then opens the project
//! the way the app does and inspects what the recovery did.

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
    sync::{Mutex, mpsc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use kineto_core::{
    generation::{self, GenerationHooks, GenerationPoint},
    session::{OpenRequest, ProjectSession, Recovery, SessionError, SessionSink},
};
use kineto_project::{
    manifest::{
        CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
        ProjectWorkflow,
    },
    shot::ShotWorkflow,
};

const CHILD: &str = "KINETO_GENERATION_CRASH";
const WAIT: Duration = Duration::from_secs(10);

fn point_named(name: &str) -> GenerationPoint {
    match name {
        "first-intent" => GenerationPoint::Prepared { index: 0 },
        "invoked" => GenerationPoint::Invoked,
        "attached" => GenerationPoint::Attached,
        other => panic!("unknown crash point {other}"),
    }
}

/// The child half. A no-op unless the parent asked for a crash.
#[test]
fn crash_child() {
    let Ok(spec) = env::var(CHILD) else {
        return;
    };
    let (point, path) = spec.split_once('|').expect("crash spec is point|path");
    let point = point_named(point);

    struct AbortAt(GenerationPoint);
    impl GenerationHooks for AbortAt {
        fn reached(&mut self, point: GenerationPoint) {
            if point == self.0 {
                std::process::abort();
            }
        }
    }

    let project = CanonicalProject::open(path).expect("child opens the project");
    let mut shot = ShotWorkflow::load(&project, 1).expect("child loads shot 1");
    let _ = generation::generate(
        &project,
        &mut shot,
        &generation::ImageProvider::LocalDemo,
        &mut AbortAt(point),
    );
    panic!("the child must abort at {point:?}");
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
        let nonce = format!(
            "{nonce}-{}",
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        let path = env::temp_dir().join(format!(
            "kineto-generation-crash-{}-{nonce}",
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

fn create_project(target: &Path) {
    drop(
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "generation_crash".to_owned(),
                title: "Generation Crash".to_owned(),
                created_at: "2026-09-30T00:00:00Z".to_owned(),
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
            },
            b"A filmmaker waits across the table.\n",
        )
        .unwrap(),
    );
}

fn crash_at(point: &str, target: &Path) -> ExitStatus {
    Command::new(env::current_exe().unwrap())
        .args(["crash_child", "--exact", "--nocapture", "--test-threads=1"])
        .env(CHILD, format!("{point}|{}", target.display()))
        // Where core dumps are enabled with a relative core_pattern (as in the
        // CI runner image), the aborted child writes `core*` into its working
        // directory. Keep that inside the test's temporary directory.
        .current_dir(target.parent().expect("target has a parent"))
        .status()
        .unwrap()
}

/// Core dumps from aborted children must never land in the checkout.
fn assert_no_core_dump_in_the_package() {
    let package = Path::new(env!("CARGO_MANIFEST_DIR"));
    let cores = fs::read_dir(package)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name == "core" || name.starts_with("core."))
        .collect::<Vec<_>>();
    assert!(
        cores.is_empty(),
        "core dumps left in the package: {cores:?}"
    );
}

fn intent_states(target: &Path) -> Vec<String> {
    let mut states = fs::read_dir(target.join(".kineto/jobs/intents"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .map(|path| {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["state"].as_str().unwrap().to_owned()
        })
        .collect::<Vec<_>>();
    states.sort();
    states
}

fn artifact_ids(target: &Path) -> Vec<String> {
    let path = target.join("scenes/scene_001/shots/shot_001/artifacts.json");
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let artifacts: Vec<serde_json::Value> = serde_json::from_slice(&bytes).unwrap();
    artifacts
        .iter()
        .map(|artifact| artifact["artifact_id"].as_str().unwrap().to_owned())
        .collect()
}

#[derive(Debug)]
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

/// Open the project as the app does; return what it recovered before opening.
fn open_session(target: &Path) -> (ProjectSession, Vec<String>) {
    let (sender, events) = mpsc::channel();
    let session = ProjectSession::spawn(
        OpenRequest::Open {
            path: target.to_path_buf(),
        },
        generation::ImageProvider::LocalDemo,
        Box::new(Sink(Mutex::new(sender))),
    )
    .unwrap();
    let mut recovered = Vec::new();
    loop {
        match events.recv_timeout(WAIT).unwrap() {
            Event::Recovered(recovery) => recovered.push(recovery),
            Event::Opened(result) => {
                assert_eq!(result, Ok(()));
                return (session, recovered);
            }
        }
    }
}

fn crash_then_recover(point: &str, expected_intents: &[&str], candidates_on_disk: usize) {
    let temp = TempDir::new();
    let target = temp.0.join("film");
    create_project(&target);

    let status = crash_at(point, &target);
    assert!(!status.success(), "the child must not exit cleanly");
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(6), "the child must die by SIGABRT");
    }
    assert_no_core_dump_in_the_package();
    assert_eq!(intent_states(&target), expected_intents);
    assert_eq!(artifact_ids(&target).len(), candidates_on_disk);

    let (session, recovered) = open_session(&target);
    assert_eq!(
        recovered,
        vec!["Completed { shot_index: 0, revision: 1 }".to_owned()]
    );
    let snapshot = session.snapshot(0).unwrap();
    assert!(snapshot.generated);
    assert_eq!(snapshot.generation_revision, 1);
    assert_eq!(snapshot.candidate_count, 3);
    assert_eq!(snapshot.superseded_count, 0);
    drop(session);

    let ids = artifact_ids(&target);
    assert_eq!(
        ids,
        vec![
            "shot_001_g0001_candidate_a",
            "shot_001_g0001_candidate_b",
            "shot_001_g0001_candidate_c",
        ],
        "each candidate attached exactly once"
    );
    assert_eq!(intent_states(&target), vec!["reconciled"; 3]);

    // Nothing is left to recover on the next open.
    let (_session, recovered) = open_session(&target);
    assert!(recovered.is_empty(), "{recovered:?}");
}

#[test]
fn killed_after_the_first_intent_the_generation_completes_on_reopen() {
    crash_then_recover("first-intent", &["prepared"], 0);
}

#[test]
fn killed_before_attaching_the_generation_attaches_on_reopen() {
    crash_then_recover("invoked", &["invoked", "invoked", "invoked"], 0);
}

#[test]
fn killed_after_attaching_the_generation_is_acknowledged_on_reopen() {
    crash_then_recover("attached", &["invoked", "invoked", "invoked"], 3);
}
