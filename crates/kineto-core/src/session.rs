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
    collections::{BTreeMap, BTreeSet},
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

use kineto_llm::LlmBackend;
use kineto_project::{
    bible::{self as bible_document, Bible, EntryEdit, StaleCause},
    cast::{self as cast_document, CastError, CastImage, CastImageKind, CharacterLock},
    fs::{ProjectRelativePath, ProjectRoot},
    manifest::{CanonicalProject, ProjectManifest, ProjectStoreError},
    media::{self, MediaRecord},
    revisions::{self, DocumentKind, RevisionError, RevisionRecord},
    screenplay::{self as screenplay_document, SceneEdit, Screenplay},
    shot::{
        self, AttachOutcome, CandidateOrigin, CandidateRecord, ShotApprovalSnapshot, ShotDirection,
        ShotKey, ShotWorkflow, ShotWorkflowError, VideoRecord,
    },
    shot_plan::{self as shot_plan_document, ScenePlan, ShotPlan},
    source::{self, SourceError, SourceFormat},
    story::StoryAnalysis,
    style::{self as style_document, StyleError, StyleLock, StyleSample},
    voice::{self as voice_document, Audition, VoiceError, VoiceLock},
};
use kineto_providers::Price;

use crate::{
    bible, cast,
    documents::{self, DOCUMENT_PROVIDERS},
    export,
    generation::{
        self, Consent, GenerationError, ImageProvider, ImageProviderInfo, NoHooks, Unfinished,
    },
    llm::{DocumentModel, DocumentModelInfo},
    paid_image::{ImageRecovery, Submission},
    screenplay, shot_plan,
    speech::{LocalDemoVoices, SpeechGenerator, SpeechProvider, SpeechProviderInfo, Voice},
    story,
    style::{self, SampleRequest},
    video::{
        self, LocalDemoVideos, VideoAsk, VideoDone, VideoEstimate, VideoGenerator, VideoJob,
        VideoProvider, VideoProviderInfo,
    },
    voice::{self, VoiceChoice},
    workflow,
};

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
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShotCommand {
    SetDirection(ShotDirection),
    Generate,
    Select(usize),
    Lock,
    Reset,
    MigrateLegacyHashes,
    /// Generate again although a paid call of the last attempt may have been
    /// accepted, and charged for, without leaving its result. The director's
    /// explicit decision to risk paying twice.
    ResubmitKeyframes,
    /// Generate with what the director explicitly accepted, such as showing
    /// characters without an identity reference where the project's policy
    /// blocks that.
    GenerateConsenting(Consent),
    /// Generate again as the director's note asks, with what they accepted.
    Refine {
        note: String,
        consent: Consent,
    },
    /// Submit a video from the locked keyframe. Completes once the provider
    /// has accepted the task; the video arrives later through
    /// [`SessionSink::video_finished`].
    GenerateVideo,
    /// Submit the video again although the provider may already have
    /// accepted, and charged for, the last submission that left no record.
    /// The director's explicit decision to risk paying twice.
    ResubmitVideo,
    /// Submit a video generation as the director asks: several candidates,
    /// a note, or a branch from an earlier take. Each candidate arrives
    /// through [`SessionSink::video_finished`].
    GenerateVideos(VideoAsk),
    /// [`Self::GenerateVideos`] at the risk of paying twice for a candidate
    /// whose last submission left no record.
    ResubmitVideos(VideoAsk),
    /// Choose one of the latest generation's videos, by its place among
    /// them, to lock.
    SelectVideo(usize),
    /// Lock the chosen video, or the only one of the latest generation.
    LockVideo,
}

/// A change to the project's documents: its source, story analysis,
/// screenplay, bible and shot plan. Every command runs on the session
/// worker.
#[derive(Debug, Clone, PartialEq)]
pub enum DocumentCommand {
    /// Import new source text as the next source revision.
    ImportSource {
        text: String,
        format: SourceFormat,
        original_name: Option<String>,
    },
    AnalyzeStory,
    WriteScreenplay,
    /// Write one scene of the latest screenplay again.
    RewriteScene {
        scene_id: String,
        direction: Option<String>,
    },
    /// Save the director's edit of one scene of revision `based_on`, which
    /// must be the latest.
    SaveScene {
        based_on: u32,
        edit: SceneEdit,
    },
    LockScreenplay {
        revision: u32,
    },
    /// Write a bible of the locked screenplay.
    WriteBible,
    /// Save the director's edit of one entry of bible revision `based_on`,
    /// which must be the latest.
    SaveBibleEntry {
        based_on: u32,
        edit: EntryEdit,
    },
    LockBible {
        revision: u32,
    },
    /// Plan the shots of the locked screenplay from it and the locked bible.
    PlanShots,
    /// Plan one scene of the latest shot plan again.
    ReplanScene {
        scene_id: String,
        direction: Option<String>,
    },
    /// Save the director's edit of one scene's shots of plan revision
    /// `based_on`, which must be the latest.
    SaveScenePlan {
        based_on: u32,
        scene: ScenePlan,
    },
    LockShotPlan {
        revision: u32,
    },
}

/// A change to the style or the cast. Every command runs on the session
/// worker and completes through [`SessionSink::document_completed`]. Paid
/// commands are refused while an image of theirs may have been charged
/// without its result, unless submitted with
/// [`ProjectSession::resubmit_production`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProductionCommand {
    /// Paid: one sample of a style preset.
    SampleStyle {
        preset: String,
        note: Option<String>,
    },
    /// Paid: a sample of an earlier one's preset, with a note.
    RefineStyle {
        sample: String,
        note: String,
    },
    SelectStyle(String),
    RejectStyle(String),
    LockStyle(String),
    /// Paid: have `count` portrait candidates of `character`, making only
    /// those missing.
    Audition {
        character: String,
        count: u32,
    },
    /// Paid: complete the reference pack of `character`'s selected
    /// portrait, making `remake` again although the pack holds them.
    MakePack {
        character: String,
        remake: Vec<CastImageKind>,
    },
    SelectPortrait {
        character: String,
        portrait: String,
    },
    RejectPortrait {
        character: String,
        portrait: String,
    },
    LockCharacter {
        character: String,
    },
    /// Free: ask the speech provider which voices it offers.
    LoadVoices,
    /// Paid: have each voice of `choices` say `line` as `character`, or
    /// its default line; a voice already heard saying it is not asked
    /// again.
    AuditionVoices {
        character: String,
        choices: Vec<VoiceChoice>,
        line: Option<String>,
    },
    SelectVoice {
        character: String,
        audition: String,
    },
    RejectVoice {
        character: String,
        audition: String,
    },
    LockVoice {
        character: String,
    },
}

/// Why a style, casting or voice change was refused.
#[derive(Debug)]
pub enum ProductionError {
    Style(StyleError),
    Cast(CastError),
    Voice(VoiceError),
}

impl fmt::Display for ProductionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Style(error) => error.fmt(formatter),
            Self::Cast(error) => error.fmt(formatter),
            Self::Voice(error) => error.fmt(formatter),
        }
    }
}

impl Error for ProductionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Style(error) => Some(error),
            Self::Cast(error) => Some(error),
            Self::Voice(error) => Some(error),
        }
    }
}

/// Why a document change was refused.
#[derive(Debug)]
pub enum DocumentError {
    Source(SourceError),
    Revision(RevisionError),
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Source(error) => error.fmt(formatter),
            Self::Revision(error) => error.fmt(formatter),
        }
    }
}

impl Error for DocumentError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Source(error) => Some(error),
            Self::Revision(error) => Some(error),
        }
    }
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
    /// A document change was refused.
    Document(DocumentError),
    /// A style or casting change was refused.
    Production(ProductionError),
    /// The project's media could not be listed or collected.
    Media(media::MediaError),
    /// The production package could not be written.
    Export(export::ExportError),
    /// Collecting was asked for before a dry run showed what it would let
    /// go.
    NoCollectionPlan,
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
            Self::Document(error) => error.fmt(formatter),
            Self::Production(error) => error.fmt(formatter),
            Self::Media(error) => error.fmt(formatter),
            Self::Export(error) => error.fmt(formatter),
            Self::NoCollectionPlan => {
                formatter.write_str("work out what collecting would let go before collecting it")
            }
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
            Self::Document(error) => Some(error),
            Self::Production(error) => Some(error),
            Self::Media(error) => Some(error),
            Self::Export(error) => Some(error),
            Self::NoCollectionPlan | Self::NotOpen | Self::VideoInFlight | Self::Panicked => None,
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
    /// A document a model answered before an interruption was kept as this
    /// revision.
    Document { kind: DocumentKind, revision: u32 },
    /// Paid images MiniMax answered before an interruption were kept.
    Images { kind: ImageWork, count: usize },
}

/// A kind of paid work kept as the provider answers it: images, and the
/// speech of voice auditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageWork {
    StyleSample,
    Casting,
    VoiceAudition,
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

    /// A document command finished. The documents are published before this
    /// is called.
    fn document_completed(&self, token: u64, result: &Result<(), SessionError>);
}

/// The analysis the director's documents build on.
#[derive(Debug, Clone)]
pub struct PublishedAnalysis {
    pub revision: u32,
    pub analysis: StoryAnalysis,
}

/// What the worker last published about the project's documents. Reads
/// never do I/O.
#[derive(Debug, Clone, Default)]
pub struct PublishedDocuments {
    /// The current source revision.
    pub source_revision: Option<u32>,
    /// The current story analysis: the locked one, otherwise the latest.
    pub analysis: Option<PublishedAnalysis>,
    /// Every screenplay revision, oldest first.
    pub revisions: Vec<RevisionRecord>,
    /// The latest screenplay revision still in play, and what it says: the
    /// one the director works on.
    pub screenplay: Option<(u32, Screenplay)>,
    /// The locked screenplay revision, which production follows.
    pub locked: Option<u32>,
    /// The analysis read a source revision that is no longer current.
    pub analysis_stale: bool,
    /// The latest screenplay was written from an analysis that is no longer
    /// current.
    pub screenplay_stale: bool,
    /// Estimated cost of the next analysis, in USD micros; `None` when not
    /// known, such as for an agent.
    pub analysis_cost: Option<u64>,
    /// Estimated cost of the next screenplay, likewise.
    pub screenplay_cost: Option<u64>,
    /// Every bible revision, oldest first.
    pub bible_revisions: Vec<RevisionRecord>,
    /// The latest bible revision still in play, and what it says.
    pub bible: Option<(u32, Bible)>,
    /// The locked bible revision, which casting and keyframes follow.
    pub bible_locked: Option<u32>,
    /// Why the bible in play may no longer describe the locked screenplay.
    pub bible_stale: Vec<StaleCause>,
    /// Estimated cost of the next bible, likewise.
    pub bible_cost: Option<u64>,
    /// Every shot plan revision, oldest first.
    pub shot_plan_revisions: Vec<RevisionRecord>,
    /// The latest shot plan revision still in play, and what it says.
    pub shot_plan: Option<(u32, ShotPlan)>,
    /// The locked shot plan revision, which shots follow.
    pub shot_plan_locked: Option<u32>,
    /// Why the plan in play may no longer follow the locked screenplay and
    /// bible.
    pub shot_plan_stale: Vec<shot_plan_document::StaleCause>,
    /// Estimated cost of the next shot plan, likewise.
    pub shot_plan_cost: Option<u64>,
    /// The documents could not be read; why.
    pub damaged: Option<String>,
}

/// One character worth casting, as last published.
#[derive(Debug, Clone)]
pub struct PublishedCharacter {
    pub id: String,
    pub name: String,
    /// Every casting image, oldest first, with where its image is.
    pub images: Vec<(CastImage, PathBuf)>,
    pub lock: Option<CharacterLock>,
    /// The parents of the locked cast that changed, went or came.
    pub stale: Vec<String>,
    /// An image of this character may have been charged without its result.
    pub unresolved: bool,
    pub voice: PublishedVoice,
}

/// Where casting a character's voice stands.
#[derive(Debug, Clone, Default)]
pub struct PublishedVoice {
    /// Every audition, oldest first, with where its audio is.
    pub auditions: Vec<(Audition, PathBuf)>,
    pub lock: Option<VoiceLock>,
    /// The parents of the locked voice that changed, went or came.
    pub stale: Vec<String>,
    /// An audition of this character may have been charged without its
    /// audio.
    pub unresolved: bool,
    /// What an audition says unless the director writes a line, in the
    /// project's language.
    pub default_line: String,
}

/// What the worker last published about the style and the cast. Reads
/// never do I/O.
#[derive(Debug, Clone, Default)]
pub struct PublishedProduction {
    /// Every style sample, oldest first, with where its image is.
    pub samples: Vec<(StyleSample, PathBuf)>,
    pub style: Option<StyleLock>,
    /// A style sample may have been charged without its result.
    pub style_unresolved: bool,
    /// The characters worth casting, in bible order.
    pub characters: Vec<PublishedCharacter>,
    /// The voices the speech provider offers, once asked for.
    pub voices: Vec<Voice>,
    /// Why the voices could not be listed, when they could not.
    pub voices_problem: Option<String>,
    /// The style or the cast could not be read; why.
    pub damaged: Option<String>,
}

/// What paid images cost with the session's image provider, as it declares.
/// Fixed at spawn.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImagePrices {
    pub sample: Option<Price>,
    pub portrait: Option<Price>,
    /// How many images a reference pack takes, and what they cost.
    pub pack: Option<(usize, Price)>,
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
    /// The last video submission may have been accepted but left no task.
    video_unresolved: bool,
    /// Who made each current candidate, in candidate order.
    origins: Vec<CandidateOrigin>,
    /// A paid keyframe call may have been accepted without leaving its result.
    keyframes_unresolved: bool,
    /// Why each current candidate is stale, in candidate order; see
    /// [`generation::keyframe_staleness`].
    staleness: Vec<Vec<String>>,
    /// Every candidate the shot has had, oldest first, with its media's
    /// absolute path.
    history: Vec<(CandidateRecord, Option<PathBuf>)>,
    /// Every video the shot has had, oldest generation first, with its
    /// media's absolute path.
    videos: Vec<(VideoRecord, Option<PathBuf>)>,
    /// What one take of the shot's next video costs with the session's
    /// video provider.
    video_estimate: Option<VideoEstimate>,
}

type ShotSlot = Option<Result<PublishedShot, Arc<ShotWorkflowError>>>;

#[derive(Default)]
struct View {
    metadata: OnceLock<ProjectMetadata>,
    /// The shots in play, in order, each with what was last published for
    /// it; `None` until the open finishes.
    shots: Mutex<Option<Vec<(ShotKey, ShotSlot)>>>,
    documents: Mutex<Option<Arc<PublishedDocuments>>>,
    production: Mutex<Option<Arc<PublishedProduction>>>,
    workflow: Mutex<Option<Arc<PublishedWorkflow>>>,
    media: Mutex<Option<Arc<PublishedMedia>>>,
}

/// The project's media as last checked. Reads never do I/O.
#[derive(Debug, Clone, Default)]
pub struct PublishedMedia {
    /// Records that say their bytes are kept, whose file is gone. Found at
    /// open and after the session made media; reported, never rewritten.
    pub missing: Vec<MediaRecord>,
    /// Records whose file's bytes no longer hash as recorded, as the last
    /// [`StorageCommand::Verify`] found them.
    pub mismatched: Vec<MediaRecord>,
    /// The disk the media takes, each file counted once.
    pub usage: media::Usage,
    /// How many of each owner's latest takes are kept.
    pub recent: usize,
    /// The last dry run, until the media changes or it is collected.
    pub plan: Option<media::Collection>,
    /// Why the media could not be listed, such as a damaged record.
    pub problem: Option<String>,
}

/// Where the project stands in its recipe, as last published. Reads never
/// do I/O.
#[derive(Debug, Clone, Default)]
pub struct PublishedWorkflow {
    /// Every step, in recipe order.
    pub steps: Vec<workflow::StepStatus>,
    /// Every scene with shots in play, and where each shot stands.
    pub scenes: Vec<workflow::SceneProgress>,
    /// Which shots can be exported, and why the others cannot.
    pub export: export::Readiness,
    /// The package the session last wrote.
    pub last_export: Option<export::Exported>,
    /// Why the steps could not be worked out, such as a recipe this build
    /// does not ship.
    pub problem: Option<String>,
}

