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
    fs::ProjectRoot,
    manifest::{CanonicalProject, ProjectManifest, ProjectStoreError},
    shot::{CandidateOrigin, ShotApprovalSnapshot, ShotDirection, ShotWorkflow, ShotWorkflowError},
};

use crate::{
    generation::{self, GenerationError, ImageProvider, ImageProviderInfo, NoHooks, Unfinished},
    video::{self, VideoClient, VideoDone, VideoJob},
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
    /// Submit a video from the locked keyframe. Completes once MiniMax has
    /// accepted the task; the video arrives later through
    /// [`SessionSink::video_finished`].
    GenerateVideo,
    LockVideo,
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
    /// The shot already has a video being generated.
    VideoInFlight,
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
            Self::VideoInFlight => {
                formatter.write_str("this shot's video is still being generated")
            }
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
            Self::NotOpen | Self::VideoInFlight | Self::Panicked => None,
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

    /// A submitted video finished: attached, or failed and retired so it can
    /// be asked for again. The shot's snapshot is published first.
    fn video_finished(&self, shot_index: u32, revision: u16, result: &Result<(), SessionError>);
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
    /// Absolute path of the current video's media, if any.
    video: Option<PathBuf>,
    /// A video for this shot is being generated.
    video_in_flight: bool,
    /// Who made each current candidate, in candidate order.
    origins: Vec<CandidateOrigin>,
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
        Self::spawn_with(request, provider, sink, SessionOptions::default())
    }

    /// [`Self::spawn`] with explicit timing, for tests.
    pub fn spawn_with(
        request: OpenRequest,
        provider: ImageProvider,
        sink: Box<dyn SessionSink>,
        options: SessionOptions,
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
                move || run(request, provider, options, &inbox, &view, &gate)
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
        self.published(shot_index, |published| {
            published.media.get(candidate_index).cloned()
        })?
        .ok_or(SnapshotError::InvalidCandidate)
    }

    /// The provider and model recorded on the shot's candidate
    /// `candidate_index`. Never does I/O, and never answers with the session's
    /// own provider: the candidate may have been made by another.
    pub fn candidate_origin(
        &self,
        shot_index: u32,
        candidate_index: usize,
    ) -> Result<CandidateOrigin, SnapshotError> {
        self.published(shot_index, |published| {
            published.origins.get(candidate_index).cloned()
        })?
        .ok_or(SnapshotError::InvalidCandidate)
    }

    /// Where the shot's current video keeps its media, if it has one. Never
    /// does I/O.
    pub fn video_media(&self, shot_index: u32) -> Result<Option<PathBuf>, SnapshotError> {
        self.published(shot_index, |published| published.video.clone())
    }

    /// Whether a video for the shot is being generated. Never does I/O.
    pub fn video_in_flight(&self, shot_index: u32) -> Result<bool, SnapshotError> {
        self.published(shot_index, |published| published.video_in_flight)
    }

    fn published<R>(
        &self,
        shot_index: u32,
        read: impl FnOnce(&PublishedShot) -> R,
    ) -> Result<R, SnapshotError> {
        if !shot_index_is_valid(shot_index) {
            return Err(SnapshotError::InvalidShot);
        }
        match &lock(&self.view.shots)[shot_index as usize] {
            None => Err(SnapshotError::NotOpen),
            Some(Ok(published)) => Ok(read(published)),
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
    options: SessionOptions,
    inbox: &mpsc::Receiver<Envelope>,
    view: &View,
    gate: &Arc<Gate>,
) {
    let opened = panic::catch_unwind(AssertUnwindSafe(|| open(request)))
        .unwrap_or(Err(SessionError::Panicked));
    let mut worker = match opened {
        Ok(project) => {
            let mut worker = Worker::new(project, provider, Arc::clone(gate), options);
            // Finish what an earlier session left before accepting commands.
            let mut recoveries = panic::catch_unwind(AssertUnwindSafe(|| worker.recover()))
                .unwrap_or_else(|_| {
                    worker.shots = [None, None];
                    vec![Recovery::NeedsAttention { count: 1 }]
                });
            // Videos submitted earlier are watched again, never resubmitted.
            let unplaced =
                panic::catch_unwind(AssertUnwindSafe(|| worker.resume_videos())).unwrap_or(1);
            if unplaced > 0 {
                recoveries.push(Recovery::NeedsAttention { count: unplaced });
            }
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

    loop {
        // Wake regularly so videos that finished in the background are
        // attached even while no command arrives.
        let envelope = match inbox.recv_timeout(VIDEO_CHECK_INTERVAL) {
            Ok(envelope) => Some(envelope),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if gate.is_closed() {
            break;
        }
        if let Some(envelope) = envelope {
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
        if let Some(worker) = worker.as_mut() {
            for (shot_index, revision, result) in worker.finish_videos() {
                worker.publish(view, shot_index);
                let shot_index = u32::try_from(shot_index).expect("shot index fits in u32");
                gate.deliver(|sink| sink.video_finished(shot_index, revision, &result));
            }
        }
    }
    if let Some(worker) = worker {
        // Watchers see the closed gate within their wake interval; the
        // project and its writer lock outlive every one of them.
        worker.videos.join();
    }
}

/// Timing a session runs with.
#[derive(Debug, Clone, Copy)]
pub struct SessionOptions {
    /// How often a video's remote task is queried.
    pub video_poll_interval: std::time::Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            video_poll_interval: video::POLL_INTERVAL,
        }
    }
}

/// How often the worker collects videos finished in the background.
const VIDEO_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Videos being generated for this session's shots.
struct Videos {
    client: Option<VideoClient>,
    gate: Arc<Gate>,
    in_flight: [Option<VideoJob>; SHOT_COUNT],
    finished: Arc<Mutex<Vec<VideoDone>>>,
    watchers: Vec<thread::JoinHandle<()>>,
    poll_interval: std::time::Duration,
}

impl Videos {
    /// Follow `job` on its own thread until its video is on disk or failed.
    fn watch(&mut self, job: VideoJob, root: ProjectRoot) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let Ok(shot_index) = usize::try_from(job.shot_number - 1) else {
            return;
        };
        if shot_index >= SHOT_COUNT {
            return;
        }
        self.in_flight[shot_index] = Some(job.clone());
        let gate = Arc::clone(&self.gate);
        let finished = Arc::clone(&self.finished);
        let interval = self.poll_interval;
        let spawned = thread::Builder::new()
            .name("kineto-video-watch".to_owned())
            .spawn(move || {
                if let Some(done) = video::watch(&client, &root, job, &gate.closed, interval) {
                    lock(&finished).push(done);
                }
            });
        match spawned {
            Ok(handle) => self.watchers.push(handle),
            // Without a watcher the task is followed again on the next open.
            Err(_) => self.in_flight[shot_index] = None,
        }
    }

    fn join(self) {
        for watcher in self.watchers {
            let _ = watcher.join();
        }
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
    videos: Videos,
    // Canonical JSON is loaded once per shot per session; reads then stay
    // typed and in memory rather than re-parsing project files.
    shots: [Option<ShotWorkflow>; SHOT_COUNT],
}

impl Worker {
    fn new(
        project: CanonicalProject,
        provider: ImageProvider,
        gate: Arc<Gate>,
        options: SessionOptions,
    ) -> Self {
        // Video uses the MiniMax key the image provider was given.
        let client = match &provider {
            ImageProvider::Minimax(client) => client.clone(),
            ImageProvider::LocalDemo => None,
        };
        Self {
            project,
            provider,
            videos: Videos {
                client,
                gate,
                in_flight: [None, None],
                finished: Arc::new(Mutex::new(Vec::new())),
                watchers: Vec::new(),
                poll_interval: options.video_poll_interval,
            },
            shots: [None, None],
        }
    }

    /// Attach (or retire) every video whose watcher has finished.
    fn finish_videos(&mut self) -> Vec<(usize, u16, Result<(), SessionError>)> {
        let finished = std::mem::take(&mut *lock(&self.videos.finished));
        let mut outcomes = Vec::with_capacity(finished.len());
        for done in finished {
            let Some(shot_index) = usize::try_from(done.job.shot_number)
                .ok()
                .and_then(|number| number.checked_sub(1))
                .filter(|index| *index < SHOT_COUNT)
            else {
                continue;
            };
            self.videos.in_flight[shot_index] = None;
            let revision = done.job.revision;
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                self.with_shot(shot_index, |project, _, shot| {
                    video::finish(project, shot, done)
                })
            }))
            .unwrap_or(Err(GenerationError::Unexpected(
                "attaching a video panicked",
            )));
            if result.is_err() {
                self.shots[shot_index] = None;
            }
            outcomes.push((
                shot_index,
                revision,
                result.map(|_| ()).map_err(generation_error),
            ));
        }
        outcomes
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
            ShotCommand::GenerateVideo => self.start_video(shot_index),
            command => self
                .with_shot(shot_index, |project, _, shot| match command {
                    ShotCommand::SetDirection(direction) => shot.set_direction(project, direction),
                    ShotCommand::Select(candidate) => shot.select(project, candidate),
                    ShotCommand::Lock => shot.lock(project),
                    ShotCommand::Reset => shot.reset(project),
                    ShotCommand::MigrateLegacyHashes => {
                        shot.migrate_legacy_hashes(project).map(|_report| ())
                    }
                    ShotCommand::LockVideo => shot.lock_video(project),
                    ShotCommand::Generate | ShotCommand::GenerateVideo => {
                        unreachable!("generation is handled above")
                    }
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

    fn start_video(&mut self, shot_index: usize) -> Result<(), SessionError> {
        if self.videos.in_flight[shot_index].is_some() {
            return Err(SessionError::VideoInFlight);
        }
        let Some(client) = self.videos.client.clone() else {
            return Err(SessionError::Generation(GenerationError::NotConfigured));
        };
        let job = self
            .with_shot(shot_index, |project, _, shot| {
                video::start(project, shot, &client)
            })
            .map_err(generation_error)?;
        let root = self.project.root().clone();
        self.videos.watch(job, root);
        Ok(())
    }

    /// Watch again the videos an earlier session submitted. Nothing is
    /// submitted; without a key they wait for a session that has one.
    fn resume_videos(&mut self) -> usize {
        if self.project.is_read_only() || self.videos.client.is_none() {
            return 0;
        }
        match video::in_flight(&self.project) {
            Ok(found) => {
                for job in found.jobs {
                    let root = self.project.root().clone();
                    self.videos.watch(job, root);
                }
                found.needs_attention
            }
            Err(_) => 1,
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
            ..
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
                    video: shot
                        .video_media()?
                        .map(|path| project.root().resolve(&path)),
                    video_in_flight: false,
                    origins: shot.candidate_origins()?,
                })
            })
        }))
        .unwrap_or(Err(ShotWorkflowError::InvalidCanonicalState))
        .map(|mut published| {
            published.video_in_flight = self.videos.in_flight[shot_index].is_some();
            published
        })
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
        VideoFinished {
            shot_index: u32,
            revision: u16,
            result: Result<(), String>,
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

        fn video_finished(
            &self,
            shot_index: u32,
            revision: u16,
            result: &Result<(), SessionError>,
        ) {
            let _ = lock(&self.0).send(Event::VideoFinished {
                shot_index,
                revision,
                result: describe(result),
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
                    video_cost_micros: 0,
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
                video_cost_micros: 280_000,
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

            fn get(
                &self,
                _url: &str,
                _api_key: &SecretString,
            ) -> Result<HttpResponse, TransportFailure> {
                unreachable!("image generation makes no GET request")
            }

            fn download(
                &self,
                _url: &str,
                _max_bytes: u64,
            ) -> Result<HttpResponse, TransportFailure> {
                unreachable!("image generation downloads nothing")
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
        assert_eq!(session.image_provider().kind, ImageProviderKind::LocalDemo);
        for candidate in 0..3 {
            let media = session.candidate_media(0, candidate).unwrap().unwrap();
            assert!(
                media.is_absolute() && media.is_file(),
                "{}",
                media.display()
            );
            // The candidates were made by MiniMax, whoever the session uses now.
            assert_eq!(
                session.candidate_origin(0, candidate).unwrap(),
                CandidateOrigin {
                    provider: "minimax".to_owned(),
                    model: "image-01".to_owned(),
                }
            );
        }
        assert!(matches!(
            session.candidate_media(0, 3),
            Err(SnapshotError::InvalidCandidate)
        ));
        assert!(matches!(
            session.candidate_origin(0, 3),
            Err(SnapshotError::InvalidCandidate)
        ));
        assert!(matches!(
            session.candidate_media(2, 0),
            Err(SnapshotError::InvalidShot)
        ));
    }

    mod video_flow {
        use super::*;
        use kineto_provider_minimax::{
            HttpResponse, MinimaxClient, SharedTransport, Transport, TransportFailure,
        };
        use kineto_providers::secret::SecretString;
        use std::sync::atomic::{AtomicBool, AtomicU32};

        const MP4: &[u8] = b"\0\0\0\x18ftypisom\0\0\x02\0isommp41-video";

        /// MiniMax for images and Hailuo video, offline. A video task stays
        /// processing until `finish` is set.
        #[derive(Default)]
        struct FakeMinimax {
            images: AtomicU32,
            submits: AtomicU32,
            finish: AtomicBool,
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
                body: &[u8],
            ) -> Result<HttpResponse, TransportFailure> {
                if url.ends_with("/v1/video_generation") {
                    self.submits.fetch_add(1, Ordering::SeqCst);
                    let sent: serde_json::Value = serde_json::from_slice(body).unwrap();
                    assert!(
                        sent["first_frame_image"]
                            .as_str()
                            .unwrap()
                            .starts_with("data:image/jpeg;base64,")
                    );
                    assert!(sent["prompt"].as_str().unwrap().contains("[Push in]"));
                    return answer(r#"{"task_id":"777","base_resp":{"status_code":0}}"#);
                }
                let call = self.images.fetch_add(1, Ordering::SeqCst);
                let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x51, u8::try_from(call).unwrap()];
                use base64::Engine as _;
                answer(format!(
                    r#"{{"id":"i","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                    base64::engine::general_purpose::STANDARD.encode(jpeg)
                ))
            }

            fn get(
                &self,
                url: &str,
                _api_key: &SecretString,
            ) -> Result<HttpResponse, TransportFailure> {
                if url.contains("/v1/query/video_generation?task_id=777") {
                    return answer(if self.finish.load(Ordering::SeqCst) {
                        r#"{"status":"Success","file_id":"999","base_resp":{"status_code":0}}"#
                    } else {
                        r#"{"status":"Processing","file_id":"","base_resp":{"status_code":0}}"#
                    });
                }
                assert!(url.ends_with("/v1/files/retrieve?file_id=999"), "{url}");
                answer(
                    r#"{"file":{"download_url":"https://cdn.example.test/v.mp4"},"base_resp":{"status_code":0}}"#,
                )
            }

            fn download(
                &self,
                url: &str,
                _max_bytes: u64,
            ) -> Result<HttpResponse, TransportFailure> {
                assert_eq!(url, "https://cdn.example.test/v.mp4");
                answer(MP4)
            }
        }

        fn minimax(fake: &Arc<FakeMinimax>) -> ImageProvider {
            let transport: SharedTransport = Arc::clone(fake) as SharedTransport;
            ImageProvider::Minimax(Some(MinimaxClient::with_transport(
                transport,
                SecretString::new("sk-test".to_owned()).unwrap(),
                "https://example.test",
            )))
        }

        fn options() -> SessionOptions {
            SessionOptions {
                video_poll_interval: Duration::from_millis(20),
            }
        }

        fn spawn(
            request: OpenRequest,
            fake: &Arc<FakeMinimax>,
        ) -> (ProjectSession, Receiver<Event>) {
            let (sink, events) = sink();
            let session =
                ProjectSession::spawn_with(request, minimax(fake), sink, options()).unwrap();
            (session, events)
        }

        fn command(
            session: &ProjectSession,
            events: &Receiver<Event>,
            token: u64,
            command: ShotCommand,
        ) -> Result<(), String> {
            session.submit(0, command, token).unwrap();
            completion(events, token)
        }

        fn locked_keyframe(
            target: &std::path::Path,
            fake: &Arc<FakeMinimax>,
        ) -> (ProjectSession, Receiver<Event>) {
            let (session, events) = spawn(
                OpenRequest::CreateText {
                    path: target.to_path_buf(),
                    manifest: Box::new(manifest()),
                    source: b"A filmmaker waits.\n".to_vec(),
                },
                fake,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            command(&session, &events, 1, ShotCommand::Generate).unwrap();
            command(&session, &events, 2, ShotCommand::Select(1)).unwrap();
            command(&session, &events, 3, ShotCommand::Lock).unwrap();
            (session, events)
        }

        #[test]
        fn a_video_is_generated_in_the_background_and_can_be_locked() {
            let temp = TempDir::new();
            let fake = Arc::new(FakeMinimax::default());
            let (session, events) = locked_keyframe(&temp.0.join("film"), &fake);

            command(&session, &events, 4, ShotCommand::GenerateVideo).unwrap();
            assert!(session.video_in_flight(0).unwrap());
            // Other work goes on while the video is generated.
            assert_eq!(
                command(&session, &events, 5, ShotCommand::GenerateVideo),
                Err("VideoInFlight".to_owned())
            );
            session.submit(1, ShotCommand::Generate, 6).unwrap();
            assert_eq!(completion(&events, 6), Ok(()));

            fake.finish.store(true, Ordering::SeqCst);
            assert_eq!(
                events.recv_timeout(WAIT).unwrap(),
                Event::VideoFinished {
                    shot_index: 0,
                    revision: 1,
                    result: Ok(())
                }
            );
            assert!(!session.video_in_flight(0).unwrap());
            assert_eq!(
                session.snapshot(0).unwrap().video,
                kineto_project::shot::VideoState::Ready
            );
            let media = session.video_media(0).unwrap().unwrap();
            assert_eq!(fs::read(&media).unwrap(), MP4);

            command(&session, &events, 7, ShotCommand::LockVideo).unwrap();
            assert_eq!(
                session.snapshot(0).unwrap().video,
                kineto_project::shot::VideoState::Locked
            );
            assert_eq!(fake.submits.load(Ordering::SeqCst), 1);
        }

        #[test]
        fn a_reopened_project_finishes_its_video_without_submitting_again() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeMinimax::default());
            let (session, events) = locked_keyframe(&target, &fake);
            command(&session, &events, 4, ShotCommand::GenerateVideo).unwrap();
            drop(session);
            assert_eq!(fake.submits.load(Ordering::SeqCst), 1);

            fake.finish.store(true, Ordering::SeqCst);
            let (reopened, events) = spawn(OpenRequest::Open { path: target }, &fake);
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(
                events.recv_timeout(WAIT).unwrap(),
                Event::VideoFinished {
                    shot_index: 0,
                    revision: 1,
                    result: Ok(())
                }
            );
            assert_eq!(
                reopened.snapshot(0).unwrap().video,
                kineto_project::shot::VideoState::Ready
            );
            assert_eq!(
                fake.submits.load(Ordering::SeqCst),
                1,
                "never submitted again"
            );
        }

        #[test]
        fn an_accepted_submission_whose_intent_was_not_saved_is_never_submitted_again() {
            struct FailAfterAccept;
            impl video::VideoHooks for FailAfterAccept {
                fn reached(&mut self, point: video::VideoPoint) {
                    assert_eq!(point, video::VideoPoint::Accepted);
                    panic!("the intent save fails after MiniMax accepted the task");
                }
            }

            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeMinimax::default());
            drop(locked_keyframe(&target, &fake));

            let project = CanonicalProject::open(&target).unwrap();
            let shot = ShotWorkflow::load(&project, 1).unwrap();
            let ImageProvider::Minimax(Some(client)) = minimax(&fake) else {
                unreachable!();
            };
            let failed = panic::catch_unwind(AssertUnwindSafe(|| {
                video::start_with(&project, &shot, &client, &mut FailAfterAccept)
            }));
            assert!(failed.is_err());
            assert_eq!(fake.submits.load(Ordering::SeqCst), 1);
            assert_eq!(video_intent(&target)["state"], "prepared");
            assert_eq!(video_receipt(&target)["task_id"], "777");

            // Asking again follows the accepted task.
            let job = video::start(&project, &shot, &client).unwrap();
            assert_eq!(job.task.as_str(), "777");
            assert_eq!(fake.submits.load(Ordering::SeqCst), 1, "never paid twice");
            let intent = video_intent(&target);
            assert_eq!(intent["state"], "invoked");
            assert_eq!(intent["provider_job_id"], "777");
        }

        #[test]
        fn a_reopened_project_follows_a_task_its_intent_never_recorded() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeMinimax::default());
            drop(locked_keyframe(&target, &fake));
            {
                let project = CanonicalProject::open(&target).unwrap();
                let shot = ShotWorkflow::load(&project, 1).unwrap();
                let ImageProvider::Minimax(Some(client)) = minimax(&fake) else {
                    unreachable!();
                };
                video::start(&project, &shot, &client).unwrap();
            }
            assert_eq!(fake.submits.load(Ordering::SeqCst), 1);
            // The state a crash between acceptance and the intent save leaves.
            let path = video_intent_path(&target);
            let mut intent = video_intent(&target);
            intent["state"] = "prepared".into();
            intent["provider_job_id"] = serde_json::Value::Null;
            fs::write(&path, serde_json::to_vec_pretty(&intent).unwrap()).unwrap();

            fake.finish.store(true, Ordering::SeqCst);
            let (reopened, events) = spawn(
                OpenRequest::Open {
                    path: target.clone(),
                },
                &fake,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(
                events.recv_timeout(WAIT).unwrap(),
                Event::VideoFinished {
                    shot_index: 0,
                    revision: 1,
                    result: Ok(())
                }
            );
            assert_eq!(
                reopened.snapshot(0).unwrap().video,
                kineto_project::shot::VideoState::Ready
            );
            assert_eq!(fake.submits.load(Ordering::SeqCst), 1, "never paid twice");
        }

        fn video_intent_path(target: &std::path::Path) -> PathBuf {
            fs::read_dir(target.join(".kineto/jobs/intents"))
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .find(|path| {
                    let intent: serde_json::Value =
                        serde_json::from_slice(&fs::read(path).unwrap()).unwrap_or_default();
                    intent["provider"] == video::VIDEO_PROVIDER
                })
                .expect("a video intent exists")
        }

        fn video_intent(target: &std::path::Path) -> serde_json::Value {
            serde_json::from_slice(&fs::read(video_intent_path(target)).unwrap()).unwrap()
        }

        fn video_receipt(target: &std::path::Path) -> serde_json::Value {
            let artifact = video_intent(target)["artifact_id"]
                .as_str()
                .unwrap()
                .to_owned();
            serde_json::from_slice(
                &fs::read(target.join(format!(".kineto/generation/{artifact}.json"))).unwrap(),
            )
            .unwrap()
        }

        #[test]
        fn a_video_needs_a_real_locked_keyframe() {
            let temp = TempDir::new();
            let (session, events) = create(&temp.0.join("film"));
            // The demo session has no video provider.
            assert!(
                command(&session, &events, 1, ShotCommand::GenerateVideo)
                    .unwrap_err()
                    .contains("NotConfigured")
            );
            let fake = Arc::new(FakeMinimax::default());
            let (session, events) = spawn(
                OpenRequest::CreateText {
                    path: temp.0.join("second"),
                    manifest: Box::new(manifest()),
                    source: b"source\n".to_vec(),
                },
                &fake,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert!(
                command(&session, &events, 1, ShotCommand::GenerateVideo)
                    .unwrap_err()
                    .contains("KeyframeNotLocked")
            );
            assert_eq!(fake.submits.load(Ordering::SeqCst), 0);
            drop(session);
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

            fn video_finished(&self, _: u32, _: u16, _: &Result<(), SessionError>) {}
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
