//! One worker thread per open project session.
//!
//! The worker thread is the only code that touches a session's
//! `CanonicalProject` and its cached shot workflows. Callers never wait for
//! filesystem work: they enqueue a command and learn its outcome through a
//! [`SessionSink`]. Snapshot and metadata reads come from state the worker
//! publishes after every command, so they never do I/O and never wait for a
//! command to finish.
//!
//! Dropping a [`ProjectSession`] closes it: no sink call starts after the drop
//! begins, queued commands that have not started are discarded, the command
//! already running finishes, and the project (with its single-writer lock) is
//! released before the drop returns.

use std::{
    error::Error,
    fmt,
    panic::{self, AssertUnwindSafe},
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, OnceLock,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use kineto_project::{
    manifest::{CanonicalProject, ProjectManifest, ProjectStoreError},
    shot::{ShotApprovalSnapshot, ShotDirection, ShotWorkflow, ShotWorkflowError},
};

/// Shots addressable in the bootstrap scene.
pub const SHOT_COUNT: usize = 2;

/// How a session obtains its project.
#[derive(Debug)]
pub enum OpenRequest {
    Open {
        path: PathBuf,
    },
    CreateText {
        path: PathBuf,
        manifest: Box<ProjectManifest>,
        source: Vec<u8>,
    },
}

/// A state change to one shot. Every command runs on the session worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShotCommand {
    SetDirection(ShotDirection),
    Generate,
    Select(usize),
    Lock,
    Reset,
    MigrateLegacyHashes,
}

#[derive(Debug)]
pub enum SessionError {
    /// The project could not be opened or created.
    Open(ProjectStoreError),
    /// The shot operation was refused or failed.
    Shot(ShotWorkflowError),
    /// A command arrived for a session whose open had failed.
    NotOpen,
    /// The worker caught a panic. The affected shot is reloaded from its
    /// canonical files before its next use.
    Panicked,
}

impl fmt::Display for SessionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => error.fmt(formatter),
            Self::Shot(error) => error.fmt(formatter),
            Self::NotOpen => formatter.write_str("project session is not open"),
            Self::Panicked => formatter.write_str("project session worker panicked"),
        }
    }
}

impl Error for SessionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open(error) => Some(error),
            Self::Shot(error) => Some(error),
            Self::NotOpen | Self::Panicked => None,
        }
    }
}