struct Gate {
    /// Set when the session closes; agent calls in progress watch it too.
    closed: Arc<AtomicBool>,
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

enum Work {
    Shot {
        shot_index: u32,
        command: ShotCommand,
    },
    Document {
        command: DocumentCommand,
        /// The director chose to risk paying twice.
        resubmit: bool,
    },
    Production {
        command: ProductionCommand,
        submission: Submission,
    },
    Storage(StorageCommand),
    Export(export::ExportRequest),
}

/// Work on the project's stored media.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageCommand {
    /// Work out what collecting would let go, touching nothing: the dry
    /// run the director looks at before collecting.
    PlanCollection,
    /// Let go of what the last dry run listed, and nothing else.
    Collect,
    /// Read every kept file and check its bytes against its record.
    Verify,
}

struct Envelope {
    token: u64,
    work: Work,
}

/// Owner handle for one project session and its worker thread.
///
/// The handle is `Send + Sync`: any thread may submit commands or read
/// published state. Only the owner may drop it.
pub struct ProjectSession {
    commands: Option<mpsc::Sender<Envelope>>,
    image_provider: ImageProviderInfo,
    video_provider: VideoProviderInfo,
    speech_provider: SpeechProviderInfo,
    image_prices: ImagePrices,
    document_model: DocumentModelInfo,
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
        Self::spawn_with_model(request, provider, DocumentModel::None, sink, options)
    }

    /// [`Self::spawn_with`], writing documents with `model`.
    pub fn spawn_with_model(
        request: OpenRequest,
        provider: ImageProvider,
        model: DocumentModel,
        sink: Box<dyn SessionSink>,
        options: SessionOptions,
    ) -> std::io::Result<Self> {
        let image_provider = provider.info();
        let mut options = options;
        let videos = options
            .video_provider
            .get_or_insert_with(|| video_provider_for(&provider));
        let video_provider = videos.info();
        let speech_provider = options
            .speech_provider
            .get_or_insert_with(|| speech_provider_for(&provider))
            .info();
        let image_prices = ImagePrices {
            sample: style::sample_cost(&provider),
            portrait: cast::audition_cost(&provider, 1),
            pack: cast::pack_cost(&provider),
        };
        let document_model = model.describe();
        let (commands, inbox) = mpsc::channel();
        let view = Arc::new(View::default());
        let gate = Arc::new(Gate {
            closed: Arc::new(AtomicBool::new(false)),
            sink: Mutex::new(Some(sink)),
        });
        // Closing the session stops an agent that is still answering.
        let model = model.cancelled_by(Arc::clone(&gate.closed));
        let worker = thread::Builder::new()
            .name("kineto-project-session".to_owned())
            .spawn({
                let view = Arc::clone(&view);
                let gate = Arc::clone(&gate);
                move || run(request, provider, model, options, &inbox, &view, &gate)
            })?;
        Ok(Self {
            commands: Some(commands),
            image_provider,
            video_provider,
            speech_provider,
            image_prices,
            document_model,
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
        // Before the open finishes the shots are not known; the worker
        // refuses an index that turns out not to name one.
        if lock(&self.view.shots).as_ref().is_some_and(|shots| {
            usize::try_from(shot_index).map_or(true, |index| index >= shots.len())
        }) {
            return Err(SubmitError::InvalidShot);
        }
        self.send(Envelope {
            token,
            work: Work::Shot {
                shot_index,
                command,
            },
        })
    }

    /// Queue a document command. Returns without waiting for it to run.
    pub fn submit_document(&self, command: DocumentCommand, token: u64) -> Result<(), SubmitError> {
        self.send(Envelope {
            token,
            work: Work::Document {
                command,
                resubmit: false,
            },
        })
    }

    /// Queue a command that asks a model for a document although the last
    /// request for that document may have been answered, and charged for,
    /// without its answer being kept, so this one may pay twice. Only on the
    /// director's explicit decision; for any other command it is
    /// [`Self::submit_document`].
    pub fn resubmit_document(
        &self,
        command: DocumentCommand,
        token: u64,
    ) -> Result<(), SubmitError> {
        self.send(Envelope {
            token,
            work: Work::Document {
                command,
                resubmit: true,
            },
        })
    }

    /// Queue a style or casting command. Returns without waiting for it to
    /// run.
    pub fn submit_production(
        &self,
        command: ProductionCommand,
        token: u64,
    ) -> Result<(), SubmitError> {
        self.send(Envelope {
            token,
            work: Work::Production {
                command,
                submission: Submission::Once,
            },
        })
    }

    /// Queue writing a production package. Its result arrives through
    /// [`SessionSink::document_completed`]; what was written is published in
    /// [`PublishedWorkflow::last_export`].
    pub fn submit_export(
        &self,
        request: export::ExportRequest,
        token: u64,
    ) -> Result<(), SubmitError> {
        self.send(Envelope {
            token,
            work: Work::Export(request),
        })
    }

    /// Queue work on the project's stored media. Its result arrives through
    /// [`SessionSink::document_completed`].
    pub fn submit_storage(&self, command: StorageCommand, token: u64) -> Result<(), SubmitError> {
        self.send(Envelope {
            token,
            work: Work::Storage(command),
        })
    }

    /// Queue a paid style or casting command although an image of its own
    /// may have been charged without its result being kept, so this one may
    /// pay twice. Only on the director's explicit decision.
    pub fn resubmit_production(
        &self,
        command: ProductionCommand,
        token: u64,
    ) -> Result<(), SubmitError> {
        self.send(Envelope {
            token,
            work: Work::Production {
                command,
                submission: Submission::RiskPayingTwice,
            },
        })
    }

    fn send(&self, envelope: Envelope) -> Result<(), SubmitError> {
        self.commands
            .as_ref()
            .ok_or(SubmitError::Closed)?
            .send(envelope)
            .map_err(|_| SubmitError::Closed)
    }

    /// The project's documents as last published. Never does I/O.
    pub fn documents(&self) -> Result<Arc<PublishedDocuments>, SnapshotError> {
        lock(&self.view.documents)
            .clone()
            .ok_or(SnapshotError::NotOpen)
    }

    /// The project's media as last checked. Never does I/O.
    pub fn media(&self) -> Result<Arc<PublishedMedia>, SnapshotError> {
        lock(&self.view.media).clone().ok_or(SnapshotError::NotOpen)
    }

    /// Where the project stands in its recipe, as last published. Never does
    /// I/O.
    pub fn workflow(&self) -> Result<Arc<PublishedWorkflow>, SnapshotError> {
        lock(&self.view.workflow)
            .clone()
            .ok_or(SnapshotError::NotOpen)
    }

    /// The style and the cast as last published. Never does I/O.
    pub fn production(&self) -> Result<Arc<PublishedProduction>, SnapshotError> {
        lock(&self.view.production)
            .clone()
            .ok_or(SnapshotError::NotOpen)
    }

    /// What paid images cost with this session's image provider. Fixed at
    /// spawn.
    #[must_use]
    pub const fn image_prices(&self) -> ImagePrices {
        self.image_prices
    }

    /// The model this session writes documents with. Fixed at spawn.
    #[must_use]
    pub const fn document_model(&self) -> &DocumentModelInfo {
        &self.document_model
    }

    /// How many shots are in play: those of the locked shot plan, or of the
    /// bootstrap scene before one is locked. 0 until the open finishes.
    /// Never does I/O.
    #[must_use]
    pub fn shot_count(&self) -> usize {
        lock(&self.view.shots).as_ref().map_or(0, Vec::len)
    }

    /// Which shot index `shot_index` names. Never does I/O.
    #[must_use]
    pub fn shot_key(&self, shot_index: u32) -> Option<ShotKey> {
        let index = usize::try_from(shot_index).ok()?;
        lock(&self.view.shots)
            .as_ref()?
            .get(index)
            .map(|(key, _)| key.clone())
    }

    /// Why each current candidate of the shot is stale, in candidate order:
    /// `intent.<field>` for each intent field changed since, and each lock or
    /// bible entry followed that changed or went, by artifact ID. Never does
    /// I/O.
    pub fn candidate_staleness(&self, shot_index: u32) -> Result<Vec<Vec<String>>, SnapshotError> {
        self.published(shot_index, |published| published.staleness.clone())
    }

    /// Every keyframe candidate the shot has had, current or superseded,
    /// oldest first, with its media's absolute path. Never does I/O.
    pub fn candidate_history(
        &self,
        shot_index: u32,
    ) -> Result<Vec<(CandidateRecord, Option<PathBuf>)>, SnapshotError> {
        self.published(shot_index, |published| published.history.clone())
    }

    /// What one take of the shot's next video costs, at the clip length its
    /// plan asks for. Never does I/O.
    pub fn video_estimate(&self, shot_index: u32) -> Result<Option<VideoEstimate>, SnapshotError> {
        self.published(shot_index, |published| published.video_estimate)
    }

    /// Every video the shot has had, current or superseded, oldest
    /// generation first, with its media's absolute path. Never does I/O.
    pub fn video_history(
        &self,
        shot_index: u32,
    ) -> Result<Vec<(VideoRecord, Option<PathBuf>)>, SnapshotError> {
        self.published(shot_index, |published| published.videos.clone())
    }

    /// The shot's most recently published snapshot. Never does I/O.
    pub fn snapshot(&self, shot_index: u32) -> Result<ShotApprovalSnapshot, SnapshotError> {
        self.published(shot_index, |published| published.snapshot)
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

    /// Whether the shot's last video submission may have been accepted, and
    /// charged, without leaving its task. Generating is refused until the
    /// director resubmits. Never does I/O.
    pub fn video_unresolved(&self, shot_index: u32) -> Result<bool, SnapshotError> {
        self.published(shot_index, |published| published.video_unresolved)
    }

    /// Whether a paid keyframe call of the shot's last generation may have
    /// been accepted, and charged, without leaving its result. Generating is
    /// refused until the director resubmits. Never does I/O.
    pub fn keyframes_unresolved(&self, shot_index: u32) -> Result<bool, SnapshotError> {
        self.published(shot_index, |published| published.keyframes_unresolved)
    }

    fn published<R>(
        &self,
        shot_index: u32,
        read: impl FnOnce(&PublishedShot) -> R,
    ) -> Result<R, SnapshotError> {
        let shots = lock(&self.view.shots);
        let shots = shots.as_ref().ok_or(SnapshotError::NotOpen)?;
        let (_, slot) = usize::try_from(shot_index)
            .ok()
            .and_then(|index| shots.get(index))
            .ok_or(SnapshotError::InvalidShot)?;
        match slot {
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

    /// The provider this session makes videos with.
    #[must_use]
    pub const fn video_provider(&self) -> VideoProviderInfo {
        self.video_provider
    }

    /// The provider this session speaks lines with.
    #[must_use]
    pub const fn speech_provider(&self) -> SpeechProviderInfo {
        self.speech_provider
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

fn run(
    request: OpenRequest,
    provider: ImageProvider,
    model: DocumentModel,
    options: SessionOptions,
    inbox: &mpsc::Receiver<Envelope>,
    view: &View,
    gate: &Arc<Gate>,
) {
    let opened = panic::catch_unwind(AssertUnwindSafe(|| open(request)))
        .unwrap_or(Err(SessionError::Panicked));
    let mut worker = match opened {
        Ok(project) => {
            let mut worker = Worker::new(project, provider, model, Arc::clone(gate), options);
            // The locked plan's shots are on disk before anything reads them.
            let mut recoveries = Vec::new();
            if panic::catch_unwind(AssertUnwindSafe(|| worker.find_shots(true))).is_err() {
                recoveries.push(Recovery::NeedsAttention { count: 1 });
            }
            // Finish what an earlier session left before accepting commands.
            recoveries.extend(
                panic::catch_unwind(AssertUnwindSafe(|| worker.recover())).unwrap_or_else(|_| {
                    worker.shots.clear();
                    vec![Recovery::NeedsAttention { count: 1 }]
                }),
            );
            // Videos submitted earlier are watched again, never resubmitted.
            let unplaced =
                panic::catch_unwind(AssertUnwindSafe(|| worker.resume_videos())).unwrap_or(1);
            if unplaced > 0 {
                recoveries.push(Recovery::NeedsAttention { count: unplaced });
            }
            recoveries.extend(
                panic::catch_unwind(AssertUnwindSafe(|| worker.recover_documents()))
                    .unwrap_or_else(|_| vec![Recovery::NeedsAttention { count: 1 }]),
            );
            recoveries.extend(
                panic::catch_unwind(AssertUnwindSafe(|| worker.recover_images()))
                    .unwrap_or_else(|_| vec![Recovery::NeedsAttention { count: 1 }]),
            );
            let project = &worker.project;
            let _ = view.metadata.set(ProjectMetadata {
                project_id: project.manifest().project_id.clone(),
                title: project.manifest().title.clone(),
                format_version: project.manifest().format_version,
                read_only: project.is_read_only(),
            });
            worker.publish_shots(view);
            worker.publish_documents(view);
            worker.publish_production(view);
            worker.publish_workflow(view);
            worker.reconcile_media();
            worker.tend_media(view, false);
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
        match envelope {
            None => {}
            Some(Envelope {
                token,
                work:
                    Work::Shot {
                        shot_index,
                        command,
                    },
            }) => {
                let result = match worker.as_mut() {
                    None => Err(SessionError::NotOpen),
                    Some(worker) => {
                        let index = usize::try_from(shot_index).unwrap_or(usize::MAX);
                        let makes_media = matches!(
                            command,
                            ShotCommand::Generate
                                | ShotCommand::ResubmitKeyframes
                                | ShotCommand::GenerateConsenting(_)
                                | ShotCommand::Refine { .. }
                        );
                        let result = worker.apply(index, command);
                        worker.publish(view, index);
                        worker.publish_workflow(view);
                        if makes_media {
                            worker.tend_media(view, true);
                        }
                        result
                    }
                };
                gate.deliver(|sink| sink.completed(token, shot_index, &result));
            }
            Some(Envelope {
                token,
                work: Work::Document { command, resubmit },
            }) => {
                let result = match worker.as_mut() {
                    None => Err(SessionError::NotOpen),
                    Some(worker) => {
                        // Locking a plan rewrites its shots' intent and may
                        // change which shots are in play.
                        let rewrites = matches!(command, DocumentCommand::LockShotPlan { .. });
                        let result = worker.apply_document(command, resubmit);
                        if worker.find_shots(false) || (rewrites && result.is_ok()) {
                            worker.shots.clear();
                            worker.publish_shots(view);
                        }
                        worker.publish_documents(view);
                        // The bible says who is cast.
                        worker.publish_production(view);
                        worker.publish_workflow(view);
                        result
                    }
                };
                gate.deliver(|sink| sink.document_completed(token, &result));
            }
            Some(Envelope {
                token,
                work:
                    Work::Production {
                        command,
                        submission,
                    },
            }) => {
                let result = match worker.as_mut() {
                    None => Err(SessionError::NotOpen),
                    Some(worker) => {
                        let makes_media = matches!(
                            command,
                            ProductionCommand::SampleStyle { .. }
                                | ProductionCommand::RefineStyle { .. }
                                | ProductionCommand::Audition { .. }
                                | ProductionCommand::MakePack { .. }
                                | ProductionCommand::AuditionVoices { .. }
                        );
                        let result = worker.apply_production(command, submission);
                        worker.publish_production(view);
                        worker.publish_workflow(view);
                        if makes_media {
                            worker.tend_media(view, true);
                        }
                        result
                    }
                };
                gate.deliver(|sink| sink.document_completed(token, &result));
            }
            Some(Envelope {
                token,
                work: Work::Export(request),
            }) => {
                let result = match worker.as_mut() {
                    None => Err(SessionError::NotOpen),
                    Some(worker) => {
                        let result = worker.export(&request);
                        worker.publish_workflow(view);
                        result
                    }
                };
                gate.deliver(|sink| sink.document_completed(token, &result));
            }
            Some(Envelope {
                token,
                work: Work::Storage(command),
            }) => {
                let result = match worker.as_mut() {
                    None => Err(SessionError::NotOpen),
                    Some(worker) => {
                        let result = worker.apply_storage(command);
                        if command == StorageCommand::Collect {
                            worker.shots.clear();
                            worker.publish_shots(view);
                            worker.publish_production(view);
                        }
                        worker.tend_media(view, false);
                        result
                    }
                };
                gate.deliver(|sink| sink.document_completed(token, &result));
            }
        }
        if let Some(worker) = worker.as_mut() {
            for (key, revision, result) in worker.finish_videos() {
                // A shot no longer in play keeps its video; no one is told.
                let Some(index) = worker.index_of(&key) else {
                    continue;
                };
                worker.publish(view, index);
                worker.publish_workflow(view);
                if result.is_ok() {
                    worker.tend_media(view, true);
                }
                let shot_index = u32::try_from(index).expect("shot index fits in u32");
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

/// Timing and video provider a session runs with.
#[derive(Debug, Clone)]
pub struct SessionOptions {
    /// How often a video's remote task is queried.
    pub video_poll_interval: std::time::Duration,
    /// The provider videos are made with. `None` follows the image
    /// provider: MiniMax with its key, otherwise the local demo.
    pub video_provider: Option<VideoProvider>,
    /// The provider lines are spoken with. `None` follows the image
    /// provider: MiniMax with its key, otherwise the local demo.
    pub speech_provider: Option<SpeechProvider>,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            video_poll_interval: video::POLL_INTERVAL,
            video_provider: None,
            speech_provider: None,
        }
    }
}

/// How often the worker collects videos finished in the background.
const VIDEO_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Videos being generated for this session's shots.
struct Videos {
    provider: VideoProvider,
    gate: Arc<Gate>,
    /// Submitted videos being followed, by artifact ID: a shot may have
    /// several candidates in flight.
    in_flight: BTreeMap<String, VideoJob>,
    /// Shots whose last submission is unresolved; see [`ShotCommand::ResubmitVideo`].
    unresolved: BTreeSet<ShotKey>,
    finished: Arc<Mutex<Vec<VideoDone>>>,
    watchers: Vec<thread::JoinHandle<()>>,
    poll_interval: std::time::Duration,
}

impl Videos {
    /// Follow `job` on its own thread, with the provider it was submitted
    /// to, until its video is on disk or failed. A MiniMax job waits for a
    /// session with the key.
    fn watch(&mut self, job: VideoJob, root: ProjectRoot) {
        if job.provider == video::DEMO_VIDEO_PROVIDER {
            self.spawn_watcher(LocalDemoVideos, job, root);
        } else if let VideoProvider::Minimax(Some(client)) = &self.provider {
            let client = client.clone();
            self.spawn_watcher(client, job, root);
        }
    }

    /// Whether a video of `key` is being followed.
    fn follows(&self, key: &ShotKey) -> bool {
        self.in_flight.values().any(|job| &job.shot == key)
    }

    fn spawn_watcher<G: VideoGenerator + Send + 'static>(
        &mut self,
        generator: G,
        job: VideoJob,
        root: ProjectRoot,
    ) {
        let artifact = job.artifact_id.as_str().to_owned();
        self.in_flight.insert(artifact.clone(), job.clone());
        let gate = Arc::clone(&self.gate);
        let finished = Arc::clone(&self.finished);
        let interval = self.poll_interval;
        let spawned = thread::Builder::new()
            .name("kineto-video-watch".to_owned())
            .spawn(move || {
                if let Some(done) = video::watch(&generator, &root, job, &gate.closed, interval) {
                    lock(&finished).push(done);
                }
            });
        match spawned {
            Ok(handle) => self.watchers.push(handle),
            // Without a watcher the task is followed again on the next open.
            Err(_) => {
                self.in_flight.remove(&artifact);
            }
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
    model: DocumentModel,
    videos: Videos,
    /// The last dry run of a collection, which [`StorageCommand::Collect`]
    /// carries out.
    collection: Option<media::Collection>,
    /// What the last verification found.
    mismatched: Vec<MediaRecord>,
    /// The package this session last wrote.
    last_export: Option<export::Exported>,
    /// The shots in play, in order; a shot index names one of them.
    keys: Vec<ShotKey>,
    // Canonical JSON is loaded once per shot per session; reads then stay
    // typed and in memory rather than re-parsing project files.
    shots: BTreeMap<ShotKey, ShotWorkflow>,
    /// Shots whose last generation is unresolved; see
    /// [`ShotCommand::ResubmitKeyframes`].
    keyframes_unresolved: BTreeSet<ShotKey>,
    /// A style sample may have been charged without its result.
    style_unresolved: bool,
    /// Characters an image of which may have been charged without its
    /// result.
    cast_unresolved: BTreeSet<String>,
    speech: SpeechProvider,
    /// Characters an audition of which may have been charged without its
    /// audio.
    voice_unresolved: BTreeSet<String>,
    /// The voices the speech provider offers, or why they could not be
    /// listed; `None` until asked for.
    voices: Option<Result<Vec<Voice>, String>>,
}

impl Worker {
    fn new(
        project: CanonicalProject,
        provider: ImageProvider,
        model: DocumentModel,
        gate: Arc<Gate>,
        options: SessionOptions,
    ) -> Self {
        let video_provider = options
            .video_provider
            .clone()
            .unwrap_or_else(|| video_provider_for(&provider));
        let speech = options
            .speech_provider
            .clone()
            .unwrap_or_else(|| speech_provider_for(&provider));
        Self {
            project,
            provider,
            model,
            videos: Videos {
                provider: video_provider,
                gate,
                in_flight: BTreeMap::new(),
                unresolved: BTreeSet::new(),
                finished: Arc::new(Mutex::new(Vec::new())),
                watchers: Vec::new(),
                poll_interval: options.video_poll_interval,
            },
            keys: Vec::new(),
            shots: BTreeMap::new(),
            keyframes_unresolved: BTreeSet::new(),
            style_unresolved: false,
            cast_unresolved: BTreeSet::new(),
            speech,
            voice_unresolved: BTreeSet::new(),
            voices: None,
            collection: None,
            mismatched: Vec::new(),
            last_export: None,
        }
    }

    /// Attach (or retire) every video whose watcher has finished.
    fn finish_videos(&mut self) -> Vec<(ShotKey, u16, Result<(), SessionError>)> {
        let finished = std::mem::take(&mut *lock(&self.videos.finished));
        let mut outcomes = Vec::with_capacity(finished.len());
        for done in finished {
            let key = done.job.shot.clone();
            self.videos.in_flight.remove(done.job.artifact_id.as_str());
            let revision = done.job.revision;
            let result = panic::catch_unwind(AssertUnwindSafe(|| {
                self.with_shot(&key, |project, _, shot| video::finish(project, shot, done))
            }))
            .unwrap_or(Err(GenerationError::Unexpected(
                "attaching a video panicked",
            )));
            match &result {
                // A video of the shot attached: a candidate of its generation
                // left unresolved can no longer become the shot's.
                Ok(_) => {
                    self.videos.unresolved.remove(&key);
                }
                Err(_) => {
                    self.shots.remove(&key);
                }
            }
            outcomes.push((key, revision, result.map(|_| ()).map_err(generation_error)));
        }
        outcomes
    }

    fn apply(&mut self, shot_index: usize, command: ShotCommand) -> Result<(), SessionError> {
        let Some(key) = self.keys.get(shot_index).cloned() else {
            return Err(SessionError::Shot(ShotWorkflowError::InvalidShotNumber));
        };
        let key = &key;
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| match command {
            // Generation runs as write-ahead jobs so an interruption is
            // recoverable; every other command is a single canonical update.
            ShotCommand::Generate => self.generate(key, Consent::default()),
            ShotCommand::ResubmitKeyframes => self.generate(key, Consent::RISK_PAYING_TWICE),
            ShotCommand::GenerateConsenting(consent) => self.generate(key, consent),
            ShotCommand::Refine { note, consent } => self.refine(key, consent, &note),
            ShotCommand::GenerateVideo => self.start_video(key, &VideoAsk::default(), false),
            ShotCommand::ResubmitVideo => self.start_video(key, &VideoAsk::default(), true),
            ShotCommand::GenerateVideos(ask) => self.start_video(key, &ask, false),
            ShotCommand::ResubmitVideos(ask) => self.start_video(key, &ask, true),
            // A keyframe of a style or cast since moved on from is not locked.
            ShotCommand::Lock => self
                .with_shot(key, |project, _, shot| generation::lock(project, shot))
                .map_err(generation_error),
            command => self
                .with_shot(key, |project, _, shot| match command {
                    ShotCommand::SetDirection(direction) => shot.set_direction(project, direction),
                    ShotCommand::Select(candidate) => shot.select(project, candidate),
                    ShotCommand::Reset => shot.reset(project),
                    ShotCommand::MigrateLegacyHashes => {
                        shot.migrate_legacy_hashes(project).map(|_report| ())
                    }
                    ShotCommand::SelectVideo(video) => shot.select_video(project, video),
                    ShotCommand::LockVideo => shot.lock_video(project),
                    ShotCommand::Generate
                    | ShotCommand::Lock
                    | ShotCommand::ResubmitKeyframes
                    | ShotCommand::GenerateConsenting(_)
                    | ShotCommand::Refine { .. }
                    | ShotCommand::GenerateVideo
                    | ShotCommand::ResubmitVideo
                    | ShotCommand::GenerateVideos(_)
                    | ShotCommand::ResubmitVideos(_) => {
                        unreachable!("handled above")
                    }
                })
                .map_err(SessionError::Shot),
        }));
        match outcome {
            Ok(result) => result,
            Err(_) => {
                // Do not trust a half-applied in-memory shot; reload it from
                // its canonical files on next use.
                self.shots.remove(key);
                Err(SessionError::Panicked)
            }
        }
    }

    fn start_video(
        &mut self,
        key: &ShotKey,
        ask: &VideoAsk,
        risk_paying_twice: bool,
    ) -> Result<(), SessionError> {
        if self.videos.follows(key) {
            return Err(SessionError::VideoInFlight);
        }
        let started = match self.videos.provider.clone() {
            VideoProvider::LocalDemo => self.with_shot(key, |project, _, shot| {
                video::start_videos(
                    project,
                    shot,
                    &LocalDemoVideos,
                    ask,
                    &mut NoHooks,
                    risk_paying_twice,
                )
            }),
            VideoProvider::Minimax(Some(client)) => self.with_shot(key, |project, _, shot| {
                video::start_videos(project, shot, &client, ask, &mut NoHooks, risk_paying_twice)
            }),
            VideoProvider::Minimax(None) => {
                return Err(SessionError::Generation(GenerationError::NotConfigured));
            }
        }
        .map_err(generation_error)?;
        // Candidates submitted before one could not be are followed whatever
        // happened to it.
        let root = self.project.root().clone();
        for job in started.jobs {
            self.videos.watch(job, root.clone());
        }
        match started.error {
            None => {
                self.videos.unresolved.remove(key);
                Ok(())
            }
            Some(error) => {
                if matches!(error, GenerationError::UnresolvedSubmission) {
                    self.videos.unresolved.insert(key.clone());
                }
                Err(generation_error(error))
            }
        }
    }

    /// Watch again the videos an earlier session submitted. Nothing is
    /// submitted. The demo's are always followed; MiniMax's wait for a
    /// session with the key.
    fn resume_videos(&mut self) -> usize {
        if self.project.is_read_only() {
            return 0;
        }
        let keyed = matches!(self.videos.provider, VideoProvider::Minimax(Some(_)));
        let mut needs_attention = 0;
        for provider in [video::DEMO_VIDEO_PROVIDER, video::VIDEO_PROVIDER] {
            if provider == video::VIDEO_PROVIDER && !keyed {
                continue;
            }
            match video::in_flight(&self.project, provider) {
                Ok(found) => {
                    for job in found.jobs {
                        let root = self.project.root().clone();
                        self.videos.watch(job, root);
                    }
                    self.videos.unresolved.extend(found.unresolved);
                    needs_attention += found.needs_attention;
                }
                Err(_) => needs_attention += 1,
            }
        }
        needs_attention
    }

    fn generate(&mut self, key: &ShotKey, consent: Consent) -> Result<(), SessionError> {
        let generated = self.with_shot(key, |project, provider, shot| {
            generation::generate_with_consent(project, shot, provider, &mut NoHooks, consent)
        });
        self.note_keyframes(key, generated)
    }

    fn refine(&mut self, key: &ShotKey, consent: Consent, note: &str) -> Result<(), SessionError> {
        let refined = self.with_shot(key, |project, provider, shot| {
            generation::refine(project, shot, provider, &mut NoHooks, consent, note)
        });
        self.note_keyframes(key, refined)
    }

    fn note_keyframes(
        &mut self,
        key: &ShotKey,
        generated: Result<AttachOutcome, GenerationError>,
    ) -> Result<(), SessionError> {
        match &generated {
            Ok(_) => {
                self.keyframes_unresolved.remove(key);
            }
            Err(GenerationError::UnresolvedSubmission) => {
                self.keyframes_unresolved.insert(key.clone());
            }
            Err(_) => {}
        }
        generated.map(|_outcome| ()).map_err(generation_error)
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
            keys,
            shots,
            keyframes_unresolved,
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
                unresolved,
            } = match generation::unfinished(project, provider) {
                Ok(unfinished) => unfinished,
                Err(_) => {
                    unplaced += 1;
                    continue;
                }
            };
            unplaced += needs_attention;
            keyframes_unresolved.extend(unresolved);
            for plan in plans {
                let Ok(Some(key)) = shot::locate(project, plan.shot_id()) else {
                    unplaced += 1;
                    continue;
                };
                let result = shot_mut(project, shots, &key)
                    .map_err(GenerationError::Shot)
                    .and_then(|shot| {
                        generation::complete(project, shot, &plan, provider, &mut NoHooks)
                    });
                if result.is_err() {
                    shots.remove(&key);
                }
                // A shot no longer in play is completed all the same; no
                // one is told.
                let Some(shot_index) = keys
                    .iter()
                    .position(|in_play| *in_play == key)
                    .and_then(|index| u32::try_from(index).ok())
                else {
                    continue;
                };
                recoveries.push(match result {
                    Ok(_outcome) => Recovery::Completed {
                        shot_index,
                        revision: plan.revision(),
                    },
                    Err(error) => Recovery::Failed {
                        shot_index,
                        revision: plan.revision(),
                        error: generation_error(error),
                    },
                });
            }
        }
        if unplaced > 0 {
            recoveries.push(Recovery::NeedsAttention { count: unplaced });
        }
        recoveries
    }

    fn apply_document(
        &mut self,
        command: DocumentCommand,
        resubmit: bool,
    ) -> Result<(), SessionError> {
        let ready = self.model.describe().ready;
        let project = &self.project;
        let model = &self.model;
        let ask =
            |kind: DocumentKind, generate: &dyn Fn() -> Result<RevisionRecord, GenerationError>| {
                if !ready {
                    return Err(SessionError::Generation(
                        GenerationError::ModelNotConfigured,
                    ));
                }
                if resubmit {
                    documents::risk_paying_twice(project, &model.info().provider, kind)
                        .map_err(generation_error)?;
                }
                generate().map(|_| ()).map_err(generation_error)
            };
        let revision_error = |error| SessionError::Document(DocumentError::Revision(error));
        panic::catch_unwind(AssertUnwindSafe(|| match command {
            DocumentCommand::ImportSource {
                text,
                format,
                original_name,
            } => source::import(project, text.as_bytes(), format, original_name.as_deref())
                .map(|_| ())
                .map_err(|error| SessionError::Document(DocumentError::Source(error))),
            DocumentCommand::AnalyzeStory => ask(DocumentKind::StoryAnalysis, &|| {
                story::analyze(project, model)
            }),
            DocumentCommand::WriteScreenplay => ask(DocumentKind::Screenplay, &|| {
                screenplay::write(project, model)
            }),
            DocumentCommand::RewriteScene {
                scene_id,
                direction,
            } => ask(DocumentKind::Screenplay, &|| {
                screenplay::rewrite_scene(project, model, &scene_id, direction.as_deref())
            }),
            DocumentCommand::SaveScene { based_on, edit } => {
                let latest = revisions::latest(project, DocumentKind::Screenplay)
                    .map_err(revision_error)?
                    .ok_or(revision_error(RevisionError::NoSuchRevision(based_on)))?;
                if latest.revision != based_on {
                    return Err(revision_error(RevisionError::NotLatest {
                        based_on,
                        latest: latest.revision,
                    }));
                }
                let base: Screenplay = revisions::read(project, &latest).map_err(revision_error)?;
                let edited = base
                    .with_scene_edit(edit)
                    .map_err(|reason| revision_error(RevisionError::InvalidDocument(reason)))?;
                screenplay_document::edit(project, based_on, &edited)
                    .map(|_| ())
                    .map_err(revision_error)
            }
            DocumentCommand::LockScreenplay { revision } => {
                screenplay_document::lock(project, revision).map_err(revision_error)
            }
            DocumentCommand::WriteBible => {
                ask(DocumentKind::Bible, &|| bible::write(project, model))
            }
            DocumentCommand::SaveBibleEntry { based_on, edit } => {
                let latest = revisions::latest(project, DocumentKind::Bible)
                    .map_err(revision_error)?
                    .ok_or(revision_error(RevisionError::NoSuchRevision(based_on)))?;
                if latest.revision != based_on {
                    return Err(revision_error(RevisionError::NotLatest {
                        based_on,
                        latest: latest.revision,
                    }));
                }
                let base: Bible = revisions::read(project, &latest).map_err(revision_error)?;
                let edited = base
                    .with_entry_edit(edit)
                    .map_err(|reason| revision_error(RevisionError::InvalidDocument(reason)))?;
                bible_document::edit(project, based_on, &edited)
                    .map(|_| ())
                    .map_err(revision_error)
            }
            DocumentCommand::LockBible { revision } => {
                bible_document::lock(project, revision).map_err(revision_error)
            }
            DocumentCommand::PlanShots => {
                ask(DocumentKind::ShotPlan, &|| shot_plan::write(project, model))
            }
            DocumentCommand::ReplanScene {
                scene_id,
                direction,
            } => ask(DocumentKind::ShotPlan, &|| {
                shot_plan::replan_scene(project, model, &scene_id, direction.as_deref())
            }),
            DocumentCommand::SaveScenePlan { based_on, scene } => {
                shot_plan_document::edit_scene(project, based_on, scene)
                    .map(|_| ())
                    .map_err(revision_error)
            }
            DocumentCommand::LockShotPlan { revision } => {
                shot_plan_document::lock(project, revision).map_err(revision_error)
            }
        }))
        .unwrap_or(Err(SessionError::Panicked))
    }

    fn apply_production(
        &mut self,
        command: ProductionCommand,
        submission: Submission,
    ) -> Result<(), SessionError> {
        let project = &self.project;
        let provider = &self.provider;
        let style_error = |error| SessionError::Production(ProductionError::Style(error));
        let cast_error = |error| SessionError::Production(ProductionError::Cast(error));
        let voice_error = |error| SessionError::Production(ProductionError::Voice(error));
        let paid = |result: Result<(), GenerationError>| result.map_err(generation_error);
        let mut hooks = NoHooks;
        let outcome = panic::catch_unwind(AssertUnwindSafe(|| match &command {
            ProductionCommand::SampleStyle { preset, note } => paid(
                style::sample(
                    project,
                    provider,
                    SampleRequest {
                        preset,
                        note: note.as_deref(),
                        scene_id: None,
                    },
                    submission,
                    &mut hooks,
                )
                .map(|_| ()),
            ),
            ProductionCommand::RefineStyle { sample, note } => paid(
                style::refine(project, provider, sample, note, submission, &mut hooks).map(|_| ()),
            ),
            ProductionCommand::SelectStyle(sample) => {
                style_document::select(project, sample).map_err(style_error)
            }
            ProductionCommand::RejectStyle(sample) => {
                style_document::reject(project, sample).map_err(style_error)
            }
            ProductionCommand::LockStyle(sample) => style_document::lock(project, sample)
                .map(|_| ())
                .map_err(style_error),
            ProductionCommand::Audition { character, count } => paid(
                cast::audition(
                    project,
                    provider,
                    character,
                    usize::try_from(*count).unwrap_or(usize::MAX),
                    submission,
                    &mut hooks,
                )
                .map(|_| ()),
            ),
            ProductionCommand::MakePack { character, remake } => paid(
                cast::remake_pack(project, provider, character, remake, submission, &mut hooks)
                    .map(|_| ()),
            ),
            ProductionCommand::SelectPortrait {
                character,
                portrait,
            } => cast_document::select(project, character, portrait).map_err(cast_error),
            ProductionCommand::RejectPortrait {
                character,
                portrait,
            } => cast_document::reject(project, character, portrait).map_err(cast_error),
            ProductionCommand::LockCharacter { character } => {
                cast::lock(project, provider, character)
                    .map(|_| ())
                    .map_err(generation_error)
            }
            ProductionCommand::LoadVoices => {
                let listed = match &self.speech {
                    SpeechProvider::LocalDemo => {
                        LocalDemoVoices.voices().map_err(|error| match error {})
                    }
                    SpeechProvider::Minimax(Some(client)) => {
                        client.voices().map_err(|error| GenerationError::Provider {
                            class: error.class(),
                            source: Box::new(error),
                        })
                    }
                    SpeechProvider::Minimax(None) => Err(GenerationError::NotConfigured),
                };
                self.voices = Some(
                    listed
                        .as_ref()
                        .map(Clone::clone)
                        .map_err(ToString::to_string),
                );
                listed.map(|_| ()).map_err(generation_error)
            }
            ProductionCommand::AuditionVoices {
                character,
                choices,
                line,
            } => {
                let line = line.as_deref();
                paid(
                    match &self.speech {
                        SpeechProvider::LocalDemo => voice::audition(
                            project,
                            &LocalDemoVoices,
                            character,
                            choices,
                            line,
                            submission,
                            &mut hooks,
                        ),
                        SpeechProvider::Minimax(Some(client)) => voice::audition(
                            project, client, character, choices, line, submission, &mut hooks,
                        ),
                        SpeechProvider::Minimax(None) => Err(GenerationError::NotConfigured),
                    }
                    .map(|_| ()),
                )
            }
            ProductionCommand::SelectVoice {
                character,
                audition,
            } => voice_document::select(project, character, audition).map_err(voice_error),
            ProductionCommand::RejectVoice {
                character,
                audition,
            } => voice_document::reject(project, character, audition).map_err(voice_error),
            ProductionCommand::LockVoice { character } => voice::lock(project, character)
                .map(|_| ())
                .map_err(generation_error),
        }))
        .unwrap_or(Err(SessionError::Panicked));
        // A paid call cut short may have been charged without its result;
        // what the partitions now hold says so, whichever way it ended.
        match command {
            ProductionCommand::SampleStyle { .. } | ProductionCommand::RefineStyle { .. } => {
                self.note_unresolved_samples();
            }
            ProductionCommand::Audition { .. } | ProductionCommand::MakePack { .. } => {
                self.note_unresolved_casting();
            }
            ProductionCommand::AuditionVoices { .. } => self.note_unresolved_voices(),
            _ => {}
        }
        outcome
    }

    /// Record style samples and casting images MiniMax answered before an
    /// interruption, and note those that may have been charged without
    /// their result. Never calls MiniMax.
    fn recover_images(&mut self) -> Vec<Recovery> {
        if self.project.is_read_only() {
            return Vec::new();
        }
        let mut recoveries = Vec::new();
        let mut attention = 0;
        for (kind, recovered) in [
            (ImageWork::StyleSample, style::recover(&self.project)),
            (ImageWork::Casting, cast::recover(&self.project)),
        ] {
            match recovered {
                Ok(recovery) => {
                    attention += recovery.needs_attention;
                    if !recovery.attached.is_empty() {
                        recoveries.push(Recovery::Images {
                            kind,
                            count: recovery.attached.len(),
                        });
                    }
                    self.note_unresolved(kind, &recovery);
                }
                Err(_) => attention += 1,
            }
        }
        match voice::recover(&self.project) {
            Ok(recovery) => {
                attention += recovery.needs_attention;
                if !recovery.attached.is_empty() {
                    recoveries.push(Recovery::Images {
                        kind: ImageWork::VoiceAudition,
                        count: recovery.attached.len(),
                    });
                }
                self.voice_unresolved = unresolved_characters(&recovery.unresolved);
            }
            Err(_) => attention += 1,
        }
        if attention > 0 {
            recoveries.push(Recovery::NeedsAttention { count: attention });
        }
        recoveries
    }

    fn note_unresolved_voices(&mut self) {
        if let Ok(recovery) = voice::recover(&self.project) {
            self.voice_unresolved = unresolved_characters(&recovery.unresolved);
        }
    }

    fn note_unresolved_samples(&mut self) {
        if let Ok(recovery) = style::recover(&self.project) {
            self.note_unresolved(ImageWork::StyleSample, &recovery);
        }
    }

    fn note_unresolved_casting(&mut self) {
        if let Ok(recovery) = cast::recover(&self.project) {
            self.note_unresolved(ImageWork::Casting, &recovery);
        }
    }

    fn note_unresolved(&mut self, kind: ImageWork, recovery: &ImageRecovery) {
        match kind {
            ImageWork::StyleSample => self.style_unresolved = !recovery.unresolved.is_empty(),
            ImageWork::VoiceAudition => {
                self.voice_unresolved = unresolved_characters(&recovery.unresolved);
            }
            ImageWork::Casting => {
                self.cast_unresolved = recovery
                    .unresolved
                    .iter()
                    .filter_map(|id| cast_document::character_of(id))
                    .map(str::to_owned)
                    .collect();
            }
        }
    }

    /// Keep the documents models answered before an interruption, whichever
    /// model this session uses, and render the screenplay again. Never asks
    /// a model.
    fn recover_documents(&mut self) -> Vec<Recovery> {
        if self.project.is_read_only() {
            return Vec::new();
        }
        let mut recoveries = Vec::new();
        let mut attention = 0;
        let own = self.model.info().provider;
        let mut providers: Vec<&str> = DOCUMENT_PROVIDERS.to_vec();
        if !providers.contains(&own.as_str()) {
            providers.push(&own);
        }
        for provider in providers {
            match documents::recover(&self.project, provider) {
                Ok((completed, needs_attention)) => {
                    attention += needs_attention;
                    for record in completed {
                        let kind = [
                            DocumentKind::Screenplay,
                            DocumentKind::Bible,
                            DocumentKind::ShotPlan,
                        ]
                        .into_iter()
                        .find(|kind| record.artifact_id.starts_with(kind.id_prefix()))
                        .unwrap_or(DocumentKind::StoryAnalysis);
                        recoveries.push(Recovery::Document {
                            kind,
                            revision: record.revision,
                        });
                    }
                }
                Err(_) => attention += 1,
            }
        }
        if attention > 0 {
            recoveries.push(Recovery::NeedsAttention { count: attention });
        }
        screenplay_document::rerender(&self.project);
        recoveries
    }

    fn publish_documents(&self, view: &View) {
        let published = panic::catch_unwind(AssertUnwindSafe(|| self.read_documents()))
            .unwrap_or_else(|_| Err(SessionError::Panicked))
            .unwrap_or_else(|error| PublishedDocuments {
                damaged: Some(error.to_string()),
                ..PublishedDocuments::default()
            });
        *lock(&view.documents) = Some(Arc::new(published));
    }

    fn read_documents(&self) -> Result<PublishedDocuments, SessionError> {
        let project = &self.project;
        let revision_error = |error| SessionError::Document(DocumentError::Revision(error));
        let source = source::current(project)
            .map_err(|error| SessionError::Document(DocumentError::Source(error)))?;
        let analysis_record =
            revisions::current(project, DocumentKind::StoryAnalysis).map_err(revision_error)?;
        let analysis = match &analysis_record {
            None => None,
            Some(record) => Some(PublishedAnalysis {
                revision: record.revision,
                analysis: revisions::read(project, record).map_err(revision_error)?,
            }),
        };
        let all =
            revisions::revisions(project, DocumentKind::Screenplay).map_err(revision_error)?;
        let latest =
            revisions::latest(project, DocumentKind::Screenplay).map_err(revision_error)?;
        let screenplay = match &latest {
            None => None,
            Some(record) => Some((
                record.revision,
                revisions::read::<Screenplay>(project, record).map_err(revision_error)?,
            )),
        };
        let locked = all
            .iter()
            .find(|record| record.status == revisions::RevisionStatus::Locked)
            .map(|record| record.revision);
        let source_id = format!("source_r{:04}", source.revision);
        let analysis_stale = analysis_record.as_ref().is_some_and(|record| {
            !record.dependencies.iter().any(|dependency| {
                dependency.artifact_id == source_id
                    && dependency.content_hash == source.content_hash
            })
        });
        let screenplay_stale = latest
            .as_ref()
            .is_some_and(|record| match &analysis_record {
                None => true,
                Some(current) => !record.dependencies.iter().any(|dependency| {
                    dependency.artifact_id == current.artifact_id
                        && dependency.content_hash == current.content_hash
                }),
            });
        let bible_revisions =
            revisions::revisions(project, DocumentKind::Bible).map_err(revision_error)?;
        let bible_latest =
            revisions::latest(project, DocumentKind::Bible).map_err(revision_error)?;
        let bible_stale = match &bible_latest {
            None => Vec::new(),
            Some(record) => bible_document::staleness(project, record).map_err(revision_error)?,
        };
        let bible = match &bible_latest {
            None => None,
            Some(record) => Some((
                record.revision,
                revisions::read::<Bible>(project, record).map_err(revision_error)?,
            )),
        };
        let bible_locked = bible_revisions
            .iter()
            .find(|record| record.status == revisions::RevisionStatus::Locked)
            .map(|record| record.revision);
        let shot_plan_revisions =
            revisions::revisions(project, DocumentKind::ShotPlan).map_err(revision_error)?;
        let shot_plan_latest =
            revisions::latest(project, DocumentKind::ShotPlan).map_err(revision_error)?;
        let shot_plan_stale = match &shot_plan_latest {
            None => Vec::new(),
            Some(record) => {
                shot_plan_document::staleness(project, record).map_err(revision_error)?
            }
        };
        let shot_plan = match &shot_plan_latest {
            None => None,
            Some(record) => Some((
                record.revision,
                revisions::read::<ShotPlan>(project, record).map_err(revision_error)?,
            )),
        };
        let shot_plan_locked = shot_plan_revisions
            .iter()
            .find(|record| record.status == revisions::RevisionStatus::Locked)
            .map(|record| record.revision);
        let info = self.model.info();
        let ready = self.model.describe().ready;
        Ok(PublishedDocuments {
            source_revision: Some(source.revision),
            shot_plan_cost: if ready && bible_locked.is_some() && locked.is_some() {
                shot_plan::plan_cost(project, &info).ok().flatten()
            } else {
                None
            },
            shot_plan_revisions,
            shot_plan,
            shot_plan_locked,
            shot_plan_stale,
            bible_cost: if ready && locked.is_some() {
                bible::bible_cost(project, &info).ok().flatten()
            } else {
                None
            },
            bible_revisions,
            bible,
            bible_locked,
            bible_stale,
            analysis_cost: if ready {
                story::analysis_cost(project, &info).ok().flatten()
            } else {
                None
            },
            screenplay_cost: if ready && analysis.is_some() {
                screenplay::screenplay_cost(project, &info).ok().flatten()
            } else {
                None
            },
            analysis,
            revisions: all,
            screenplay,
            locked,
            analysis_stale,
            screenplay_stale,
            damaged: None,
        })
    }

    /// List the project's media, link files whose bytes are the same when
    /// `dedupe`, and publish which present files are gone. Storage upkeep:
    /// it never fails a command.
    fn tend_media(&mut self, view: &View, dedupe: bool) {
        // New media may change what a collection would let go; the dry run
        // is asked for again.
        if dedupe {
            self.collection = None;
        }
        let recent = media::recent_window(&self.project);
        let tended = panic::catch_unwind(AssertUnwindSafe(|| {
            let records = media::catalog(&self.project)?;
            if dedupe && !self.project.is_read_only() {
                media::dedupe(&self.project, &records)?;
            }
            Ok::<_, media::MediaError>((
                media::check(&self.project, &records, false)?,
                media::usage(&self.project, &records),
            ))
        }));
        let (missing, usage, problem) = match tended {
            Ok(Ok((report, usage))) => (report.missing, usage, None),
            Ok(Err(error)) => (Vec::new(), media::Usage::default(), Some(error.to_string())),
            Err(_) => (
                Vec::new(),
                media::Usage::default(),
                Some("checking the media panicked".to_owned()),
            ),
        };
        let published = PublishedMedia {
            missing,
            mismatched: self.mismatched.clone(),
            usage,
            recent,
            plan: self.collection.clone(),
            problem,
        };
        *lock(&view.media) = Some(Arc::new(published));
    }

    /// Finish a collection an earlier session left interrupted.
    fn reconcile_media(&self) {
        if self.project.is_read_only() {
            return;
        }
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            media::catalog(&self.project)
                .and_then(|records| media::reconcile(&self.project, &records))
        }));
    }

    fn export(&mut self, request: &export::ExportRequest) -> Result<(), SessionError> {
        let written = panic::catch_unwind(AssertUnwindSafe(|| {
            export::export(&self.project, request, std::time::SystemTime::now())
        }))
        .map_err(|_| SessionError::Panicked)?
        .map_err(SessionError::Export)?;
        self.last_export = Some(written);
        Ok(())
    }

    fn apply_storage(&mut self, command: StorageCommand) -> Result<(), SessionError> {
        let recent = media::recent_window(&self.project);
        let applied = panic::catch_unwind(AssertUnwindSafe(|| match command {
            StorageCommand::PlanCollection => {
                let records = media::catalog(&self.project).map_err(SessionError::Media)?;
                self.collection = Some(media::plan(&self.project, &records, recent));
                Ok(())
            }
            StorageCommand::Collect => {
                let plan = self
                    .collection
                    .take()
                    .ok_or(SessionError::NoCollectionPlan)?;
                media::collect(&self.project, &plan, recent)
                    .map(|_| ())
                    .map_err(SessionError::Media)
            }
            StorageCommand::Verify => {
                let records = media::catalog(&self.project).map_err(SessionError::Media)?;
                self.mismatched = media::check(&self.project, &records, true)
                    .map_err(SessionError::Media)?
                    .mismatched;
                Ok(())
            }
        }));
        applied.unwrap_or(Err(SessionError::Panicked))
    }

    fn publish_workflow(&self, view: &View) {
        let read = || -> Result<_, String> {
            Ok((
                workflow::status(&self.project).map_err(|error| error.to_string())?,
                workflow::scenes(&self.project).map_err(|error| error.to_string())?,
                export::readiness(&self.project).map_err(|error| error.to_string())?,
            ))
        };
        let failed = |problem: String| PublishedWorkflow {
            steps: Vec::new(),
            scenes: Vec::new(),
            export: export::Readiness::default(),
            last_export: self.last_export.clone(),
            problem: Some(problem),
        };
        let published = match panic::catch_unwind(AssertUnwindSafe(read)) {
            Ok(Ok((steps, scenes, export))) => PublishedWorkflow {
                steps,
                scenes,
                export,
                last_export: self.last_export.clone(),
                problem: None,
            },
            Ok(Err(problem)) => failed(problem),
            Err(_) => failed("working out the steps panicked".to_owned()),
        };
        *lock(&view.workflow) = Some(Arc::new(published));
    }

    fn publish_production(&self, view: &View) {
        let published = panic::catch_unwind(AssertUnwindSafe(|| self.read_production()))
            .unwrap_or_else(|_| Err(SessionError::Panicked))
            .unwrap_or_else(|error| PublishedProduction {
                damaged: Some(error.to_string()),
                ..PublishedProduction::default()
            });
        *lock(&view.production) = Some(Arc::new(published));
    }

    fn read_production(&self) -> Result<PublishedProduction, SessionError> {
        let project = &self.project;
        let style_error = |error| SessionError::Production(ProductionError::Style(error));
        let cast_error = |error| SessionError::Production(ProductionError::Cast(error));
        // Media paths were checked when their records were read.
        let at = |media_path: &str| {
            ProjectRelativePath::new(media_path)
                .map(|path| project.root().resolve(&path))
                .unwrap_or_default()
        };
        let samples = style_document::samples(project)
            .map_err(style_error)?
            .into_iter()
            .map(|sample| {
                let media = at(&sample.media_path);
                (sample, media)
            })
            .collect();
        let bible = revisions::current(project, DocumentKind::Bible)
            .map_err(|error| SessionError::Document(DocumentError::Revision(error)))?
            .map(|record| revisions::read::<Bible>(project, &record))
            .transpose()
            .map_err(|error| SessionError::Document(DocumentError::Revision(error)))?;
        let mut characters = Vec::new();
        for id in cast::main_characters(project).map_err(generation_error)? {
            let name = bible
                .as_ref()
                .and_then(|bible| bible.characters.iter().find(|character| character.id == id))
                .map_or_else(|| id.clone(), |character| character.name.clone());
            let images = cast_document::images(project, &id)
                .map_err(cast_error)?
                .into_iter()
                .map(|image| {
                    let media = at(&image.media_path);
                    (image, media)
                })
                .collect();
            let voice_error = |error| SessionError::Production(ProductionError::Voice(error));
            let voice = PublishedVoice {
                auditions: voice_document::auditions(project, &id)
                    .map_err(voice_error)?
                    .into_iter()
                    .map(|audition| {
                        let media = at(&audition.media_path);
                        (audition, media)
                    })
                    .collect(),
                lock: voice_document::locked(project, &id).map_err(voice_error)?,
                stale: voice::staleness(project, &id).map_err(generation_error)?,
                unresolved: self.voice_unresolved.contains(&id),
                default_line: voice::default_line(
                    project,
                    &id,
                    &project.manifest().defaults.language,
                )
                .map_err(generation_error)?,
            };
            characters.push(PublishedCharacter {
                lock: cast_document::locked(project, &id).map_err(cast_error)?,
                stale: cast::staleness(project, &id).map_err(generation_error)?,
                unresolved: self.cast_unresolved.contains(&id),
                id,
                name,
                images,
                voice,
            });
        }
        let (voices, voices_problem) = match &self.voices {
            Some(Ok(voices)) => (voices.clone(), None),
            Some(Err(problem)) => (Vec::new(), Some(problem.clone())),
            None => (Vec::new(), None),
        };
        Ok(PublishedProduction {
            samples,
            style: style_document::locked(project).map_err(style_error)?,
            style_unresolved: self.style_unresolved,
            characters,
            voices,
            voices_problem,
            damaged: None,
        })
    }

    /// Which shots are in play: those of the locked plan, written into the
    /// project first when `materialize` (as a project opens), or the
    /// bootstrap scene's. Returns whether the list changed; a list that
    /// cannot be read leaves none in play.
    fn find_shots(&mut self, materialize: bool) -> bool {
        if materialize && !self.project.is_read_only() {
            // A plan locked by a session that stopped before writing its
            // shots gets them now; failure leaves what is on disk.
            let _ = shot_plan_document::materialize(&self.project);
        }
        let keys = shot_plan_document::shots_in_play(&self.project).unwrap_or_default();
        if keys == self.keys {
            return false;
        }
        self.keys = keys;
        true
    }

    fn index_of(&self, key: &ShotKey) -> Option<usize> {
        self.keys.iter().position(|in_play| in_play == key)
    }

    /// Publish every shot in play, replacing the list.
    fn publish_shots(&mut self, view: &View) {
        let keys = self.keys.clone();
        *lock(&view.shots) = Some(keys.iter().map(|key| (key.clone(), None)).collect());
        for index in 0..keys.len() {
            self.publish(view, index);
        }
    }

    fn publish(&mut self, view: &View, shot_index: usize) {
        let Some(key) = self.keys.get(shot_index).cloned() else {
            return;
        };
        let video_provider = self.videos.provider.clone();
        let published = panic::catch_unwind(AssertUnwindSafe(|| {
            self.with_shot(&key, |project, _, shot| {
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
                    video_unresolved: false,
                    origins: shot.candidate_origins()?,
                    keyframes_unresolved: false,
                    staleness: generation::keyframe_staleness(project, shot)
                        .map_err(|_| ShotWorkflowError::InvalidCanonicalState)?,
                    history: shot
                        .candidate_history()?
                        .into_iter()
                        .map(|record| {
                            let media = record
                                .media_path
                                .as_ref()
                                .map(|path| project.root().resolve(path));
                            (record, media)
                        })
                        .collect(),
                    videos: shot
                        .video_history()?
                        .into_iter()
                        .map(|record| {
                            let media = record
                                .media_path
                                .as_ref()
                                .map(|path| project.root().resolve(path));
                            (record, media)
                        })
                        .collect(),
                    video_estimate: video_provider
                        .estimate(shot.intent()?.and_then(|intent| intent.duration_target)),
                })
            })
        }))
        .unwrap_or(Err(ShotWorkflowError::InvalidCanonicalState))
        .map(|mut published| {
            published.video_in_flight = self.videos.follows(&key);
            published.video_unresolved = self.videos.unresolved.contains(&key);
            published.keyframes_unresolved = self.keyframes_unresolved.contains(&key);
            published
        })
        .map_err(Arc::new);
        if let Some(slot) = lock(&view.shots)
            .as_mut()
            .and_then(|shots| shots.get_mut(shot_index))
            .filter(|(published_key, _)| *published_key == key)
        {
            slot.1 = Some(published);
        }
    }

    fn with_shot<R, E: From<ShotWorkflowError>>(
        &mut self,
        key: &ShotKey,
        operation: impl FnOnce(&CanonicalProject, &ImageProvider, &mut ShotWorkflow) -> Result<R, E>,
    ) -> Result<R, E> {
        let shot = shot_mut(&self.project, &mut self.shots, key)?;
        operation(&self.project, &self.provider, shot)
    }
}

/// The video provider a session follows its image provider with: MiniMax
/// with the same key, otherwise the local demo.
fn video_provider_for(images: &ImageProvider) -> VideoProvider {
    match images {
        ImageProvider::Minimax(client) => VideoProvider::Minimax(client.clone()),
        ImageProvider::LocalDemo => VideoProvider::LocalDemo,
    }
}

/// The speech provider a session follows its image provider with: MiniMax
/// with the same key, otherwise the local demo.
fn speech_provider_for(images: &ImageProvider) -> SpeechProvider {
    match images {
        ImageProvider::Minimax(client) => SpeechProvider::Minimax(client.clone()),
        ImageProvider::LocalDemo => SpeechProvider::LocalDemo,
    }
}

/// The characters `unresolved` auditions belong to.
fn unresolved_characters(unresolved: &[String]) -> BTreeSet<String> {
    unresolved
        .iter()
        .filter_map(|id| voice_document::character_of(id))
        .map(str::to_owned)
        .collect()
}

/// The shot's cached workflow, loaded from its canonical files on first use.
fn shot_mut<'a>(
    project: &CanonicalProject,
    shots: &'a mut BTreeMap<ShotKey, ShotWorkflow>,
    key: &ShotKey,
) -> Result<&'a mut ShotWorkflow, ShotWorkflowError> {
    if !shots.contains_key(key) {
        let shot = ShotWorkflow::load(project, key)?;
        shots.insert(key.clone(), shot);
    }
    Ok(shots
        .get_mut(key)
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
        DocumentCompleted {
            token: u64,
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

        fn document_completed(&self, token: u64, result: &Result<(), SessionError>) {
            let _ = lock(&self.0).send(Event::DocumentCompleted {
                token,
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
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
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

        use crate::video::VideoProviderKind;
        use kineto_project::ArtifactStatus;

        const MP4: &[u8] = b"\0\0\0\x18ftypisom\0\0\x02\0isommp41-video";

        /// MiniMax for images and Hailuo video, offline. A video task stays
        /// processing until `finish` is set.
        #[derive(Default)]
        struct FakeMinimax {
            images: AtomicU32,
            submits: AtomicU32,
            finish: AtomicBool,
            /// Submissions to refuse with a rate-limit answer before accepting.
            refuse: AtomicU32,
            /// Submissions to time out after sending before accepting.
            time_out: AtomicU32,
            /// From this submission on (counting from 1), every one times out
            /// after sending; 0 for never.
            time_out_from: AtomicU32,
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
                    let submit = self.submits.fetch_add(1, Ordering::SeqCst) + 1;
                    let from = self.time_out_from.load(Ordering::SeqCst);
                    if from != 0 && submit >= from {
                        return Err(TransportFailure::Timeout);
                    }
                    let take = |count: &AtomicU32| {
                        count
                            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                            .is_ok()
                    };
                    if take(&self.refuse) {
                        return answer(
                            r#"{"task_id":"","base_resp":{"status_code":1002,"status_msg":"rate limit"}}"#,
                        );
                    }
                    if take(&self.time_out) {
                        return Err(TransportFailure::Timeout);
                    }
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
                ..SessionOptions::default()
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
                    if point == video::VideoPoint::Recorded {
                        panic!("the intent save fails after MiniMax accepted the task");
                    }
                }
            }

            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeMinimax::default());
            drop(locked_keyframe(&target, &fake));

            let project = CanonicalProject::open(&target).unwrap();
            let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
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
                let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
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

        #[test]
        fn a_submission_accepted_before_anything_was_written_is_never_submitted_again_unasked() {
            struct CrashOnAccept;
            impl video::VideoHooks for CrashOnAccept {
                fn reached(&mut self, point: video::VideoPoint) {
                    if point == video::VideoPoint::Accepted {
                        panic!("the process dies as MiniMax accepts the task");
                    }
                }
            }

            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeMinimax::default());
            drop(locked_keyframe(&target, &fake));
            {
                let project = CanonicalProject::open(&target).unwrap();
                let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
                let ImageProvider::Minimax(Some(client)) = minimax(&fake) else {
                    unreachable!();
                };
                let crashed = panic::catch_unwind(AssertUnwindSafe(|| {
                    video::start_with(&project, &shot, &client, &mut CrashOnAccept)
                }));
                assert!(crashed.is_err());
            }
            assert_eq!(fake.submits.load(Ordering::SeqCst), 1);
            let intent = video_intent(&target);
            assert_eq!(
                (intent["state"].as_str(), intent["attempts"].as_u64()),
                (Some("prepared"), Some(1))
            );
            assert!(video_receipt(&target)["task_id"].is_null());

            // The reopened session cannot know whether MiniMax charged.
            let (session, events) = spawn(OpenRequest::Open { path: target }, &fake);
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert!(session.video_unresolved(0).unwrap());
            assert!(!session.video_in_flight(0).unwrap());
            assert_eq!(
                command(&session, &events, 1, ShotCommand::GenerateVideo),
                Err("Generation(UnresolvedSubmission)".to_owned())
            );
            assert_eq!(
                fake.submits.load(Ordering::SeqCst),
                1,
                "never paid twice unasked"
            );

            // The director decides to risk it.
            command(&session, &events, 2, ShotCommand::ResubmitVideo).unwrap();
            assert_eq!(fake.submits.load(Ordering::SeqCst), 2);
            assert!(!session.video_unresolved(0).unwrap());
            assert!(session.video_in_flight(0).unwrap());
            fake.finish.store(true, Ordering::SeqCst);
            assert_eq!(
                events.recv_timeout(WAIT).unwrap(),
                Event::VideoFinished {
                    shot_index: 0,
                    revision: 1,
                    result: Ok(())
                }
            );
        }

        #[test]
        fn a_refused_submission_is_asked_for_again_and_a_timed_out_one_is_not() {
            let temp = TempDir::new();
            let fake = Arc::new(FakeMinimax::default());
            let (session, events) = locked_keyframe(&temp.0.join("film"), &fake);

            // MiniMax answered with a refusal: nothing was charged.
            fake.refuse.store(1, Ordering::SeqCst);
            let refused = command(&session, &events, 4, ShotCommand::GenerateVideo).unwrap_err();
            assert!(refused.contains("Provider"), "{refused}");
            assert!(!session.video_unresolved(0).unwrap());

            // The request went out and no answer came back: it may have been.
            fake.time_out.store(1, Ordering::SeqCst);
            assert_eq!(
                command(&session, &events, 5, ShotCommand::GenerateVideo),
                Err("Generation(UnresolvedSubmission)".to_owned())
            );
            assert!(session.video_unresolved(0).unwrap());
            assert_eq!(
                command(&session, &events, 6, ShotCommand::GenerateVideo),
                Err("Generation(UnresolvedSubmission)".to_owned())
            );
            assert_eq!(fake.submits.load(Ordering::SeqCst), 2);

            command(&session, &events, 7, ShotCommand::ResubmitVideo).unwrap();
            assert_eq!(fake.submits.load(Ordering::SeqCst), 3);
            assert!(session.video_in_flight(0).unwrap());
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

        /// Wait for `count` videos to finish, whatever else arrives.
        fn videos_finished(events: &Receiver<Event>, count: usize) -> Vec<Result<(), String>> {
            let mut results = Vec::new();
            while results.len() < count {
                if let Event::VideoFinished { result, .. } = events.recv_timeout(WAIT).unwrap() {
                    results.push(result);
                }
            }
            results
        }

        fn ask(count: u8, note: Option<&str>, branch: Option<&str>) -> ShotCommand {
            ShotCommand::GenerateVideos(VideoAsk {
                count,
                note: note.map(str::to_owned),
                branch_from: branch.map(str::to_owned),
            })
        }

        #[test]
        fn candidates_already_submitted_are_followed_and_one_left_unresolved_is_not_paid_again() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeMinimax::default());
            let (session, events) = locked_keyframe(&target, &fake);
            // The third candidate's submission times out after sending.
            fake.time_out_from.store(3, Ordering::SeqCst);
            assert!(
                command(&session, &events, 4, ask(3, None, None))
                    .unwrap_err()
                    .contains("UnresolvedSubmission")
            );
            assert_eq!(fake.submits.load(Ordering::SeqCst), 3);
            assert!(session.video_in_flight(0).unwrap());
            assert!(session.video_unresolved(0).unwrap());
            // Asking again while two are being made is refused.
            assert_eq!(
                command(&session, &events, 5, ask(3, None, None)),
                Err("VideoInFlight".to_owned())
            );
            drop(session);

            // The app closed before they finished. Reopened, the two submitted
            // candidates are followed to the end; nothing is submitted.
            fake.finish.store(true, Ordering::SeqCst);
            fake.time_out_from.store(0, Ordering::SeqCst);
            let (session, events) = spawn(OpenRequest::Open { path: target }, &fake);
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(videos_finished(&events, 2), [Ok(()), Ok(())]);
            assert_eq!(fake.submits.load(Ordering::SeqCst), 3);
            let history = session.video_history(0).unwrap();
            assert_eq!(
                history
                    .iter()
                    .map(|(take, _)| take.artifact_id.as_str())
                    .collect::<Vec<_>>(),
                ["shot_001_v0001_candidate_a", "shot_001_v0001_candidate_b"]
            );
            // The generation has its videos; the lost candidate is no longer
            // the shot's to resubmit.
            assert!(!session.video_unresolved(0).unwrap());
            assert_eq!(
                command(&session, &events, 6, ShotCommand::LockVideo),
                Err("Shot(NoSelection)".to_owned())
            );
            command(&session, &events, 7, ShotCommand::SelectVideo(1)).unwrap();
            command(&session, &events, 8, ShotCommand::LockVideo).unwrap();
            let history = session.video_history(0).unwrap();
            assert_eq!(history[1].0.status, ArtifactStatus::Locked);
            assert_eq!(history[0].0.status, ArtifactStatus::Candidate);
            assert_eq!(fake.submits.load(Ordering::SeqCst), 3);
        }

        #[test]
        fn takes_are_compared_refined_branched_chosen_and_complete_their_scene() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let (session, events) = create(&target);
            let mut token = 0;
            let mut run = |shot: u32, command: ShotCommand| {
                token += 1;
                session.submit(shot, command, token).unwrap();
                completion(&events, token)
            };
            for shot in [0, 1] {
                for command in [
                    ShotCommand::Generate,
                    ShotCommand::Select(0),
                    ShotCommand::Lock,
                ] {
                    run(shot, command).unwrap();
                }
            }

            // Two takes, then one refined by a note, then a branch from it.
            run(0, ask(2, None, None)).unwrap();
            assert_eq!(videos_finished(&events, 2), [Ok(()), Ok(())]);
            run(0, ask(1, Some("Slower."), None)).unwrap();
            videos_finished(&events, 1);
            run(
                0,
                ask(
                    1,
                    Some("Hold on his eyes"),
                    Some("shot_001_v0002_candidate_a"),
                ),
            )
            .unwrap();
            videos_finished(&events, 1);
            assert!(
                run(0, ask(1, None, Some("shot_001_v0009_candidate_a")))
                    .unwrap_err()
                    .contains("InvalidCandidate")
            );
            let history = session.video_history(0).unwrap();
            let takes: Vec<_> = history
                .iter()
                .map(|(take, media)| {
                    (
                        take.artifact_id.as_str(),
                        take.generation,
                        take.candidate,
                        take.status,
                        take.notes.clone(),
                        take.branched_from.clone(),
                        media.as_ref().is_some_and(|media| media.is_file()),
                    )
                })
                .collect();
            let superseded = ArtifactStatus::Superseded;
            assert_eq!(
                takes,
                [
                    (
                        "shot_001_v0001_candidate_a",
                        1,
                        Some(0),
                        superseded,
                        vec![],
                        None,
                        true
                    ),
                    (
                        "shot_001_v0001_candidate_b",
                        1,
                        Some(1),
                        superseded,
                        vec![],
                        None,
                        true
                    ),
                    (
                        "shot_001_v0002_candidate_a",
                        2,
                        Some(0),
                        superseded,
                        vec!["Slower.".to_owned()],
                        None,
                        true
                    ),
                    (
                        "shot_001_v0003_candidate_a",
                        3,
                        Some(0),
                        ArtifactStatus::Candidate,
                        vec!["Slower.".to_owned(), "Hold on his eyes".to_owned()],
                        Some("shot_001_v0002_candidate_a".to_owned()),
                        true
                    ),
                ]
            );
            let artifacts: serde_json::Value = serde_json::from_slice(
                &fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap(),
            )
            .unwrap();
            let branch = artifacts
                .as_array()
                .unwrap()
                .iter()
                .find(|artifact| artifact["artifact_id"] == "shot_001_v0003_candidate_a")
                .unwrap();
            assert!(
                branch["compiled_prompt"]
                    .as_str()
                    .unwrap()
                    .contains("Director's note: Slower; Hold on his eyes.")
            );

            // The sole take of the latest generation is locked; the scene
            // still needs the second shot's video.
            run(0, ShotCommand::LockVideo).unwrap();
            let scene = |session: &ProjectSession| session.workflow().unwrap().scenes[0].clone();
            let progress = scene(&session);
            assert_eq!(progress.scene_id, "scene_001");
            assert!(!progress.complete());
            assert_eq!(
                progress
                    .shots
                    .iter()
                    .map(|shot| (shot.shot_id.as_str(), shot.keyframe, shot.video))
                    .collect::<Vec<_>>(),
                [
                    ("shot_001", workflow::Stage::Locked, workflow::Stage::Locked),
                    (
                        "shot_002",
                        workflow::Stage::Locked,
                        workflow::Stage::Missing
                    ),
                ]
            );

            // Of two takes the director chooses the second.
            run(1, ask(2, None, None)).unwrap();
            videos_finished(&events, 2);
            assert!(
                run(1, ShotCommand::LockVideo)
                    .unwrap_err()
                    .contains("NoSelection")
            );
            run(1, ShotCommand::SelectVideo(1)).unwrap();
            run(1, ShotCommand::LockVideo).unwrap();
            assert!(scene(&session).complete());
            let video_step = |session: &ProjectSession| {
                session
                    .workflow()
                    .unwrap()
                    .steps
                    .iter()
                    .find(|step| step.id == "video")
                    .unwrap()
                    .state
            };
            assert_eq!(video_step(&session), workflow::StepState::Done);
            drop(session);

            // Every take and choice is in the project's files.
            fs::remove_dir_all(target.join(".kineto")).unwrap();
            let (sink, events) = sink();
            let reopened = ProjectSession::spawn(
                OpenRequest::Open { path: target },
                ImageProvider::LocalDemo,
                sink,
            )
            .unwrap();
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(reopened.video_history(0).unwrap().len(), 4);
            assert_eq!(
                reopened.video_history(1).unwrap()[1].0.status,
                ArtifactStatus::Locked
            );
            assert!(scene(&reopened).complete());
            assert_eq!(video_step(&reopened), workflow::StepState::Done);
        }

        #[test]
        fn identical_takes_share_their_bytes_and_a_lost_file_is_reported_at_open() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let (session, events) = create(&target);
            for (token, command) in [
                (1, ShotCommand::Generate),
                (2, ShotCommand::Select(0)),
                (3, ShotCommand::Lock),
                (4, ask(2, None, None)),
            ] {
                command_ok(&session, &events, token, command);
            }
            videos_finished(&events, 2);
            let takes = session.video_history(0).unwrap();
            let paths: Vec<PathBuf> = takes
                .iter()
                .map(|(_, media)| media.clone().unwrap())
                .collect();
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let inode = |path: &PathBuf| fs::metadata(path).unwrap().ino();
                assert_eq!(
                    inode(&paths[0]),
                    inode(&paths[1]),
                    "the demo clip is stored once"
                );
            }
            let media = session.media().unwrap();
            assert!(media.missing.is_empty() && media.problem.is_none());
            drop(session);

            fs::remove_file(&paths[1]).unwrap();
            assert_eq!(fs::read(&paths[0]).unwrap(), video::DEMO_CLIP);
            let (sink, events) = sink();
            let reopened = ProjectSession::spawn(
                OpenRequest::Open { path: target },
                ImageProvider::LocalDemo,
                sink,
            )
            .unwrap();
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            let missing = &reopened.media().unwrap().missing;
            assert_eq!(missing.len(), 1);
            assert_eq!(missing[0].artifact_id, "shot_001_v0001_candidate_b");
            // Reported, not rewritten: the take still says its bytes are kept.
            assert_eq!(reopened.video_history(0).unwrap().len(), 2);
        }

        fn storage(
            session: &ProjectSession,
            events: &Receiver<Event>,
            token: u64,
            command: StorageCommand,
        ) -> Result<(), String> {
            session.submit_storage(command, token).unwrap();
            match events.recv_timeout(WAIT).unwrap() {
                Event::DocumentCompleted {
                    token: actual,
                    result,
                } => {
                    assert_eq!(actual, token);
                    result
                }
                other => panic!("expected a storage result, got {other:?}"),
            }
        }

        #[test]
        fn the_director_sees_what_collecting_lets_go_before_it_does() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let (session, events) = create(&target);
            for (token, command) in [
                (1, ShotCommand::Generate),
                (2, ShotCommand::Select(0)),
                (3, ShotCommand::Lock),
            ] {
                command_ok(&session, &events, token, command);
            }
            for token in 4..7 {
                command_ok(&session, &events, token, ask(1, None, None));
                videos_finished(&events, 1);
            }
            command_ok(&session, &events, 7, ShotCommand::LockVideo);
            assert_eq!(session.media().unwrap().recent, media::DEFAULT_RECENT);
            drop(session);

            // The project keeps only the latest take of each kind.
            let manifest = target.join("project.toml");
            let text = fs::read_to_string(&manifest).unwrap().replace(
                "recipe_version = 1\n",
                "recipe_version = 1\nrecent_takes = 1\n",
            );
            fs::write(&manifest, text).unwrap();
            let (sink, events) = sink();
            let session = ProjectSession::spawn(
                OpenRequest::Open {
                    path: target.clone(),
                },
                ImageProvider::LocalDemo,
                sink,
            )
            .unwrap();
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            let media = session.media().unwrap();
            assert_eq!(media.recent, 1);
            assert!(media.plan.is_none());
            assert!(media.usage.bytes > 0);
            assert_eq!(
                storage(&session, &events, 8, StorageCommand::Collect),
                Err("NoCollectionPlan".to_owned())
            );

            storage(&session, &events, 9, StorageCommand::PlanCollection).unwrap();
            let plan = session.media().unwrap().plan.clone().unwrap();
            let planned: Vec<_> = plan
                .items
                .iter()
                .map(|item| item.record.artifact_id.as_str())
                .collect();
            assert_eq!(
                planned,
                ["shot_001_v0001_candidate_a", "shot_001_v0002_candidate_a"]
            );
            // Nothing is touched by the dry run.
            assert!(session.video_history(0).unwrap()[0].1.is_some());

            storage(&session, &events, 10, StorageCommand::Collect).unwrap();
            let media = session.media().unwrap();
            assert!(media.plan.is_none() && media.missing.is_empty());
            let history = session.video_history(0).unwrap();
            assert!(history[0].1.is_none() && history[1].1.is_none());
            let locked = history[2].1.clone().unwrap();
            assert_eq!(fs::read(&locked).unwrap(), video::DEMO_CLIP);
            assert_eq!(history.len(), 3, "the records stay");

            // The locked take's bytes are changed behind Kineto's back.
            fs::write(&locked, b"\0\0\0\x18ftypisom-changed").unwrap();
            assert!(session.media().unwrap().mismatched.is_empty());
            storage(&session, &events, 11, StorageCommand::Verify).unwrap();
            let mismatched = &session.media().unwrap().mismatched;
            assert_eq!(mismatched.len(), 1);
            assert_eq!(mismatched[0].artifact_id, "shot_001_v0003_candidate_a");
        }

        #[test]
        fn a_finished_scene_is_handed_off_as_a_package() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let out = temp.0.join("out");
            fs::create_dir_all(&out).unwrap();
            let (session, events) = create(&target);
            let mut token = 0;
            let mut run = |shot: u32, command: ShotCommand| {
                token += 1;
                session.submit(shot, command, token).unwrap();
                completion(&events, token)
            };
            for shot in [0, 1] {
                for command in [
                    ShotCommand::Generate,
                    ShotCommand::Select(0),
                    ShotCommand::Lock,
                    ShotCommand::GenerateVideo,
                ] {
                    run(shot, command).unwrap();
                }
                videos_finished(&events, 1);
            }
            run(0, ShotCommand::LockVideo).unwrap();
            let workflow = session.workflow().unwrap();
            assert_eq!(workflow.export.ready, [(ShotKey::bootstrap(1), 1)]);
            assert_eq!(workflow.export.missing[0].reason, "its video is not locked");
            let request = |partial| export::ExportRequest {
                destination: out.clone(),
                name: "dailies".to_owned(),
                partial,
                replace: false,
            };
            let export = |request: export::ExportRequest, token: u64| {
                session.submit_export(request, token).unwrap();
                match events.recv_timeout(WAIT).unwrap() {
                    Event::DocumentCompleted { result, .. } => result,
                    other => panic!("expected the export's result, got {other:?}"),
                }
            };
            assert!(
                export(request(false), 100)
                    .unwrap_err()
                    .starts_with("Export(Incomplete(")
            );
            assert!(session.workflow().unwrap().last_export.is_none());

            run(1, ShotCommand::LockVideo).unwrap();
            assert!(session.workflow().unwrap().export.complete());
            export(request(false), 101).unwrap();
            let exported = session.workflow().unwrap().last_export.clone().unwrap();
            assert_eq!(exported.path, out.join("dailies"));
            assert!(exported.complete && exported.shots == 2);
            assert!(out.join("dailies/manifest.json").is_file());
            assert_eq!(
                fs::read(out.join("dailies/scene_001_shot_002.mp4")).unwrap(),
                video::DEMO_CLIP
            );
        }

        fn command_ok(
            session: &ProjectSession,
            events: &Receiver<Event>,
            token: u64,
            command: ShotCommand,
        ) {
            session.submit(0, command, token).unwrap();
            assert_eq!(completion(events, token), Ok(()));
        }

        #[test]
        fn the_demo_makes_a_free_video_from_its_own_keyframe_and_minimax_refuses_it() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let (session, events) = create(&target);
            assert_eq!(session.video_provider().kind, VideoProviderKind::LocalDemo);
            assert_eq!(session.video_provider().clip_cost_micros, 0);
            assert_eq!(
                session.speech_provider(),
                crate::speech::SpeechProviderInfo {
                    kind: crate::speech::SpeechProviderKind::LocalDemo,
                    ready: true,
                    character_cost_micros: Some(0),
                }
            );
            assert!(
                command(&session, &events, 1, ShotCommand::GenerateVideo)
                    .unwrap_err()
                    .contains("KeyframeNotLocked")
            );
            command(&session, &events, 2, ShotCommand::Generate).unwrap();
            command(&session, &events, 3, ShotCommand::Select(0)).unwrap();
            command(&session, &events, 4, ShotCommand::Lock).unwrap();
            // The demo keyframe has no image; the demo video needs none, and
            // runs through the same job as a paid one.
            command(&session, &events, 5, ShotCommand::GenerateVideo).unwrap();
            assert_eq!(
                events.recv_timeout(WAIT).unwrap(),
                Event::VideoFinished {
                    shot_index: 0,
                    revision: 1,
                    result: Ok(())
                }
            );
            let media = session.video_media(0).unwrap().unwrap();
            assert_eq!(fs::read(&media).unwrap(), video::DEMO_CLIP);
            command(&session, &events, 6, ShotCommand::LockVideo).unwrap();
            assert_eq!(
                session.snapshot(0).unwrap().video,
                kineto_project::shot::VideoState::Locked
            );
            let artifacts: serde_json::Value = serde_json::from_slice(
                &fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap(),
            )
            .unwrap();
            let video = artifacts
                .as_array()
                .unwrap()
                .iter()
                .find(|artifact| artifact["artifact_id"] == "shot_001_v0001_candidate_a")
                .unwrap();
            assert_eq!(video["provider"], "local-demo-video");
            assert_eq!(video["model"], "demo-clip");
            for (token, command) in [
                (7, ShotCommand::Generate),
                (8, ShotCommand::Select(0)),
                (9, ShotCommand::Lock),
            ] {
                session.submit(1, command, token).unwrap();
                assert_eq!(completion(&events, token), Ok(()));
            }
            drop(session);

            // MiniMax starts only from an image, which a demo keyframe has not.
            let fake = Arc::new(FakeMinimax::default());
            let (session, events) = spawn(OpenRequest::Open { path: target }, &fake);
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(session.video_provider().kind, VideoProviderKind::Minimax);
            assert_eq!(
                session.speech_provider().kind,
                crate::speech::SpeechProviderKind::Minimax
            );
            assert_eq!(session.speech_provider().character_cost_micros, Some(100));
            session.submit(1, ShotCommand::GenerateVideo, 10).unwrap();
            assert!(
                completion(&events, 10)
                    .unwrap_err()
                    .contains("NoFirstFrame")
            );
            drop(session);
            assert_eq!(fake.submits.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn a_video_needs_a_real_locked_keyframe() {
            let temp = TempDir::new();
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

            fn document_completed(&self, _: u64, _: &Result<(), SessionError>) {}
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

    mod documents {
        use super::*;
        use kineto_llm::fake::{FakeLlm, FakeLlmError};
        use kineto_project::{
            revisions::Origin,
            screenplay::{Block, BlockKind, SAMPLE_SCREENPLAY_JSON},
            story::SAMPLE_ANALYSIS_JSON,
        };

        fn open_with(
            path: &std::path::Path,
            request: OpenRequest,
            model: DocumentModel,
        ) -> (ProjectSession, Receiver<Event>) {
            let _ = path;
            let (sink, events) = sink();
            let session = ProjectSession::spawn_with_model(
                request,
                ImageProvider::LocalDemo,
                model,
                sink,
                SessionOptions::default(),
            )
            .unwrap();
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            (session, events)
        }

        fn create_with(
            path: &std::path::Path,
            model: DocumentModel,
        ) -> (ProjectSession, Receiver<Event>) {
            open_with(
                path,
                OpenRequest::CreateText {
                    path: path.to_path_buf(),
                    manifest: Box::new(manifest()),
                    source: b"A filmmaker hears the last line and leaves.\n".to_vec(),
                },
                model,
            )
        }

        fn run(
            session: &ProjectSession,
            events: &Receiver<Event>,
            command: DocumentCommand,
        ) -> Result<(), String> {
            send(session, events, command, false)
        }

        fn send(
            session: &ProjectSession,
            events: &Receiver<Event>,
            command: DocumentCommand,
            resubmit: bool,
        ) -> Result<(), String> {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let token = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if resubmit {
                session.resubmit_document(command, token).unwrap();
            } else {
                session.submit_document(command, token).unwrap();
            }
            match events.recv_timeout(WAIT).unwrap() {
                Event::DocumentCompleted {
                    token: actual,
                    result,
                } => {
                    assert_eq!(actual, token);
                    result
                }
                other => panic!("expected a document completion, got {other:?}"),
            }
        }

        fn edit(based_on: u32, line: &str) -> DocumentCommand {
            DocumentCommand::SaveScene {
                based_on,
                edit: SceneEdit {
                    scene_id: "office_night".to_owned(),
                    heading: "INT. SMALL OFFICE - NIGHT".to_owned(),
                    time: "night".to_owned(),
                    dramatic_goal: "He answers after all.".to_owned(),
                    blocks: vec![Block {
                        kind: BlockKind::Dialogue,
                        character: Some("filmmaker".to_owned()),
                        parenthetical: None,
                        text: line.to_owned(),
                    }],
                },
            }
        }

        #[test]
        fn an_unresolved_paid_request_waits_for_the_director_to_resubmit() {
            let temp = TempDir::new();
            let model = DocumentModel::Fake(
                FakeLlm::scripted([
                    Err(FakeLlmError::Retryable),
                    Ok(SAMPLE_ANALYSIS_JSON.to_owned()),
                ])
                .metered(),
            );
            let (session, events) = create_with(&temp.0.join("film"), model);
            assert!(run(&session, &events, DocumentCommand::AnalyzeStory).is_err());
            assert_eq!(
                run(&session, &events, DocumentCommand::AnalyzeStory),
                Err("Generation(UnresolvedSubmission)".to_owned())
            );
            assert!(session.documents().unwrap().analysis.is_none());
            send(&session, &events, DocumentCommand::AnalyzeStory, true).unwrap();
            assert_eq!(
                session
                    .documents()
                    .unwrap()
                    .analysis
                    .as_ref()
                    .unwrap()
                    .revision,
                1
            );
        }

        #[test]
        fn the_director_takes_a_story_to_a_locked_screenplay() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let model = DocumentModel::Fake(FakeLlm::answering([
                SAMPLE_ANALYSIS_JSON,
                SAMPLE_SCREENPLAY_JSON,
            ]));
            let (session, events) = create_with(&target, model);
            let documents = session.documents().unwrap();
            assert_eq!(documents.source_revision, Some(1));
            assert!(documents.analysis.is_none());
            assert_eq!(
                documents.analysis_cost,
                Some(0),
                "a free model costs nothing"
            );
            assert_eq!(documents.screenplay_cost, None, "nothing to write from yet");

            assert_eq!(
                run(&session, &events, DocumentCommand::WriteScreenplay),
                Err("Generation(Prerequisite(StoryAnalysis))".to_owned())
            );
            run(&session, &events, DocumentCommand::AnalyzeStory).unwrap();
            let documents = session.documents().unwrap();
            assert_eq!(documents.analysis.as_ref().unwrap().revision, 1);
            assert_eq!(documents.screenplay_cost, Some(0));

            run(&session, &events, DocumentCommand::WriteScreenplay).unwrap();
            run(&session, &events, edit(1, "It was.")).unwrap();
            assert!(
                run(&session, &events, edit(1, "It was not."))
                    .unwrap_err()
                    .contains("NotLatest")
            );
            run(
                &session,
                &events,
                DocumentCommand::LockScreenplay { revision: 2 },
            )
            .unwrap();

            let documents = session.documents().unwrap();
            let (revision, screenplay) = documents.screenplay.as_ref().unwrap();
            assert_eq!(*revision, 2);
            assert_eq!(screenplay.scenes[0].blocks[0].text, "It was.");
            assert_eq!(documents.locked, Some(2));
            assert_eq!(documents.revisions[1].origin, Origin::Edited);
            assert!(!documents.screenplay_stale);
            assert!(target.join("screenplay/screenplay.md").is_file());

            // A new source makes the analysis, and so the screenplay, stale.
            run(
                &session,
                &events,
                DocumentCommand::ImportSource {
                    text: "A filmmaker hears the last line and stays.".to_owned(),
                    format: SourceFormat::Text,
                    original_name: None,
                },
            )
            .unwrap();
            let documents = session.documents().unwrap();
            assert_eq!(documents.source_revision, Some(2));
            assert!(documents.analysis_stale);
            assert!(
                !documents.screenplay_stale,
                "the analysis itself is unchanged"
            );
        }

        #[test]
        fn without_a_model_documents_are_refused_and_nothing_is_asked() {
            let temp = TempDir::new();
            let (session, events) = create_with(&temp.0.join("film"), DocumentModel::None);
            assert!(!session.document_model().ready);
            assert_eq!(
                run(&session, &events, DocumentCommand::AnalyzeStory),
                Err("Generation(ModelNotConfigured)".to_owned())
            );
            assert!(
                run(
                    &session,
                    &events,
                    DocumentCommand::ImportSource {
                        text: "   ".to_owned(),
                        format: SourceFormat::Text,
                        original_name: None,
                    },
                )
                .unwrap_err()
                .contains("Empty")
            );
            assert_eq!(session.documents().unwrap().analysis_cost, None);
        }

        #[test]
        fn an_answer_kept_before_a_crash_is_recovered_when_the_project_opens() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let model = DocumentModel::Fake(FakeLlm::answering([SAMPLE_ANALYSIS_JSON]));
            let (session, events) = create_with(&target, model);
            run(&session, &events, DocumentCommand::AnalyzeStory).unwrap();
            drop(session);
            // Undo the attach and acknowledgement, as if the process had died
            // just after keeping the answer.
            let log = target.join("story/analysis/revisions.json");
            let record: serde_json::Value =
                serde_json::from_slice(&fs::read(&log).unwrap()).unwrap();
            fs::write(
                target.join(".kineto/generation/story_analysis_r0001.json"),
                serde_json::to_vec(&("story_analysis", &record["revisions"][0])).unwrap(),
            )
            .unwrap();
            fs::remove_file(&log).unwrap();
            let intent = target.join(".kineto/jobs/intents/story_analysis_r0001.json");
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&intent).unwrap()).unwrap();
            value["state"] = "invoked".into();
            fs::write(&intent, serde_json::to_vec(&value).unwrap()).unwrap();

            let (sink, events) = sink();
            let reopened = ProjectSession::spawn_with_model(
                OpenRequest::Open { path: target },
                ImageProvider::LocalDemo,
                // Recovery never asks a model: this one has no answers left.
                DocumentModel::Fake(FakeLlm::answering(Vec::<String>::new())),
                sink,
                SessionOptions::default(),
            )
            .unwrap();
            assert_eq!(
                events.recv_timeout(WAIT).unwrap(),
                Event::Recovered("Document { kind: StoryAnalysis, revision: 1 }".to_owned())
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(
                reopened
                    .documents()
                    .unwrap()
                    .analysis
                    .as_ref()
                    .unwrap()
                    .revision,
                1
            );
        }
    }

    mod production {
        use super::*;
        use kineto_llm::fake::FakeLlm;
        use kineto_project::{
            bible::{EntryKind, SAMPLE_BIBLE_JSON},
            cast::PortraitStatus,
            revisions::Origin,
            screenplay::SAMPLE_SCREENPLAY_JSON,
            story::SAMPLE_ANALYSIS_JSON,
            style::SampleStatus,
        };
        use kineto_provider_minimax::{
            HttpResponse, MinimaxClient, SharedTransport, Transport, TransportFailure,
        };
        use kineto_providers::secret::SecretString;
        use std::sync::atomic::AtomicU32;

        /// MiniMax `image-01`, offline: a distinct JPEG per call.
        #[derive(Default)]
        struct FakeImages {
            calls: AtomicU32,
            /// Calls to time out after sending, leaving them unresolved.
            time_out: AtomicU32,
        }

        impl Transport for FakeImages {
            fn post_json(
                &self,
                _url: &str,
                _api_key: &SecretString,
                _body: &[u8],
            ) -> Result<HttpResponse, TransportFailure> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if self
                    .time_out
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                    .is_ok()
                {
                    return Err(TransportFailure::Timeout);
                }
                let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x52, u8::try_from(call).unwrap()];
                use base64::Engine as _;
                Ok(HttpResponse {
                    status: 200,
                    body: format!(
                        r#"{{"id":"i","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0}}}}"#,
                        base64::engine::general_purpose::STANDARD.encode(jpeg)
                    )
                    .into_bytes(),
                })
            }

            fn get(&self, url: &str, _: &SecretString) -> Result<HttpResponse, TransportFailure> {
                unreachable!("images make no GET request: {url}")
            }

            fn download(&self, url: &str, _: u64) -> Result<HttpResponse, TransportFailure> {
                unreachable!("images download nothing: {url}")
            }
        }

        fn spawn(
            request: OpenRequest,
            fake: &Arc<FakeImages>,
            model: DocumentModel,
        ) -> (ProjectSession, Receiver<Event>) {
            let transport: SharedTransport = Arc::clone(fake) as SharedTransport;
            let provider = ImageProvider::Minimax(Some(MinimaxClient::with_transport(
                transport,
                SecretString::new("sk-test".to_owned()).unwrap(),
                "https://example.test",
            )));
            let (sink, events) = sink();
            let session = ProjectSession::spawn_with_model(
                request,
                provider,
                model,
                sink,
                // Lines are spoken by the demo; the fake answers images only.
                SessionOptions {
                    speech_provider: Some(SpeechProvider::LocalDemo),
                    ..SessionOptions::default()
                },
            )
            .unwrap();
            (session, events)
        }

        fn token() -> u64 {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            NEXT.fetch_add(1, Ordering::Relaxed)
        }

        fn completion(events: &Receiver<Event>, token: u64) -> Result<(), String> {
            match events.recv_timeout(WAIT).unwrap() {
                Event::DocumentCompleted {
                    token: actual,
                    result,
                } => {
                    assert_eq!(actual, token);
                    result
                }
                other => panic!("expected a completion, got {other:?}"),
            }
        }

        fn document(
            session: &ProjectSession,
            events: &Receiver<Event>,
            command: DocumentCommand,
        ) -> Result<(), String> {
            let token = token();
            session.submit_document(command, token).unwrap();
            completion(events, token)
        }

        fn produce(
            session: &ProjectSession,
            events: &Receiver<Event>,
            command: ProductionCommand,
        ) -> Result<(), String> {
            let token = token();
            session.submit_production(command, token).unwrap();
            completion(events, token)
        }

        fn risk(
            session: &ProjectSession,
            events: &Receiver<Event>,
            command: ProductionCommand,
        ) -> Result<(), String> {
            let token = token();
            session.resubmit_production(command, token).unwrap();
            completion(events, token)
        }

        fn audition(character: &str, count: u32) -> ProductionCommand {
            ProductionCommand::Audition {
                character: character.to_owned(),
                count,
            }
        }

        /// A new film whose screenplay is locked and whose bible is written.
        fn written(
            target: &std::path::Path,
            fake: &Arc<FakeImages>,
        ) -> (ProjectSession, Receiver<Event>) {
            let model = DocumentModel::Fake(FakeLlm::answering([
                SAMPLE_ANALYSIS_JSON,
                SAMPLE_SCREENPLAY_JSON,
                SAMPLE_BIBLE_JSON,
            ]));
            let (session, events) = spawn(
                OpenRequest::CreateText {
                    path: target.to_path_buf(),
                    manifest: Box::new(manifest()),
                    source: b"A filmmaker hears the last line and leaves.\n".to_vec(),
                },
                fake,
                model,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            document(&session, &events, DocumentCommand::AnalyzeStory).unwrap();
            document(&session, &events, DocumentCommand::WriteScreenplay).unwrap();
            assert_eq!(
                session.documents().unwrap().bible_cost,
                None,
                "nothing locked yet"
            );
            document(
                &session,
                &events,
                DocumentCommand::LockScreenplay { revision: 1 },
            )
            .unwrap();
            document(&session, &events, DocumentCommand::WriteBible).unwrap();
            (session, events)
        }

        fn entry(appearance: &str) -> EntryEdit {
            EntryEdit {
                kind: EntryKind::Character,
                id: "filmmaker".to_owned(),
                description: "A documentary filmmaker in his fifties.".to_owned(),
                appearance: appearance.to_owned(),
                wardrobe: "A worn grey overcoat.".to_owned(),
                forbidden_changes: vec!["Never without the coat.".to_owned()],
            }
        }

        #[test]
        fn the_director_writes_a_bible_locks_a_style_and_casts_a_character() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeImages::default());
            let (session, events) = written(&target, &fake);

            let documents = session.documents().unwrap();
            let (revision, bible) = documents.bible.as_ref().unwrap();
            assert_eq!(*revision, 1);
            assert_eq!(bible.characters[0].id, "filmmaker");
            assert!(documents.bible_stale.is_empty());
            assert_eq!(documents.bible_locked, None);
            assert_eq!(documents.bible_revisions[0].origin, Origin::Generated);

            document(
                &session,
                &events,
                DocumentCommand::SaveBibleEntry {
                    based_on: 1,
                    edit: entry("Grey at the temples, a scar over one eyebrow."),
                },
            )
            .unwrap();
            assert_eq!(
                document(
                    &session,
                    &events,
                    DocumentCommand::SaveBibleEntry {
                        based_on: 1,
                        edit: entry("Older."),
                    },
                ),
                Err("Document(Revision(NotLatest { based_on: 1, latest: 2 }))".to_owned())
            );
            document(
                &session,
                &events,
                DocumentCommand::LockBible { revision: 2 },
            )
            .unwrap();
            let documents = session.documents().unwrap();
            assert_eq!(documents.bible_locked, Some(2));
            assert_eq!(documents.bible_revisions[1].origin, Origin::Edited);
            assert!(
                documents.bible.as_ref().unwrap().1.characters[0]
                    .appearance
                    .contains("scar")
            );

            let prices = session.image_prices();
            assert_eq!(prices.sample.unwrap().amount_micros, 3_500);
            assert_eq!(prices.pack.unwrap().0, 7);
            let production = session.production().unwrap();
            assert_eq!(
                production
                    .characters
                    .iter()
                    .map(|character| character.id.as_str())
                    .collect::<Vec<_>>(),
                ["filmmaker", "interviewer"]
            );
            assert!(production.samples.is_empty() && production.style.is_none());

            // Casting follows a locked style.
            assert_eq!(
                produce(&session, &events, audition("filmmaker", 1)),
                Err("Generation(Prerequisite(LockedStyle))".to_owned())
            );
            produce(
                &session,
                &events,
                ProductionCommand::SampleStyle {
                    preset: "film_noir".to_owned(),
                    note: None,
                },
            )
            .unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::RefineStyle {
                    sample: "style_s0001".to_owned(),
                    note: "Harder shadows.".to_owned(),
                },
            )
            .unwrap();
            assert_eq!(
                produce(
                    &session,
                    &events,
                    ProductionCommand::LockStyle("style_s0002".to_owned())
                ),
                Err("Production(Style(NotSelected(\"style_s0002\")))".to_owned())
            );
            produce(
                &session,
                &events,
                ProductionCommand::RejectStyle("style_s0001".to_owned()),
            )
            .unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::SelectStyle("style_s0002".to_owned()),
            )
            .unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::LockStyle("style_s0002".to_owned()),
            )
            .unwrap();
            let production = session.production().unwrap();
            let statuses: Vec<_> = production
                .samples
                .iter()
                .map(|(sample, _)| sample.status)
                .collect();
            assert_eq!(statuses, [SampleStatus::Rejected, SampleStatus::Selected]);
            assert!(production.samples[1].1.is_file());
            assert!(production.samples[1].1.starts_with(&target));
            assert_eq!(
                production.style.as_ref().unwrap().artifact_id,
                "style_s0002"
            );
            assert_eq!(fake.calls.load(Ordering::SeqCst), 2);

            produce(&session, &events, audition("filmmaker", 2)).unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::SelectPortrait {
                    character: "filmmaker".to_owned(),
                    portrait: "cast.filmmaker.0002".to_owned(),
                },
            )
            .unwrap();
            assert_eq!(
                produce(
                    &session,
                    &events,
                    ProductionCommand::LockCharacter {
                        character: "filmmaker".to_owned()
                    }
                ),
                Err(
                    "Generation(Provider { source: MissingReference(Front), class: Terminal })"
                        .to_owned()
                )
            );
            produce(
                &session,
                &events,
                ProductionCommand::MakePack {
                    character: "filmmaker".to_owned(),
                    remake: Vec::new(),
                },
            )
            .unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::LockCharacter {
                    character: "filmmaker".to_owned(),
                },
            )
            .unwrap();
            assert_eq!(fake.calls.load(Ordering::SeqCst), 11);
            let production = session.production().unwrap();
            let filmmaker = &production.characters[0];
            assert_eq!(filmmaker.name, "The Filmmaker");
            assert_eq!(filmmaker.images.len(), 9);
            assert_eq!(filmmaker.images[1].0.status, Some(PortraitStatus::Selected));
            let lock = filmmaker.lock.as_ref().unwrap();
            assert_eq!(lock.portrait.artifact_id, "cast.filmmaker.0002");
            assert_eq!(lock.references.len(), 7);
            assert!(filmmaker.stale.is_empty() && !filmmaker.unresolved);

            // The cast character's voice: the provider's voices listed free,
            // two auditions of its default line, one chosen and locked.
            assert!(session.production().unwrap().voices.is_empty());
            produce(&session, &events, ProductionCommand::LoadVoices).unwrap();
            assert_eq!(session.production().unwrap().voices.len(), 3);
            let choice = |voice_id: &str| VoiceChoice {
                voice: crate::speech::VoiceSettings::new(voice_id, "en"),
                name: voice_id.to_owned(),
            };
            produce(
                &session,
                &events,
                ProductionCommand::AuditionVoices {
                    character: "filmmaker".to_owned(),
                    choices: vec![choice("demo_low"), choice("demo_high")],
                    line: None,
                },
            )
            .unwrap();
            let production = session.production().unwrap();
            let voice = &production.characters[0].voice;
            assert_eq!(voice.default_line, "This is how I sound.");
            assert_eq!(voice.auditions.len(), 2);
            assert!(voice.auditions[1].1.is_file());
            assert_eq!(voice.auditions[1].0.line, "This is how I sound.");
            for command in [
                ProductionCommand::SelectVoice {
                    character: "filmmaker".to_owned(),
                    audition: "voice.filmmaker.0002".to_owned(),
                },
                ProductionCommand::LockVoice {
                    character: "filmmaker".to_owned(),
                },
            ] {
                produce(&session, &events, command).unwrap();
            }
            assert_eq!(
                produce(
                    &session,
                    &events,
                    ProductionCommand::RejectVoice {
                        character: "filmmaker".to_owned(),
                        audition: "voice.filmmaker.0002".to_owned(),
                    }
                ),
                Err("Production(Voice(Locked(\"voice.filmmaker.0002\")))".to_owned())
            );
            let voice = session.production().unwrap().characters[0].voice.clone();
            assert_eq!(
                voice.lock.as_ref().map(|lock| lock.voice.voice_id.as_str()),
                Some("demo_high")
            );
            assert!(voice.stale.is_empty() && !voice.unresolved);
            assert_eq!(
                fake.calls.load(Ordering::SeqCst),
                11,
                "speech is the demo's"
            );

            // Casting follows the locked bible: an edit of the entry the
            // cast was made from makes it stale once it is locked.
            document(
                &session,
                &events,
                DocumentCommand::SaveBibleEntry {
                    based_on: 2,
                    edit: entry("Grey at the temples, clean-shaven."),
                },
            )
            .unwrap();
            assert!(session.production().unwrap().characters[0].stale.is_empty());
            document(
                &session,
                &events,
                DocumentCommand::LockBible { revision: 3 },
            )
            .unwrap();
            assert_eq!(
                session.production().unwrap().characters[0].stale,
                ["bible_character.filmmaker"]
            );
            assert_eq!(
                session.production().unwrap().characters[0].voice.stale,
                ["bible_character.filmmaker"]
            );

            // Keyframes with what the director accepted.
            on_shot(
                &session,
                &events,
                ShotCommand::GenerateConsenting(Consent::default()),
            )
            .unwrap();
            assert_eq!(fake.calls.load(Ordering::SeqCst), 14);
            on_shot(&session, &events, ShotCommand::Select(0)).unwrap();

            // Another style is locked before the keyframe is: it was made in
            // the style the director moved on from, so it is not locked.
            for command in [
                ProductionCommand::SampleStyle {
                    preset: "film_realism".to_owned(),
                    note: None,
                },
                ProductionCommand::SelectStyle("style_s0003".to_owned()),
                ProductionCommand::LockStyle("style_s0003".to_owned()),
            ] {
                produce(&session, &events, command).unwrap();
            }
            assert_eq!(
                on_shot(&session, &events, ShotCommand::Lock),
                Err("Shot(StaleSelection)".to_owned())
            );
            assert!(!session.snapshot(0).unwrap().locked);

            // Made again in the new style, it is.
            on_shot(
                &session,
                &events,
                ShotCommand::GenerateConsenting(Consent::default()),
            )
            .unwrap();
            on_shot(&session, &events, ShotCommand::Select(0)).unwrap();
            on_shot(&session, &events, ShotCommand::Lock).unwrap();
            assert!(session.snapshot(0).unwrap().locked);
        }

        /// Submit `command` to the first shot and wait for its result.
        fn on_shot(
            session: &ProjectSession,
            events: &Receiver<Event>,
            command: ShotCommand,
        ) -> Result<(), String> {
            let token = token();
            session.submit(0, command, token).unwrap();
            match events.recv_timeout(WAIT).unwrap() {
                Event::Completed {
                    token: actual,
                    result,
                    ..
                } => {
                    assert_eq!(actual, token);
                    result
                }
                other => panic!("expected a shot completion, got {other:?}"),
            }
        }

        #[test]
        fn the_director_plans_edits_and_locks_the_shots() {
            use kineto_project::shot_plan::SAMPLE_SHOT_PLAN_JSON;
            let temp = TempDir::new();
            let fake = Arc::new(FakeImages::default());
            let model = DocumentModel::Fake(FakeLlm::answering([
                SAMPLE_ANALYSIS_JSON,
                SAMPLE_SCREENPLAY_JSON,
                SAMPLE_BIBLE_JSON,
                SAMPLE_SHOT_PLAN_JSON,
            ]));
            let (session, events) = spawn(
                OpenRequest::CreateText {
                    path: temp.0.join("film"),
                    manifest: Box::new(manifest()),
                    source: b"A filmmaker hears the last line and leaves.\n".to_vec(),
                },
                &fake,
                model,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            for command in [
                DocumentCommand::AnalyzeStory,
                DocumentCommand::WriteScreenplay,
                DocumentCommand::LockScreenplay { revision: 1 },
                DocumentCommand::WriteBible,
            ] {
                document(&session, &events, command).unwrap();
            }
            assert_eq!(
                document(&session, &events, DocumentCommand::PlanShots),
                Err("Generation(Prerequisite(LockedBible))".to_owned())
            );
            document(
                &session,
                &events,
                DocumentCommand::LockBible { revision: 1 },
            )
            .unwrap();
            document(&session, &events, DocumentCommand::PlanShots).unwrap();
            let documents = session.documents().unwrap();
            let (revision, plan) = documents.shot_plan.as_ref().unwrap();
            assert_eq!(*revision, 1);
            assert_eq!(plan.scenes[0].shots.len(), 2);
            assert!(documents.shot_plan_stale.is_empty());

            let mut scene = plan.scenes[0].clone();
            scene.shots.truncate(1);
            document(
                &session,
                &events,
                DocumentCommand::SaveScenePlan { based_on: 1, scene },
            )
            .unwrap();
            document(
                &session,
                &events,
                DocumentCommand::LockShotPlan { revision: 2 },
            )
            .unwrap();
            let documents = session.documents().unwrap();
            assert_eq!(documents.shot_plan_locked, Some(2));
            assert_eq!(documents.shot_plan_revisions.len(), 2);
            assert_eq!(
                documents.shot_plan.as_ref().unwrap().1.scenes[0]
                    .shots
                    .len(),
                1
            );
        }

        #[test]
        fn a_locked_plan_puts_its_shots_in_play_and_keyframes_follow_them() {
            use kineto_project::shot_plan::SAMPLE_SHOT_PLAN_JSON;
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeImages::default());
            let model = DocumentModel::Fake(FakeLlm::answering([
                SAMPLE_ANALYSIS_JSON,
                SAMPLE_SCREENPLAY_JSON,
                SAMPLE_BIBLE_JSON,
                SAMPLE_SHOT_PLAN_JSON,
            ]));
            let (session, events) = spawn(
                OpenRequest::CreateText {
                    path: target.clone(),
                    manifest: Box::new(manifest()),
                    source: b"A filmmaker hears the last line and leaves.\n".to_vec(),
                },
                &fake,
                model,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            // The bootstrap scene's shots until a plan is locked.
            assert_eq!(session.shot_count(), 2);
            assert_eq!(session.shot_key(1).unwrap(), ShotKey::bootstrap(2));
            for command in [
                DocumentCommand::AnalyzeStory,
                DocumentCommand::WriteScreenplay,
                DocumentCommand::LockScreenplay { revision: 1 },
                DocumentCommand::WriteBible,
                DocumentCommand::LockBible { revision: 1 },
                DocumentCommand::PlanShots,
            ] {
                document(&session, &events, command).unwrap();
            }
            assert_eq!(
                session.shot_key(1).unwrap(),
                ShotKey::bootstrap(2),
                "an unlocked plan changes nothing"
            );
            document(
                &session,
                &events,
                DocumentCommand::LockShotPlan { revision: 1 },
            )
            .unwrap();
            let planned = ShotKey::new("office_night", "office_night_02").unwrap();
            assert_eq!(session.shot_count(), 2);
            assert_eq!(session.shot_key(1).unwrap(), planned);
            let shot: serde_json::Value = serde_json::from_slice(
                &fs::read(target.join("scenes/office_night/shots/office_night_02/shot.json"))
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(shot["characters"], serde_json::json!(["filmmaker"]));
            assert_eq!(shot["continuity_from"], "office_night_01");
            let scene: serde_json::Value = serde_json::from_slice(
                &fs::read(target.join("scenes/office_night/scene.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(
                scene["shots"],
                serde_json::json!(["office_night_01", "office_night_02"])
            );

            let token = token();
            session.submit(1, ShotCommand::Generate, token).unwrap();
            assert!(matches!(
                events.recv_timeout(WAIT).unwrap(),
                Event::Completed {
                    result: Ok(()),
                    shot_index: 1,
                    ..
                }
            ));
            assert!(
                target
                    .join("scenes/office_night/shots/office_night_02/keyframes/office_night_02_g0001_candidate_a.jpg")
                    .is_file()
            );
            assert_eq!(session.snapshot(1).unwrap().candidate_count, 3);
            assert!(matches!(
                session.submit(2, ShotCommand::Generate, token + 1),
                Err(SubmitError::InvalidShot)
            ));

            // A second round, as the director's note asks; the first is kept
            // to compare against.
            let token = self::token();
            session
                .submit(
                    1,
                    ShotCommand::Refine {
                        note: "Tighter on his hands.".to_owned(),
                        consent: Consent::default(),
                    },
                    token,
                )
                .unwrap();
            assert!(matches!(
                events.recv_timeout(WAIT).unwrap(),
                Event::Completed { result: Ok(()), .. }
            ));
            let history = session.candidate_history(1).unwrap();
            assert_eq!(history.len(), 6);
            assert_eq!(
                history
                    .iter()
                    .map(|(record, _)| record.generation)
                    .collect::<Vec<_>>(),
                [1, 1, 1, 2, 2, 2]
            );
            assert_eq!(
                history[0].0.status,
                kineto_project::ArtifactStatus::Superseded
            );
            assert!(history[0].1.as_ref().unwrap().is_file());
            assert!(!history[0].0.applied.contains(&"director.note".to_owned()));
            assert!(history[5].0.applied.contains(&"director.note".to_owned()));
            assert!(
                session
                    .candidate_staleness(1)
                    .unwrap()
                    .iter()
                    .all(Vec::is_empty)
            );

            // The plan changes what the shot feels; its takes say so.
            let documents = session.documents().unwrap();
            let mut scene = documents.shot_plan.as_ref().unwrap().1.scenes[0].clone();
            scene.shots[1].emotion = "Nothing shows yet.".to_owned();
            document(
                &session,
                &events,
                DocumentCommand::SaveScenePlan { based_on: 1, scene },
            )
            .unwrap();
            document(
                &session,
                &events,
                DocumentCommand::LockShotPlan { revision: 2 },
            )
            .unwrap();
            assert!(session.snapshot(1).unwrap().stale);
            assert_eq!(
                session.candidate_staleness(1).unwrap(),
                vec![vec!["intent.emotion".to_owned()]; 3]
            );
            assert!(
                !session.snapshot(0).unwrap().stale,
                "the other shot is untouched"
            );
            drop(session);

            // Reopened, the plan's shots are still the ones in play.
            fs::remove_dir_all(target.join(".kineto")).unwrap();
            let (session, events) = spawn(
                OpenRequest::Open { path: target },
                &fake,
                DocumentModel::None,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(session.shot_key(1).unwrap(), planned);
            assert!(session.snapshot(1).unwrap().generated);
            assert!(!session.snapshot(0).unwrap().generated);
        }

        #[test]
        fn an_unresolved_portrait_waits_for_its_own_character_only_and_survives_reopening() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let fake = Arc::new(FakeImages::default());
            let (session, events) = written(&target, &fake);
            document(
                &session,
                &events,
                DocumentCommand::LockBible { revision: 1 },
            )
            .unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::SampleStyle {
                    preset: "film_noir".to_owned(),
                    note: None,
                },
            )
            .unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::SelectStyle("style_s0001".to_owned()),
            )
            .unwrap();
            produce(
                &session,
                &events,
                ProductionCommand::LockStyle("style_s0001".to_owned()),
            )
            .unwrap();

            fake.time_out.store(1, Ordering::SeqCst);
            assert!(produce(&session, &events, audition("filmmaker", 1)).is_err());
            let unresolved = |session: &ProjectSession| {
                session
                    .production()
                    .unwrap()
                    .characters
                    .iter()
                    .map(|character| character.unresolved)
                    .collect::<Vec<_>>()
            };
            assert_eq!(unresolved(&session), [true, false]);
            assert_eq!(
                produce(&session, &events, audition("filmmaker", 1)),
                Err("Generation(UnresolvedSubmission)".to_owned())
            );
            produce(&session, &events, audition("interviewer", 1)).unwrap();
            assert_eq!(unresolved(&session), [true, false]);
            drop(session);

            let (session, events) = spawn(
                OpenRequest::Open {
                    path: target.clone(),
                },
                &fake,
                DocumentModel::None,
            );
            assert_eq!(events.recv_timeout(WAIT).unwrap(), Event::Opened(Ok(())));
            assert_eq!(unresolved(&session), [true, false]);
            assert!(!session.production().unwrap().style_unresolved);
            let calls = fake.calls.load(Ordering::SeqCst);
            risk(&session, &events, audition("filmmaker", 1)).unwrap();
            assert_eq!(fake.calls.load(Ordering::SeqCst), calls + 1);
            assert_eq!(unresolved(&session), [false, false]);
        }
    }
}
