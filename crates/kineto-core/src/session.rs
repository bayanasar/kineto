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

use crate::generation::{
    self, GenerationError, ImageProvider, ImageProviderInfo, NoHooks, Unfinished,
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
    /// A generation job could not finish. Its write-ahead intents remain and
    /// the generation is completed when the project is next opened.
    Generation(GenerationError),
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
            Self::Generation(error) => error.fmt(formatter),
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
            Self::Generation(error) => Some(error),
            Self::NotOpen | Self::Panicked => None,
        }
    }
}

/// Work an earlier session left unfinished, dealt with when this session
/// opened and before any command runs.
#[derive(Debug)]
pub enum Recovery {
    /// An interrupted generation of this shot was completed.
    Completed { shot_index: u32, revision: u16 },
    /// An interrupted generation of this shot could not be completed. Its
    /// intents remain; the next open tries again.
    Failed {
        shot_index: u32,
        revision: u16,
        error: SessionError,
    },
    /// Job records that could not be read, placed or scanned. Nothing was
    /// changed; a person needs to look at them.
    NeedsAttention { count: usize },
}

/// Receives a session's outcomes on the worker thread.
///
/// Implementations must return quickly and must not call back into the
/// session; they typically post a message to the UI.
pub trait SessionSink: Send + 'static {
    /// Unfinished work from an earlier session was dealt with. Called before
    /// [`SessionSink::opened`], once per recovery.
    fn recovered(&self, recovery: &Recovery);

    /// The open or create finished. Snapshots and metadata are published and
    /// recoveries delivered before this is called.
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
    /// The shot has no candidate at that index.
    InvalidCandidate,
    /// The session has not finished opening, or its open failed.
    NotOpen,
    /// The shot's canonical state could not be loaded.
    Failed(Arc<ShotWorkflowError>),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidShot => formatter.write_str("shot index is out of range"),
            Self::InvalidCandidate => formatter.write_str("candidate index is out of range"),
            Self::NotOpen => formatter.write_str("project session is not open"),
            Self::Failed(error) => error.fmt(formatter),
        }
    }
}

impl Error for SnapshotError {}

/// What the worker last published for a shot. Reads never do I/O.
struct PublishedShot {
    snapshot: ShotApprovalSnapshot,
    /// Absolute path of each current candidate's media, in candidate order.
    media: Vec<Option<PathBuf>>,
}

type ShotSlot = Option<Result<PublishedShot, Arc<ShotWorkflowError>>>;

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
    image_provider: ImageProviderInfo,
    view: Arc<View>,
    gate: Arc<Gate>,
    worker: Option<thread::JoinHandle<()>>,
}