/// Receives a session's outcomes on the worker thread.
///
/// Implementations must return quickly and must not call back into the
/// session; they typically post a message to the UI.
pub trait SessionSink: Send + 'static {
    /// The open or create finished. Snapshots and metadata are published
    /// before this is called.
    fn opened(&self, result: &Result<(), SessionError>);

    /// A command finished. The shot's snapshot is published before this is
    /// called.
    fn completed(&self, token: u64, shot_index: u32, result: &Result<(), SessionError>);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectMetadata {
    pub project_id: String,
    pub title: String,
    pub format_version: u32,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitError {
    InvalidShot,
    Closed,
}

#[derive(Debug, Clone)]
pub enum SnapshotError {
    InvalidShot,
    /// The session has not finished opening, or its open failed.
    NotOpen,
    /// The shot's canonical state could not be loaded.
    Failed(Arc<ShotWorkflowError>),
}

type ShotSlot = Option<Result<ShotApprovalSnapshot, Arc<ShotWorkflowError>>>;

#[derive(Default)]
struct View {
    metadata: OnceLock<ProjectMetadata>,
    shots: Mutex<[ShotSlot; SHOT_COUNT]>,
}

struct Gate {
    closed: AtomicBool,
    sink: Mutex<Option<Box<dyn SessionSink>>>,
}

impl Gate {
    /// Run `deliver` unless the session is closed. The sink lock is held for
    /// the whole delivery, so `close` returns only once no delivery is running.
    fn deliver(&self, deliver: impl FnOnce(&dyn SessionSink)) {
        if let Some(sink) = lock(&self.sink).as_deref() {
            deliver(sink);
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        lock(&self.sink).take();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
}

struct Envelope {
    token: u64,
    shot_index: u32,
    command: ShotCommand,
}

/// Owner handle for one project session and its worker thread.
///
/// The handle is `Send + Sync`: any thread may submit commands or read
/// published state. Only the owner may drop it.
pub struct ProjectSession {
    commands: Option<mpsc::Sender<Envelope>>,
    view: Arc<View>,
    gate: Arc<Gate>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ProjectSession {
    /// Start a session. Returns as soon as the worker thread exists; the open
    /// or create runs on the worker and reports through [`SessionSink::opened`].
    pub fn spawn(request: OpenRequest, sink: Box<dyn SessionSink>) -> std::io::Result<Self> {
        let (commands, inbox) = mpsc::channel();
        let view = Arc::new(View::default());
        let gate = Arc::new(Gate {
            closed: AtomicBool::new(false),
            sink: Mutex::new(Some(sink)),
        });
        let worker = thread::Builder::new()
            .name("kineto-project-session".to_owned())
            .spawn({
                let view = Arc::clone(&view);
                let gate = Arc::clone(&gate);
                move || run(request, &inbox, &view, &gate)
            })?;
        Ok(Self {
            commands: Some(commands),
            view,
            gate,
            worker: Some(worker),
        })
    }

    /// Queue a command. Returns without waiting for it to run.
    pub fn submit(
        &self,
        shot_index: u32,
        command: ShotCommand,
        token: u64,
    ) -> Result<(), SubmitError> {
        if !shot_index_is_valid(shot_index) {
            return Err(SubmitError::InvalidShot);
        }
        self.commands
            .as_ref()
            .ok_or(SubmitError::Closed)?
            .send(Envelope {
                token,
                shot_index,
                command,
            })
            .map_err(|_| SubmitError::Closed)
    }

    /// The shot's most recently published snapshot. Never does I/O.
    pub fn snapshot(&self, shot_index: u32) -> Result<ShotApprovalSnapshot, SnapshotError> {
        if !shot_index_is_valid(shot_index) {
            return Err(SnapshotError::InvalidShot);
        }
        match &lock(&self.view.shots)[shot_index as usize] {
            None => Err(SnapshotError::NotOpen),
            Some(Ok(snapshot)) => Ok(*snapshot),
            Some(Err(error)) => Err(SnapshotError::Failed(Arc::clone(error))),
        }
    }

    /// Project metadata, available once the open has succeeded.
    #[must_use]
    pub fn metadata(&self) -> Option<&ProjectMetadata> {
        self.view.metadata.get()
    }
}

impl Drop for ProjectSession {
    fn drop(&mut self) {
        self.gate.close();
        self.commands.take();
        if let Some(worker) = self.worker.take() {
            // The worker finishes at most the command already running, then
            // drops the project and its writer lock.
            let _ = worker.join();
        }
    }
}

fn shot_index_is_valid(shot_index: u32) -> bool {
    usize::try_from(shot_index).is_ok_and(|index| index < SHOT_COUNT)
}

fn run(request: OpenRequest, inbox: &mpsc::Receiver<Envelope>, view: &View, gate: &Gate) {
    let opened = panic::catch_unwind(AssertUnwindSafe(|| open(request)))
        .unwrap_or(Err(SessionError::Panicked));
    let mut worker = match opened {
        Ok(project) => {
            let mut worker = Worker::new(project);
            let project = &worker.project;
            let _ = view.metadata.set(ProjectMetadata {
                project_id: project.manifest().project_id.clone(),
                title: project.manifest().title.clone(),
                format_version: project.manifest().format_version,
                read_only: project.is_read_only(),
            });
            for shot_index in 0..SHOT_COUNT {
                worker.publish(view, shot_index);
            }
            gate.deliver(|sink| sink.opened(&Ok(())));
            Some(worker)
        }
        Err(error) => {
            gate.deliver(|sink| sink.opened(&Err(error)));
            None
        }
    };

    for envelope in inbox {
        if gate.is_closed() {
            break;
        }
        let shot_index = envelope.shot_index as usize;
        let result = match worker.as_mut() {
            None => Err(SessionError::NotOpen),
            Some(worker) => {
                let result = worker.apply(shot_index, envelope.command);
                worker.publish(view, shot_index);
                result
            }
        };
        gate.deliver(|sink| sink.completed(envelope.token, envelope.shot_index, &result));
    }
}

fn open(request: OpenRequest) -> Result<CanonicalProject, SessionError> {
    match request {
        OpenRequest::Open { path } => CanonicalProject::open(path),
        OpenRequest::CreateText {
            path,
            manifest,
            source,
        } => CanonicalProject::create(path, *manifest, &source),
    }
    .map_err(SessionError::Open)
}

struct Worker {
    project: CanonicalProject,
    // Canonical JSON is loaded once per shot per session; reads then stay
    // typed and in memory rather than re-parsing project files.
    shots: [Option<ShotWorkflow>; SHOT_COUNT],
}

impl Worker {
    fn new(project: CanonicalProject) -> Self {
        Self {
            project,
            shots: [None, None],
        }
    }

    fn apply(&mut self, shot_index: usize, command: ShotCommand) -> Result<(), SessionError> {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
            self.with_shot(shot_index, |project, shot| match command {
                ShotCommand::SetDirection(direction) => shot.set_direction(project, direction),
                ShotCommand::Generate => shot.generate(project),
                ShotCommand::Select(candidate) => shot.select(project, candidate),
                ShotCommand::Lock => shot.lock(project),
                ShotCommand::Reset => shot.reset(project),
                ShotCommand::MigrateLegacyHashes => {
                    shot.migrate_legacy_hashes(project).map(|_report| ())
                }
            })
        }));
        match outcome {
            Ok(result) => result.map_err(SessionError::Shot),
            Err(_) => {
                // Do not trust a half-applied in-memory shot; reload it from
                // its canonical files on next use.
                self.shots[shot_index] = None;
                Err(SessionError::Panicked)
            }
        }
    }

    fn publish(&mut self, view: &View, shot_index: usize) {
        let snapshot = panic::catch_unwind(AssertUnwindSafe(|| {
            self.with_shot(shot_index, |_, shot| shot.snapshot())
        }))
        .unwrap_or(Err(ShotWorkflowError::InvalidCanonicalState))
        .map_err(Arc::new);
        lock(&view.shots)[shot_index] = Some(snapshot);
    }

    fn with_shot<R>(
        &mut self,
        shot_index: usize,
        operation: impl FnOnce(&CanonicalProject, &mut ShotWorkflow) -> Result<R, ShotWorkflowError>,
    ) -> Result<R, ShotWorkflowError> {
        if self.shots[shot_index].is_none() {
            let shot_number = u32::try_from(shot_index + 1).expect("shot count fits in u32");
            self.shots[shot_index] = Some(ShotWorkflow::load(&self.project, shot_number)?);
        }
        let shot = self.shots[shot_index]
            .as_mut()
            .expect("shot is installed immediately above");
        operation(&self.project, shot)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectSource, ProjectWorkflow,
    };
    use std::{
        collections::BTreeMap,
        fs,
        sync::{Barrier, mpsc::Receiver},
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    const WAIT: Duration = Duration::from_secs(10);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kineto-session-test-{}-{nonce}",
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
        Opened(Result<(), String>),
        Completed {
            token: u64,
            shot_index: u32,
            result: Result<(), String>,
            thread: String,
        },
    }

    struct ChannelSink(Mutex<mpsc::Sender<Event>>);

    fn describe(result: &Result<(), SessionError>) -> Result<(), String> {
        result
            .as_ref()
            .map(|_| ())
            .map_err(|error| format!("{error:?}"))
    }

    impl SessionSink for ChannelSink {
        fn opened(&self, result: &Result<(), SessionError>) {
            let _ = lock(&self.0).send(Event::Opened(describe(result)));
        }

        fn completed(&self, token: u64, shot_index: u32, result: &Result<(), SessionError>) {
            let _ = lock(&self.0).send(Event::Completed {
                token,
                shot_index,
                result: describe(result),
                thread: thread::current().name().unwrap_or_default().to_owned(),
            });
        }
    }

    fn sink() -> (Box<dyn SessionSink>, Receiver<Event>) {
        let (sender, receiver) = mpsc::channel();
        (Box::new(ChannelSink(Mutex::new(sender))), receiver)
    }

    fn manifest() -> ProjectManifest {
        ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: "session_test".to_owned(),
            title: "Session Test".to_owned(),
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
        }
    }

    fn create(path: &std::path::Path) -> (ProjectSession, Receiver<Event>) {
        let (sink, events) = sink();
        let session = ProjectSession::spawn(
            OpenRequest::CreateText {
                path: path.to_path_buf(),
                manifest: Box::new(manifest()),
                source: b"A filmmaker waits across the table.\n".to_vec(),
            },
            sink,
        )
        .unwrap();
        assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
        (session, events)
    }

    fn completion(events: &Receiver<Event>, token: u64) -> Result<(), String> {
        match events.recv_timeout(WAIT).unwrap() {
            Event::Completed {
                token: actual,
                result,
                thread,
                ..
            } => {
                assert_eq!(actual, token);
                assert_eq!(thread, "kineto-project-session");
                result
            }
            other => panic!("expected completion {token}, got {other:?}"),
        }
    }

    #[test]
    fn session_handle_is_shareable_across_threads() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProjectSession>();
    }

    #[test]
    fn commands_run_in_order_on_the_worker_and_publish_snapshots() {
        let temp = TempDir::new();
        let (session, events) = create(&temp.0.join("film"));

        let metadata = session.metadata().unwrap();
        assert_eq!(metadata.project_id, "session_test");
        assert!(!metadata.read_only);
        assert!(!session.snapshot(0).unwrap().generated);
        assert!(!session.snapshot(1).unwrap().generated);

        session.submit(0, ShotCommand::Generate, 1).unwrap();
        session.submit(0, ShotCommand::Select(2), 2).unwrap();
        session.submit(0, ShotCommand::Lock, 3).unwrap();
        for token in 1..=3 {
            assert_eq!(completion(&events, token), Ok(()));
        }

        let snapshot = session.snapshot(0).unwrap();
        assert!(snapshot.generated && snapshot.locked);
        assert_eq!(snapshot.selected_index, Some(2));
        assert!(!session.snapshot(1).unwrap().generated);
    }

    #[test]
    fn submit_returns_before_the_worker_runs_the_command() {
        struct GatedSink {
            events: Mutex<mpsc::Sender<Event>>,
            release: Arc<Barrier>,
        }
        impl SessionSink for GatedSink {
            fn opened(&self, result: &Result<(), SessionError>) {
                // Hold the worker here until the test has queued commands.
                self.release.wait();
                let _ = lock(&self.events).send(Event::Opened(describe(result)));
            }
            fn completed(&self, token: u64, shot_index: u32, result: &Result<(), SessionError>) {
                let _ = lock(&self.events).send(Event::Completed {
                    token,
                    shot_index,
                    result: describe(result),
                    thread: thread::current().name().unwrap_or_default().to_owned(),
                });
            }
        }

        let temp = TempDir::new();
        let (sender, events) = mpsc::channel();
        let release = Arc::new(Barrier::new(2));
        let session = ProjectSession::spawn(
            OpenRequest::CreateText {
                path: temp.0.join("film"),
                manifest: Box::new(manifest()),
                source: b"source\n".to_vec(),
            },
            Box::new(GatedSink {
                events: Mutex::new(sender),
                release: Arc::clone(&release),
            }),
        )
        .unwrap();

        // The worker is parked inside `opened`; queuing still returns.
        session.submit(0, ShotCommand::Generate, 7).unwrap();
        assert!(events.try_recv().is_err());

        release.wait();
        assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
        assert_eq!(completion(&events, 7), Ok(()));
    }

    #[test]
    fn refused_commands_report_the_shot_error() {
        let temp = TempDir::new();
        let (session, events) = create(&temp.0.join("film"));

        session.submit(1, ShotCommand::Lock, 9).unwrap();
        let result = completion(&events, 9);
        assert!(
            result.as_ref().unwrap_err().contains("NoSelection"),
            "{result:?}"
        );
        assert_eq!(
            session.submit(2, ShotCommand::Generate, 10),
            Err(SubmitError::InvalidShot)
        );
        assert!(matches!(
            session.snapshot(2),
            Err(SnapshotError::InvalidShot)
        ));
    }

    #[test]
    fn failed_open_is_reported_and_later_commands_are_refused() {
        let temp = TempDir::new();
        let (sink, events) = sink();
        let session = ProjectSession::spawn(
            OpenRequest::Open {
                path: temp.0.join("missing"),
            },
            sink,
        )
        .unwrap();

        match events.recv_timeout(WAIT).unwrap() {
            Event::Opened(Err(error)) => assert!(error.contains("Open"), "{error}"),
            other => panic!("expected failed open, got {other:?}"),
        }
        assert!(session.metadata().is_none());
        assert!(matches!(session.snapshot(0), Err(SnapshotError::NotOpen)));

        session.submit(0, ShotCommand::Generate, 4).unwrap();
        assert_eq!(completion(&events, 4), Err("NotOpen".to_owned()));
    }

    #[test]
    fn a_second_writer_session_is_refused_while_the_first_is_open() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let (first, _events) = create(&target);

        let (sink, events) = sink();
        let _second = ProjectSession::spawn(
            OpenRequest::Open {
                path: target.clone(),
            },
            sink,
        )
        .unwrap();
        match events.recv_timeout(WAIT).unwrap() {
            Event::Opened(Err(error)) => assert!(error.contains("Busy"), "{error}"),
            other => panic!("expected Busy, got {other:?}"),
        }
        drop(first);
    }

    #[test]
    fn dropping_the_session_stops_delivery_and_releases_the_project() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let (session, events) = create(&target);

        for token in 0..50 {
            session.submit(0, ShotCommand::Generate, token).unwrap();
        }
        drop(session);

        // Nothing is delivered after the drop returns, and at most the
        // commands that ran before it were reported.
        let delivered = events.try_iter().count();
        assert!(delivered <= 50);
        assert!(events.recv_timeout(Duration::from_millis(200)).is_err());

        // The writer lock is already released: a new session opens at once.
        let (sink, reopened_events) = sink();
        let reopened = ProjectSession::spawn(OpenRequest::Open { path: target }, sink).unwrap();
        assert_eq!(
            reopened_events.recv_timeout(WAIT).unwrap(),
            Event::Opened(Ok(()))
        );
        // The command running at the drop may finish without being reported,
        // so at most one more generation ran than was delivered.
        let generations = usize::from(reopened.snapshot(0).unwrap().generation_revision);
        assert!(
            (delivered..=delivered + 1).contains(&generations),
            "delivered {delivered}, generations {generations}"
        );
        assert!(
            generations < 50,
            "queued commands must not all run after close"
        );
    }
}