impl ProjectSession {
    /// Start a session. Returns as soon as the worker thread exists; the open
    /// or create runs on the worker and reports through [`SessionSink::opened`].
    pub fn spawn(
        request: OpenRequest,
        provider: ImageProvider,
        sink: Box<dyn SessionSink>,
    ) -> std::io::Result<Self> {
        let image_provider = provider.info();
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
                move || run(request, provider, &inbox, &view, &gate)
            })?;
        Ok(Self {
            commands: Some(commands),
            image_provider,
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
            Some(Ok(published)) => Ok(published.snapshot),
            Some(Err(error)) => Err(SnapshotError::Failed(Arc::clone(error))),
        }
    }

    /// Where the shot's candidate `candidate_index` keeps its media, or
    /// `None` for a candidate without media. Never does I/O.
    pub fn candidate_media(
        &self,
        shot_index: u32,
        candidate_index: usize,
    ) -> Result<Option<PathBuf>, SnapshotError> {
        if !shot_index_is_valid(shot_index) {
            return Err(SnapshotError::InvalidShot);
        }
        match &lock(&self.view.shots)[shot_index as usize] {
            None => Err(SnapshotError::NotOpen),
            Some(Ok(published)) => published
                .media
                .get(candidate_index)
                .cloned()
                .ok_or(SnapshotError::InvalidCandidate),
            Some(Err(error)) => Err(SnapshotError::Failed(Arc::clone(error))),
        }
    }

    /// The image provider this session generates with. Fixed at spawn.
    #[must_use]
    pub const fn image_provider(&self) -> ImageProviderInfo {
        self.image_provider
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

fn run(
    request: OpenRequest,
    provider: ImageProvider,
    inbox: &mpsc::Receiver<Envelope>,
    view: &View,
    gate: &Gate,
) {
    let opened = panic::catch_unwind(AssertUnwindSafe(|| open(request)))
        .unwrap_or(Err(SessionError::Panicked));
    let mut worker = match opened {
        Ok(project) => {
            let mut worker = Worker::new(project, provider);
            // Finish what an earlier session left before accepting commands.
            let recoveries = panic::catch_unwind(AssertUnwindSafe(|| worker.recover()))
                .unwrap_or_else(|_| {
                    worker.shots = [None, None];
                    vec![Recovery::NeedsAttention { count: 1 }]
                });
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
            for recovery in &recoveries {
                gate.deliver(|sink| sink.recovered(recovery));
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
        } => CanonicalProject::create_with_files(
            path,
            *manifest,
            &source,
            &crate::bootstrap::scene_files(),
        ),
    }
    .map_err(SessionError::Open)
}

struct Worker {
    project: CanonicalProject,
    provider: ImageProvider,
    // Canonical JSON is loaded once per shot per session; reads then stay
    // typed and in memory rather than re-parsing project files.
    shots: [Option<ShotWorkflow>; SHOT_COUNT],
}

impl Worker {
    fn new(project: CanonicalProject, provider: ImageProvider) -> Self {
        Self {
            project,
            provider,
            shots: [None, None],
        }
    }

    fn apply(&mut self, shot_index: usize, command: ShotCommand) -> Result<(), SessionError> {
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| match command {
            // Generation runs as write-ahead jobs so an interruption is
            // recoverable; every other command is a single canonical update.
            ShotCommand::Generate => self
                .with_shot(shot_index, |project, provider, shot| {
                    generation::generate(project, shot, provider, &mut NoHooks).map(|_outcome| ())
                })
                .map_err(generation_error),
            command => self
                .with_shot(shot_index, |project, _, shot| match command {
                    ShotCommand::SetDirection(direction) => shot.set_direction(project, direction),
                    ShotCommand::Select(candidate) => shot.select(project, candidate),
                    ShotCommand::Lock => shot.lock(project),
                    ShotCommand::Reset => shot.reset(project),
                    ShotCommand::MigrateLegacyHashes => {
                        shot.migrate_legacy_hashes(project).map(|_report| ())
                    }
                    ShotCommand::Generate => unreachable!("generation is handled above"),
                })
                .map_err(SessionError::Shot),
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => {
                // Do not trust a half-applied in-memory shot; reload it from
                // its canonical files on next use.
                self.shots[shot_index] = None;
                Err(SessionError::Panicked)
            }
        }
    }

    /// Complete generations an earlier session left unfinished. Read-only
    /// projects are never written, so nothing is recovered in them.
    ///
    /// Each provider's jobs are completed by that provider, whichever one this
    /// session generates with: a session without MiniMax credentials still
    /// completes MiniMax calls that finished before the restart.
    fn recover(&mut self) -> Vec<Recovery> {
        if self.project.is_read_only() {
            return Vec::new();
        }
        let demo = ImageProvider::LocalDemo;
        let keyless = ImageProvider::Minimax(None);
        let Self {
            project,
            provider,
            shots,
        } = self;
        let minimax = match provider {
            ImageProvider::Minimax(_) => &*provider,
            ImageProvider::LocalDemo => &keyless,
        };
        let mut recoveries = Vec::new();
        let mut unplaced = 0;
        for provider in [&demo, minimax] {
            let Unfinished {
                plans,
                needs_attention,
            } = match generation::unfinished(project, provider) {
                Ok(unfinished) => unfinished,
                Err(_) => {
                    unplaced += 1;
                    continue;
                }
            };
            unplaced += needs_attention;
            for plan in plans {
                let Some(shot_index) = usize::try_from(plan.shot_number())
                    .ok()
                    .and_then(|number| number.checked_sub(1))
                    .filter(|index| *index < SHOT_COUNT)
                else {
                    unplaced += 1;
                    continue;
                };
                let result = shot_mut(project, shots, shot_index)
                    .map_err(GenerationError::Shot)
                    .and_then(|shot| {
                        generation::complete(project, shot, &plan, provider, &mut NoHooks)
                    });
                let shot_number = u32::try_from(shot_index).expect("shot index fits in u32");
                recoveries.push(match result {
                    Ok(_outcome) => Recovery::Completed {
                        shot_index: shot_number,
                        revision: plan.revision(),
                    },
                    Err(error) => {
                        shots[shot_index] = None;
                        Recovery::Failed {
                            shot_index: shot_number,
                            revision: plan.revision(),
                            error: generation_error(error),
                        }
                    }
                });
            }
        }
        if unplaced > 0 {
            recoveries.push(Recovery::NeedsAttention { count: unplaced });
        }
        recoveries
    }

    fn publish(&mut self, view: &View, shot_index: usize) {
        let published = panic::catch_unwind(AssertUnwindSafe(|| {
            self.with_shot(shot_index, |project, _, shot| {
                Ok(PublishedShot {
                    snapshot: shot.snapshot()?,
                    media: shot
                        .candidate_media()?
                        .into_iter()
                        .map(|path| path.map(|path| project.root().resolve(&path)))
                        .collect(),
                })
            })
        }))
        .unwrap_or(Err(ShotWorkflowError::InvalidCanonicalState))
        .map_err(Arc::new);
        lock(&view.shots)[shot_index] = Some(published);
    }

    fn with_shot<R, E: From<ShotWorkflowError>>(
        &mut self,
        shot_index: usize,
        operation: impl FnOnce(&CanonicalProject, &ImageProvider, &mut ShotWorkflow) -> Result<R, E>,
    ) -> Result<R, E> {
        let shot = shot_mut(&self.project, &mut self.shots, shot_index)?;
        operation(&self.project, &self.provider, shot)
    }
}

/// The shot's cached workflow, loaded from its canonical files on first use.
fn shot_mut<'a>(
    project: &CanonicalProject,
    shots: &'a mut [Option<ShotWorkflow>; SHOT_COUNT],
    shot_index: usize,
) -> Result<&'a mut ShotWorkflow, ShotWorkflowError> {
    if shots[shot_index].is_none() {
        let shot_number = u32::try_from(shot_index + 1).expect("shot count fits in u32");
        shots[shot_index] = Some(ShotWorkflow::load(project, shot_number)?);
    }
    Ok(shots[shot_index]
        .as_mut()
        .expect("shot is installed immediately above"))
}

fn generation_error(error: GenerationError) -> SessionError {
    match error {
        GenerationError::Shot(error) => SessionError::Shot(error),
        error => SessionError::Generation(error),
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
    use crate::generation::{ImageProviderKind, ProviderSelectionError};
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
            // SystemTime is only microsecond-precise on macOS, so tests running in
            // parallel could otherwise share a directory name.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let nonce = format!(
                "{nonce}-{}",
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
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
        Recovered(String),
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
        fn recovered(&self, recovery: &Recovery) {
            let _ = lock(&self.0).send(Event::Recovered(format!("{recovery:?}")));
        }

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
            ImageProvider::LocalDemo,
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
    fn a_paid_provider_is_used_only_when_named() {
        use kineto_providers::secret::EnvSecretSource;
        let with_key = EnvSecretSource::with_lookup(|_| Some("sk-test".into()));
        let without_key = EnvSecretSource::with_lookup(|_| None);

        for selection in [None, Some(""), Some("local-demo")] {
            let provider = ImageProvider::select(selection, &with_key).unwrap();
            assert_eq!(
                provider.info(),
                ImageProviderInfo {
                    kind: ImageProviderKind::LocalDemo,
                    ready: true,
                    generation_cost_micros: 0,
                }
            );
        }
        let minimax = ImageProvider::select(Some("minimax"), &with_key).unwrap();
        assert_eq!(
            minimax.info(),
            ImageProviderInfo {
                kind: ImageProviderKind::Minimax,
                ready: true,
                generation_cost_micros: 10_500,
            }
        );
        assert!(!format!("{minimax:?}").contains("sk-test"));
        assert!(
            !ImageProvider::select(Some("minimax"), &without_key)
                .unwrap()
                .info()
                .ready
        );
        assert_eq!(
            ImageProvider::select(Some("midjourney"), &with_key).unwrap_err(),
            ProviderSelectionError::Unknown("midjourney".to_owned())
        );
    }

    #[test]
    fn a_session_completes_every_providers_finished_calls_on_open() {
        use crate::minimax::MinimaxKeyframes;
        use kineto_provider_minimax::{HttpResponse, MinimaxClient, Transport, TransportFailure};
        use kineto_providers::secret::SecretString;

        struct OneImage;
        impl Transport for OneImage {
            fn post_json(
                &self,
                _url: &str,
                _api_key: &SecretString,
                _body: &[u8],
            ) -> Result<HttpResponse, TransportFailure> {
                Ok(HttpResponse {
                    status: 200,
                    body:
                        br#"{"data":{"image_base64":["/9j/4AAQ"]},"base_resp":{"status_code":0}}"#
                            .to_vec(),
                })
            }
        }
        struct StopBeforeAttach;
        impl generation::GenerationHooks for StopBeforeAttach {
            fn reached(&mut self, point: generation::GenerationPoint) {
                if point == generation::GenerationPoint::Invoked {
                    panic!("stopped before attaching");
                }
            }
        }

        let temp = TempDir::new();
        let target = temp.0.join("film");
        drop(create(&target));
        {
            let project = CanonicalProject::open(&target).unwrap();
            let mut shot = ShotWorkflow::load(&project, 1).unwrap();
            let client = MinimaxClient::with_transport(
                OneImage,
                SecretString::new("sk-test".to_owned()).unwrap(),
                "https://example.test",
            );
            let stopped = panic::catch_unwind(AssertUnwindSafe(|| {
                generation::generate_with(
                    &project,
                    &mut shot,
                    MinimaxKeyframes::new(&project, Some(&client)),
                    &mut StopBeforeAttach,
                )
            }));
            assert!(stopped.is_err());
        }

        // The demo session has no MiniMax key, and needs none to finish.
        let (sink, events) = sink();
        let session = ProjectSession::spawn(
            OpenRequest::Open { path: target },
            ImageProvider::LocalDemo,
            sink,
        )
        .unwrap();
        assert_eq!(
            events.recv_timeout(WAIT).unwrap(),
            Event::Recovered("Completed { shot_index: 0, revision: 1 }".to_owned())
        );
        assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
        let snapshot = session.snapshot(0).unwrap();
        assert!(snapshot.generated);
        assert_eq!(snapshot.candidate_count, 3);
        for candidate in 0..3 {
            let media = session.candidate_media(0, candidate).unwrap().unwrap();
            assert!(
                media.is_absolute() && media.is_file(),
                "{}",
                media.display()
            );
        }
        assert!(matches!(
            session.candidate_media(0, 3),
            Err(SnapshotError::InvalidCandidate)
        ));
        assert!(matches!(
            session.candidate_media(2, 0),
            Err(SnapshotError::InvalidShot)
        ));
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
            fn recovered(&self, _recovery: &Recovery) {}

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
            ImageProvider::LocalDemo,
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
            ImageProvider::LocalDemo,
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
            ImageProvider::LocalDemo,
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
        let reopened = ProjectSession::spawn(
            OpenRequest::Open { path: target },
            ImageProvider::LocalDemo,
            sink,
        )
        .unwrap();
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
