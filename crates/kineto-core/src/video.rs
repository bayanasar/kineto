//! A shot's video as an asynchronous remote job.
//!
//! Submitting is the only paid step, and it runs as a write-ahead job: the
//! intent is prepared, a receipt with everything the video's record needs is
//! written, the task is submitted, its ID is written into the receipt the
//! moment the provider accepts it, and the intent is marked invoked with the
//! task ID before the command returns. Watching the task is free and
//! runs on its own thread; it downloads the finished file into the shot's
//! `videos/` directory and hands the job back to the session worker, the only
//! code that changes canonical state, to attach:
//!
//! ```text
//! worker:  plan -> prepare -> receipt -> submit (paid) -> receipt(task)
//!          -> intent invoked(task)
//! watcher: query ... query (free) -> download -> media + receipt hash
//! worker:  attach -> acknowledge -> drop receipt
//! ```
//!
//! A restart finds invoked video intents and watches their tasks again; it
//! never submits. A prepared intent whose receipt holds a task was accepted
//! before the intent could say so: the restart marks it invoked and watches
//! that task, and asking again in a live session follows it too. A task
//! the provider reports as failed is retired, so asking again submits afresh.
//!
//! An attempted submission with no task recorded is submitted again only if
//! its receipt shows the provider certainly did not accept it. Otherwise it
//! may have been accepted and charged for (a crash before the task was
//! written down, or a timeout after the request was sent), so it is
//! unresolved: asking again is refused until the director chooses to
//! [`resubmit`], accepting that the video may be paid for twice.
//!
//! Providers are [`VideoGenerator`]s, chosen by what they declare they can
//! do, never by name: MiniMax Hailuo, paid, and the local demo, free, which
//! runs through the same jobs and receipts and returns a clip bundled with
//! the build. Each records its jobs under its own provider key.

use std::{
    convert::Infallible,
    error::Error,
    fmt,
    num::NonZeroU16,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use kineto_job_runtime::{
    DispatchOutcome, Invocation, InvokePreparedError, JobRuntime, JobRuntimeError,
    JobRuntimePolicy, PaidJobAdapter, PrepareOutcome, ProviderKey, Reconciliation, RuntimeIntent,
    StartupOutcome,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, CurrencyCode, ErrorClass, FallbackPolicy, IdempotencyKey,
    IntentState, JobId, PaidFallback, ProviderExecutionPolicy, ProviderJobId,
};
use kineto_project::{
    ArtifactDependency, ArtifactId, ContentHash, DependencyImpact, InputHash,
    fs::{ProjectFsError, ProjectRelativePath, ProjectRoot},
    manifest::CanonicalProject,
    shot::{
        AttachOutcome, DEMO_PROVIDER, PromptProvenance, ShotKey, ShotWorkflow, VideoGenerationSpec,
        VideoOutput, VideoPlan, locate, video_media_path,
    },
};
use kineto_provider_minimax::{
    MinimaxClient, MinimaxError, PROVIDER as MINIMAX_PROVIDER, SharedTransport, Transport,
    VIDEO_MODEL, VideoDuration, VideoRequest, VideoResolution, VideoTask, VideoTaskState, sniff,
    video_capabilities,
};
use kineto_providers::{
    CameraControl, ImageFormat, Price, VideoCapabilities, VideoContainer, VideoMode,
    VideoRequirements, evaluate_video, secret::SecretSource,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    generation::{GenerationError, NoHooks, ProviderSelectionError, parse_job_key, provenance_of},
    video_prompt::{self, VIDEO_COMPILER_VERSION},
};

/// The capability a shot video is generated with.
pub const VIDEO_OPERATION: &str = "video.generate";
/// Provider key MiniMax video jobs are recorded under. Separate from MiniMax
/// image jobs, whose recovery must never see a video's remote task.
pub const VIDEO_PROVIDER: &str = "minimax-video";
/// Provider key the local demo's video jobs are recorded under.
pub const DEMO_VIDEO_PROVIDER: &str = "local-demo-video";
/// The model the local demo names: one bundled clip.
pub const DEMO_VIDEO_MODEL: &str = "demo-clip";
/// The local demo's clip: two seconds of a still, dark frame, 320x180 H.264
/// in MP4, made once with ffmpeg's colour source. Free and the same every
/// time.
pub const DEMO_CLIP: &[u8] = include_bytes!("../assets/demo-clip.mp4");
const DEMO_CLIP_SECONDS: u8 = 2;
const RESOLUTION: VideoResolution = VideoResolution::P768;
const RECEIPT_SCHEMA_VERSION: u32 = 1;
/// How often a watcher asks the provider about its task, and how often it
/// checks that its session is still open while it waits.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);
const WAKE_INTERVAL: Duration = Duration::from_millis(200);

/// Where a remote task is, as a watcher sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Queued or being made.
    Pending,
    /// Finished; `file` is fetched next.
    Succeeded { file: String },
}

/// One video asked of a provider.
#[derive(Clone, PartialEq, Eq)]
pub struct VideoSubmission {
    pub prompt: String,
    /// The locked keyframe's image and its format; `None` for a demo
    /// keyframe, which has none.
    pub first_frame: Option<(Vec<u8>, ImageFormat)>,
    /// One of the clip lengths the provider declares.
    pub clip_seconds: u8,
}

impl fmt::Debug for VideoSubmission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VideoSubmission")
            .field("prompt", &self.prompt)
            .field(
                "first_frame",
                &self
                    .first_frame
                    .as_ref()
                    .map(|(bytes, format)| (bytes.len(), *format)),
            )
            .field("clip_seconds", &self.clip_seconds)
            .finish()
    }
}

/// A video provider: what it can do, and its paid submission, free queries
/// and download.
pub trait VideoGenerator {
    type Error: Error + Send + Sync + 'static;

    /// The key its jobs are recorded under, which the videos it makes name
    /// as their provider.
    fn provider(&self) -> &'static str;
    fn model(&self) -> &'static str;
    fn capabilities(&self) -> VideoCapabilities;
    /// Whether a first frame must be an image: false only for the demo,
    /// which starts from a demo keyframe that has none.
    fn needs_frame_image(&self) -> bool {
        true
    }
    /// The parameters a submission of `clip_seconds` is made with, as they
    /// enter the input hash.
    fn canonical_params(&self, clip_seconds: u8) -> String;
    /// Submit the video: the paid call. Returns the task to follow.
    fn submit(&self, request: &VideoSubmission) -> Result<String, Self::Error>;
    fn query(&self, task: &ProviderJobId) -> Result<TaskState, Self::Error>;
    fn fetch(&self, file: &str) -> Result<Vec<u8>, Self::Error>;
    fn class(&self, error: &Self::Error) -> ErrorClass;
    /// Whether `error` proves the provider did not accept the submission,
    /// so it cannot have been charged.
    fn certainly_not_accepted(&self, error: &Self::Error) -> bool;
}

impl<T: Transport> VideoGenerator for MinimaxClient<T> {
    type Error = MinimaxError;

    fn provider(&self) -> &'static str {
        VIDEO_PROVIDER
    }

    fn model(&self) -> &'static str {
        VIDEO_MODEL
    }

    fn capabilities(&self) -> VideoCapabilities {
        video_capabilities()
    }

    fn canonical_params(&self, clip_seconds: u8) -> String {
        format!("duration={clip_seconds};resolution=768P")
    }

    fn submit(&self, request: &VideoSubmission) -> Result<String, MinimaxError> {
        let Some((frame, format)) = &request.first_frame else {
            return Err(MinimaxError::InvalidRequest("Hailuo needs a first frame"));
        };
        self.submit_video(&VideoRequest {
            prompt: request.prompt.clone(),
            first_frame: frame.clone(),
            first_frame_format: *format,
            duration: VideoDuration::nearest(f64::from(request.clip_seconds)),
            resolution: RESOLUTION,
        })
        .map(|task| task.as_str().to_owned())
    }

    fn query(&self, task: &ProviderJobId) -> Result<TaskState, MinimaxError> {
        let task = VideoTask::new(task.as_str())
            .map_err(|_| MinimaxError::Malformed("task ID changed shape"))?;
        Ok(match self.query_video(&task)? {
            VideoTaskState::Pending => TaskState::Pending,
            VideoTaskState::Succeeded { file_id } => TaskState::Succeeded { file: file_id },
        })
    }

    fn fetch(&self, file: &str) -> Result<Vec<u8>, MinimaxError> {
        self.fetch_video(file)
    }

    fn class(&self, error: &MinimaxError) -> ErrorClass {
        error.class()
    }

    fn certainly_not_accepted(&self, error: &MinimaxError) -> bool {
        error.certainly_not_accepted()
    }
}

/// The local demo: free, offline, and the same for every shot. Its task is
/// done as soon as it is submitted, and its file is [`DEMO_CLIP`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalDemoVideos;

/// What the local demo can do. It follows the same image-to-video contract
/// as a real provider, through jobs that complete at once.
#[must_use]
pub fn demo_video_capabilities() -> VideoCapabilities {
    VideoCapabilities {
        modes: [VideoMode::ImageToVideo].into_iter().collect(),
        min_duration_ms: Some(u64::from(DEMO_CLIP_SECONDS) * 1000),
        max_duration_ms: Some(u64::from(DEMO_CLIP_SECONDS) * 1000),
        fps_options: [12].into_iter().collect(),
        start_frame: true,
        camera_control: CameraControl::None,
        async_jobs: true,
        resumable_remote_handle: true,
        output_containers: [VideoContainer::Mp4].into_iter().collect(),
        clip_seconds: [DEMO_CLIP_SECONDS].into_iter().collect(),
        start_frame_formats: [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp]
            .into_iter()
            .collect(),
        clip_prices: [(DEMO_CLIP_SECONDS, Price::usd(0))].into_iter().collect(),
        ..VideoCapabilities::default()
    }
}

impl VideoGenerator for LocalDemoVideos {
    type Error = Infallible;

    fn provider(&self) -> &'static str {
        DEMO_VIDEO_PROVIDER
    }

    fn model(&self) -> &'static str {
        DEMO_VIDEO_MODEL
    }

    fn capabilities(&self) -> VideoCapabilities {
        demo_video_capabilities()
    }

    fn needs_frame_image(&self) -> bool {
        false
    }

    fn canonical_params(&self, clip_seconds: u8) -> String {
        format!("duration={clip_seconds}")
    }

    fn submit(&self, request: &VideoSubmission) -> Result<String, Infallible> {
        let mut digest = Sha256::new();
        digest.update(request.prompt.as_bytes());
        digest.update([request.clip_seconds]);
        if let Some((frame, _)) = &request.first_frame {
            digest.update(frame);
        }
        let hash = format!("{:x}", digest.finalize());
        Ok(format!("demo-{}", &hash[..16]))
    }

    fn query(&self, task: &ProviderJobId) -> Result<TaskState, Infallible> {
        Ok(TaskState::Succeeded {
            file: task.as_str().to_owned(),
        })
    }

    fn fetch(&self, _file: &str) -> Result<Vec<u8>, Infallible> {
        Ok(DEMO_CLIP.to_vec())
    }

    fn class(&self, error: &Infallible) -> ErrorClass {
        match *error {}
    }

    fn certainly_not_accepted(&self, error: &Infallible) -> bool {
        match *error {}
    }
}

/// Which provider a session makes videos with.
#[derive(Debug, Clone, Default)]
pub enum VideoProvider {
    /// The bundled demo clip. Free.
    #[default]
    LocalDemo,
    /// MiniMax Hailuo. Paid. Without a client, videos an earlier session
    /// submitted are not followed and no new one is submitted.
    Minimax(Option<VideoClient>),
}

/// Which video provider a session generates with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProviderKind {
    LocalDemo,
    Minimax,
}

/// What the director needs to know about the session's video provider
/// before asking for a video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoProviderInfo {
    pub kind: VideoProviderKind,
    /// Whether a video can be asked for: a paid provider needs credentials.
    pub ready: bool,
    /// Estimated cost of one clip of the shortest length, in USD micros.
    pub clip_cost_micros: u64,
    /// That clip's length, in seconds.
    pub clip_seconds: u8,
}

impl VideoProvider {
    /// The provider named by `selection`: `local-demo` for the free demo,
    /// `minimax` for MiniMax with the key `secrets` holds. `None` when
    /// nothing is named, so the session follows its image provider. A paid
    /// provider is used only when it is named.
    pub fn select(
        selection: Option<&str>,
        secrets: &impl SecretSource,
    ) -> Result<Option<Self>, ProviderSelectionError> {
        match selection.map(str::trim) {
            None | Some("") => Ok(None),
            Some(DEMO_PROVIDER) => Ok(Some(Self::LocalDemo)),
            Some(MINIMAX_PROVIDER) => Ok(Some(Self::Minimax(
                secrets
                    .api_key(MINIMAX_PROVIDER)
                    .map_err(ProviderSelectionError::Secret)?
                    .map(MinimaxClient::shared),
            ))),
            Some(other) => Err(ProviderSelectionError::Unknown(other.to_owned())),
        }
    }

    #[must_use]
    pub fn kind(&self) -> VideoProviderKind {
        match self {
            Self::LocalDemo => VideoProviderKind::LocalDemo,
            Self::Minimax(_) => VideoProviderKind::Minimax,
        }
    }

    #[must_use]
    pub fn capabilities(&self) -> VideoCapabilities {
        match self {
            Self::LocalDemo => demo_video_capabilities(),
            Self::Minimax(_) => video_capabilities(),
        }
    }

    #[must_use]
    pub fn info(&self) -> VideoProviderInfo {
        let capabilities = self.capabilities();
        let (clip_seconds, price) = capabilities
            .clip_prices
            .first_key_value()
            .map_or((0, 0), |(seconds, price)| (*seconds, price.amount_micros));
        VideoProviderInfo {
            kind: self.kind(),
            ready: match self {
                Self::LocalDemo => true,
                Self::Minimax(client) => client.is_some(),
            },
            clip_cost_micros: price,
            clip_seconds,
        }
    }
}

/// A provider's failure, kept with how it is classified once the
/// provider's own error type is no longer at hand.
#[derive(Debug)]
pub struct VideoFailure {
    pub class: ErrorClass,
    pub source: Box<dyn Error + Send + Sync>,
}

impl fmt::Display for VideoFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.source.fmt(formatter)
    }
}

impl Error for VideoFailure {}

fn failure<G: VideoGenerator>(generator: &G, error: G::Error) -> VideoFailure {
    VideoFailure {
        class: generator.class(&error),
        source: Box::new(error),
    }
}

/// Where a submission can be stopped by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoPoint {
    /// MiniMax accepted the task; nothing about it is written down yet.
    Accepted,
    /// The task is in the receipt; the intent does not say so yet.
    Recorded,
}

pub trait VideoHooks {
    fn reached(&mut self, point: VideoPoint);
}

impl VideoHooks for NoHooks {
    fn reached(&mut self, _point: VideoPoint) {}
}

/// A submitted video, as a watcher follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoJob {
    pub shot: ShotKey,
    pub revision: u16,
    pub artifact_id: ArtifactId,
    /// The provider key the job is recorded under, which says which
    /// provider follows its task.
    pub provider: &'static str,
    pub task: ProviderJobId,
}

/// What a watcher found.
#[derive(Debug)]
pub struct VideoDone {
    pub job: VideoJob,
    /// The video is on disk and its receipt names its bytes, or the
    /// provider reported the task failed.
    pub result: Result<(), VideoFailure>,
}

#[derive(Debug)]
pub enum VideoError {
    Receipt(&'static str),
    Fs(ProjectFsError),
}

impl fmt::Display for VideoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Receipt(what) => write!(formatter, "video receipt: {what}"),
            Self::Fs(error) => error.fmt(formatter),
        }
    }
}

impl Error for VideoError {}

#[derive(Debug, Serialize, Deserialize)]
struct Receipt {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
    model: String,
    keyframe_artifact_id: String,
    keyframe_content_hash: String,
    provenance: PromptProvenance,
    /// The provider's task, written as soon as the submission is accepted.
    /// From then on the video is paid for and is never submitted again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    /// The attempt the provider certainly did not accept: it was never sent
    /// or was refused. Any other attempt without a task may have been
    /// charged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused_attempt: Option<u16>,
    /// Set by the watcher once the video is on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    content_hash: Option<String>,
}

fn receipt_path(artifact_id: &ArtifactId) -> Result<ProjectRelativePath, VideoError> {
    ProjectRelativePath::new(format!(".kineto/generation/{}.json", artifact_id.as_str()))
        .map_err(|_| VideoError::Receipt("artifact ID is not a valid file name"))
}

fn write_receipt(
    root: &ProjectRoot,
    artifact_id: &ArtifactId,
    receipt: &Receipt,
) -> Result<(), VideoError> {
    let mut bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|_| VideoError::Receipt("receipt did not encode"))?;
    bytes.push(b'\n');
    root.write_atomic(&receipt_path(artifact_id)?, &bytes)
        .map_err(VideoError::Fs)
}

fn read_receipt(root: &ProjectRoot, artifact_id: &ArtifactId) -> Result<Receipt, VideoError> {
    let bytes = root
        .read(&receipt_path(artifact_id)?)
        .map_err(VideoError::Fs)?;
    let receipt: Receipt = serde_json::from_slice(&bytes)
        .map_err(|_| VideoError::Receipt("receipt is not readable"))?;
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION
        || receipt.artifact_id != artifact_id.as_str()
    {
        return Err(VideoError::Receipt("receipt belongs to another video"));
    }
    Ok(receipt)
}

/// The task the provider accepted for this video, if its receipt recorded
/// one.
fn accepted_task(
    root: &ProjectRoot,
    artifact_id: &ArtifactId,
    input_hash: &InputHash,
) -> Option<String> {
    read_receipt(root, artifact_id)
        .ok()
        .filter(|receipt| receipt.input_hash == input_hash.as_str())
        .and_then(|receipt| receipt.task_id)
}

fn record_task(root: &ProjectRoot, artifact_id: &ArtifactId, task: &str) -> Result<(), VideoError> {
    let mut receipt = read_receipt(root, artifact_id)?;
    receipt.task_id = Some(task.to_owned());
    write_receipt(root, artifact_id, &receipt)
}

fn record_refusal(
    root: &ProjectRoot,
    artifact_id: &ArtifactId,
    attempt: u16,
) -> Result<(), VideoError> {
    let mut receipt = read_receipt(root, artifact_id)?;
    receipt.refused_attempt = Some(attempt);
    write_receipt(root, artifact_id, &receipt)
}

/// How the last submission of a prepared video ended, as far as the project
/// can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastAttempt {
    /// Nothing was submitted.
    None,
    /// The provider accepted it; its task is in the receipt.
    Accepted,
    /// The provider certainly did not accept it.
    Refused,
    /// The provider may have accepted it.
    Unresolved,
}

fn last_attempt(root: &ProjectRoot, record: &RuntimeIntent) -> LastAttempt {
    if record.attempts() == 0 {
        return LastAttempt::None;
    }
    let intent = record.intent();
    let receipt = intent
        .artifact_id()
        .and_then(|artifact_id| read_receipt(root, artifact_id).ok())
        .filter(|receipt| receipt.input_hash == intent.input_hash().as_str());
    match receipt {
        Some(receipt) if receipt.task_id.is_some() => LastAttempt::Accepted,
        Some(receipt) if receipt.refused_attempt == Some(record.attempts()) => LastAttempt::Refused,
        _ => LastAttempt::Unresolved,
    }
}

fn task_handle(task: &str) -> Result<ProviderJobId, VideoFailure> {
    ProviderJobId::new(task).map_err(|_| VideoFailure {
        class: ErrorClass::Terminal,
        source: Box::new(VideoError::Receipt("task ID is not a job handle")),
    })
}

fn remove_receipt(root: &ProjectRoot, artifact_id: &ArtifactId) {
    // Best effort: a receipt left behind is never read again once its job
    // is acknowledged or retired.
    if let Ok(path) = receipt_path(artifact_id) {
        let _ = std::fs::remove_file(root.resolve(&path));
    }
}

fn provider_key(provider: &'static str) -> ProviderKey {
    ProviderKey::new(provider).expect("static provider key is valid")
}

fn policy() -> JobRuntimePolicy {
    JobRuntimePolicy {
        execution: ProviderExecutionPolicy::new(
            NonZeroU16::new(2).expect("static concurrency is non-zero"),
            BackoffPolicy::new(2_000, 30_000, 3).expect("static backoff policy is valid"),
        ),
        // A video is an explicit, chosen spend; nothing falls back to it.
        fallback_policy: FallbackPolicy {
            paid_fallback: PaidFallback::Forbidden,
            max_paid_amount_micros: Some(0),
            require_user_approval: true,
        },
    }
}

fn runtime<'a>(project: &'a CanonicalProject, provider: &'static str) -> JobRuntime<'a> {
    JobRuntime::new(project, provider_key(provider), policy())
}

/// Submits a planned video and records its task.
struct Submit<'a, G, H> {
    generator: &'a G,
    root: &'a ProjectRoot,
    artifact_id: &'a ArtifactId,
    input_hash: &'a InputHash,
    /// The attempt this submission is, as the intent will count it.
    attempt: u16,
    hooks: &'a mut H,
}

/// A submission's failure: the provider's own, or a task handle it
/// returned that cannot be kept.
#[derive(Debug)]
enum SubmitError<E> {
    Provider(E),
    Handle(VideoFailure),
}

impl<E: fmt::Display> fmt::Display for SubmitError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => error.fmt(formatter),
            Self::Handle(error) => error.fmt(formatter),
        }
    }
}

impl<E: Error> Error for SubmitError<E> {}

impl<G: VideoGenerator, H: VideoHooks> PaidJobAdapter for Submit<'_, G, H> {
    type Request = VideoSubmission;
    type Output = ();
    type Error = SubmitError<G::Error>;

    fn estimate_cost(&self, request: &VideoSubmission) -> Result<CostEstimate, Self::Error> {
        let price = self
            .generator
            .capabilities()
            .clip_prices
            .get(&request.clip_seconds)
            .copied()
            .ok_or(SubmitError::Handle(VideoFailure {
                class: ErrorClass::Terminal,
                source: Box::new(VideoError::Receipt("the clip length has no price")),
            }))?;
        Ok(CostEstimate {
            call_count: 1,
            amount_micros: price.amount_micros,
            currency: CurrencyCode::new(price.currency).expect("declared currency is valid"),
            estimated_duration_ms: None,
        })
    }

    fn invoke(
        &mut self,
        request: &VideoSubmission,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<()>, Self::Error> {
        // Accepted earlier, but the intent was never marked invoked: follow
        // that task instead of paying for another.
        if let Some(task) = accepted_task(self.root, self.artifact_id, self.input_hash) {
            return task_handle(&task)
                .map(Invocation::Pending)
                .map_err(SubmitError::Handle);
        }
        let task = match self.generator.submit(request) {
            Ok(task) => task,
            Err(error) => {
                // Only a proven refusal lets this video be submitted again
                // without the director's say; if it cannot be written down
                // the attempt stays unresolved, which fails closed.
                if self.generator.certainly_not_accepted(&error) {
                    let _ = record_refusal(self.root, self.artifact_id, self.attempt);
                }
                return Err(SubmitError::Provider(error));
            }
        };
        // The provider may have charged from here on. The task goes into the
        // receipt before anything else can fail. If that write fails the
        // runtime's intent save is the remaining record, so the task is
        // still returned for it to keep.
        self.hooks.reached(VideoPoint::Accepted);
        let _ = record_task(self.root, self.artifact_id, &task);
        self.hooks.reached(VideoPoint::Recorded);
        task_handle(&task)
            .map(Invocation::Pending)
            .map_err(SubmitError::Handle)
    }

    fn reconcile(&mut self, _id: &ProviderJobId) -> Result<Reconciliation<()>, Self::Error> {
        unreachable!("submission never reconciles; watchers follow tasks")
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        _key: &IdempotencyKey,
    ) -> Result<Reconciliation<()>, Self::Error> {
        unreachable!("submission never reconciles; watchers follow tasks")
    }

    fn classify_error(&self, error: &Self::Error) -> ErrorClass {
        match error {
            SubmitError::Provider(error) => self.generator.class(error),
            SubmitError::Handle(error) => error.class,
        }
    }
}

/// Startup reconciliation that leaves every remote task to a watcher, so
/// opening a project never waits on the network.
struct Deferred<'a> {
    root: &'a ProjectRoot,
}

impl PaidJobAdapter for Deferred<'_> {
    type Request = VideoSubmission;
    type Output = ();
    type Error = VideoFailure;

    fn estimate_cost(&self, _request: &VideoSubmission) -> Result<CostEstimate, VideoFailure> {
        Err(deferred("recovery estimates nothing"))
    }

    fn invoke(
        &mut self,
        _request: &VideoSubmission,
        _key: &IdempotencyKey,
    ) -> Result<Invocation<()>, VideoFailure> {
        Err(deferred("recovery submits nothing"))
    }

    fn reconcile(&mut self, id: &ProviderJobId) -> Result<Reconciliation<()>, VideoFailure> {
        Ok(Reconciliation::Pending {
            provider_job_id: Some(id.clone()),
        })
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        key: &IdempotencyKey,
    ) -> Result<Reconciliation<()>, VideoFailure> {
        // No provider keeps a record a submission could be found by, so the
        // receipt is the only place an accepted task can be.
        let Ok((artifact_id, input_hash)) = parse_job_key(VIDEO_OPERATION, key) else {
            return Ok(Reconciliation::NotFound);
        };
        match accepted_task(self.root, &artifact_id, &input_hash) {
            Some(task) => Ok(Reconciliation::Pending {
                provider_job_id: Some(task_handle(&task)?),
            }),
            None => Ok(Reconciliation::NotFound),
        }
    }

    fn classify_error(&self, error: &VideoFailure) -> ErrorClass {
        error.class
    }
}

fn deferred(what: &'static str) -> VideoFailure {
    VideoFailure {
        class: ErrorClass::Terminal,
        source: Box::new(VideoError::Receipt(what)),
    }
}

fn job_from_intent(
    project: &CanonicalProject,
    runtime: &JobRuntime<'_>,
    provider: &'static str,
    job_id: &JobId,
) -> Result<Option<VideoJob>, JobRuntimeError> {
    let Some(record) = runtime.load(job_id)? else {
        return Ok(None);
    };
    let intent = record.intent();
    let (Some(artifact_id), Some(task)) = (intent.artifact_id(), intent.provider_job_id()) else {
        return Ok(None);
    };
    let Ok(plan) = VideoPlan::for_artifact(artifact_id, intent.input_hash().clone()) else {
        return Ok(None);
    };
    let Ok(Some(shot)) = locate(project, plan.shot_id()) else {
        return Ok(None);
    };
    Ok(Some(VideoJob {
        shot,
        revision: plan.revision(),
        artifact_id: artifact_id.clone(),
        provider,
        task: task.clone(),
    }))
}

/// Submit the shot's next video. This is the paid call; it returns as soon as
/// the provider has accepted the task.
pub fn start<G: VideoGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
) -> Result<VideoJob, GenerationError> {
    start_with(project, shot, generator, &mut NoHooks)
}

/// [`start`], stopping at `hooks`' points.
pub fn start_with<G: VideoGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
    hooks: &mut impl VideoHooks,
) -> Result<VideoJob, GenerationError> {
    submit(project, shot, generator, hooks, false)
}

/// Submit the shot's video again although its last submission is
/// unresolved: the provider may already have accepted, and charged for,
/// that one. Only on the director's explicit decision to risk paying twice.
pub fn resubmit<G: VideoGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
) -> Result<VideoJob, GenerationError> {
    submit(project, shot, generator, &mut NoHooks, true)
}

/// What a shot's video needs of a provider: a clip made from the locked
/// keyframe. A provider that would make it from words alone, or cannot take
/// that keyframe, is refused rather than quietly used.
fn check_capabilities(
    capabilities: &VideoCapabilities,
    frame: Option<&(Vec<u8>, ImageFormat)>,
) -> Result<(), GenerationError> {
    let report = evaluate_video(
        VideoRequirements {
            mode: VideoMode::ImageToVideo,
            requires_start_frame: true,
            requires_end_frame: false,
            requires_identity_reference: false,
            preferred_camera_control: CameraControl::None,
        },
        capabilities,
    );
    let mut missing: Vec<&'static str> = report
        .gaps
        .iter()
        .filter(|gap| gap.severity == kineto_providers::GapSeverity::Incompatible)
        .map(|gap| gap.capability)
        .collect();
    if let Some((bytes, format)) = frame {
        if !capabilities.start_frame_formats.contains(format) {
            missing.push("video.start_frame_format");
        }
        if capabilities
            .max_start_frame_bytes
            .is_some_and(|max| bytes.len() as u64 > max)
        {
            missing.push("video.start_frame_bytes");
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(GenerationError::Incapable(missing))
    }
}

/// The clip lengths `capabilities` render, shortest first.
fn clip_lengths(capabilities: &VideoCapabilities) -> Vec<u8> {
    if !capabilities.clip_seconds.is_empty() {
        return capabilities.clip_seconds.iter().copied().collect();
    }
    let seconds = |ms: Option<u64>| ms.and_then(|ms| u8::try_from(ms.div_ceil(1000)).ok());
    match (
        seconds(capabilities.min_duration_ms),
        seconds(capabilities.max_duration_ms),
    ) {
        (Some(min), Some(max)) => (min.max(1)..=max).collect(),
        _ => Vec::new(),
    }
}

fn submit<G: VideoGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
    hooks: &mut impl VideoHooks,
    risk_paying_twice: bool,
) -> Result<VideoJob, GenerationError> {
    let (keyframe, first_frame) = shot.first_frame()?.ok_or(GenerationError::Shot(
        kineto_project::shot::ShotWorkflowError::KeyframeNotLocked,
    ))?;
    let frame = match first_frame {
        Some(path) => {
            let bytes = project
                .root()
                .read(&path)
                .map_err(|error| video_error(VideoError::Fs(error)))?;
            if ContentHash::of(&bytes) != keyframe.content_hash {
                return Err(GenerationError::Shot(
                    kineto_project::shot::ShotWorkflowError::MediaMismatch(
                        path.as_path().to_string_lossy().into_owned(),
                    ),
                ));
            }
            let format = sniff(&bytes).ok_or(GenerationError::NoFirstFrame)?;
            Some((bytes, format))
        }
        None if generator.needs_frame_image() => return Err(GenerationError::NoFirstFrame),
        None => None,
    };
    let capabilities = generator.capabilities();
    check_capabilities(&capabilities, frame.as_ref())?;
    let prompt = video_prompt::compile(
        shot.intent()?.as_ref(),
        shot.direction(),
        keyframe.content_hash.as_str(),
        &clip_lengths(&capabilities),
        capabilities.max_prompt_chars.unwrap_or(usize::MAX),
    )?;
    let params = generator.canonical_params(prompt.duration_seconds);
    let plan = shot.plan_video(
        project,
        VideoGenerationSpec {
            provider: generator.provider(),
            model: generator.model(),
            canonical_params: params.as_bytes(),
            compiler_version: VIDEO_COMPILER_VERSION,
        },
    )?;

    let provider = generator.provider();
    let runtime = runtime(project, provider);
    let job_id = JobId::new(plan.artifact_id().as_str())?;
    if let Some(record) = runtime.load(&job_id)?
        && record.intent().state() == IntentState::Prepared
    {
        match last_attempt(project.root(), &record) {
            LastAttempt::None | LastAttempt::Accepted => {}
            // Asking again is what retries a submission MiniMax refused.
            LastAttempt::Refused => {
                if runtime.retire_failed(&job_id)? {
                    remove_receipt(project.root(), plan.artifact_id());
                }
            }
            LastAttempt::Unresolved if risk_paying_twice => {
                runtime.retire_unresolved(&job_id)?;
                remove_receipt(project.root(), plan.artifact_id());
            }
            LastAttempt::Unresolved => return Err(GenerationError::UnresolvedSubmission),
        }
    }
    let record = match runtime.prepare(
        job_id.clone(),
        VIDEO_OPERATION,
        plan.input_hash().clone(),
        plan.artifact_id().clone(),
    )? {
        PrepareOutcome::Created(record) | PrepareOutcome::Existing(record) => record,
    };
    match record.intent().state() {
        IntentState::Prepared => {}
        // Already submitted: follow the task it has.
        IntentState::Invoked => {
            return job_from_intent(project, &runtime, provider, &job_id)?
                .ok_or(GenerationError::Unexpected("a submitted video has no task"));
        }
        IntentState::Reconciled => {
            return Err(GenerationError::Unexpected(
                "this video was already attached",
            ));
        }
    }

    // A receipt holding an accepted task is kept: the submission below then
    // follows that task instead of paying again.
    if accepted_task(project.root(), plan.artifact_id(), plan.input_hash()).is_none() {
        write_receipt(
            project.root(),
            plan.artifact_id(),
            &Receipt {
                schema_version: RECEIPT_SCHEMA_VERSION,
                artifact_id: plan.artifact_id().as_str().to_owned(),
                input_hash: plan.input_hash().as_str().to_owned(),
                model: generator.model().to_owned(),
                keyframe_artifact_id: keyframe.artifact_id.as_str().to_owned(),
                keyframe_content_hash: keyframe.content_hash.as_str().to_owned(),
                provenance: provenance_of(&prompt.compiled, &prompt.prompt_hash),
                task_id: None,
                refused_attempt: None,
                content_hash: None,
            },
        )
        .map_err(video_error)?;
    }

    let request = VideoSubmission {
        prompt: prompt.compiled.prompt().to_owned(),
        first_frame: frame,
        clip_seconds: prompt.duration_seconds,
    };
    let mut submit = Submit {
        generator,
        root: project.root(),
        artifact_id: plan.artifact_id(),
        input_hash: plan.input_hash(),
        attempt: record.attempts().saturating_add(1),
        hooks,
    };
    match runtime.invoke_prepared(&job_id, &mut submit, &request) {
        Ok(DispatchOutcome::Pending {
            provider_job_id, ..
        }) => Ok(VideoJob {
            shot: shot.key().clone(),
            revision: plan.revision(),
            artifact_id: plan.artifact_id().clone(),
            provider,
            task: provider_job_id,
        }),
        Ok(DispatchOutcome::Completed { .. }) => Err(GenerationError::Unexpected(
            "a video submission completed synchronously",
        )),
        Err(InvokePreparedError::Runtime(error)) => Err(error.into()),
        // A failure that may have reached the provider may have been
        // charged.
        Err(InvokePreparedError::Provider {
            source: SubmitError::Provider(source),
            ..
        }) if !generator.certainly_not_accepted(&source) => {
            Err(GenerationError::UnresolvedSubmission)
        }
        Err(InvokePreparedError::Provider { source, class, .. }) => {
            Err(GenerationError::Provider {
                source: Box::new(source),
                class,
            })
        }
    }
}

fn video_error(error: VideoError) -> GenerationError {
    let class = match error {
        VideoError::Fs(_) => ErrorClass::Retryable,
        VideoError::Receipt(_) => ErrorClass::Terminal,
    };
    GenerationError::Provider {
        source: Box::new(error),
        class,
    }
}

/// Follow a submitted task until it finishes, then download the video.
/// Free. Returns `None` if `closed` is set first.
pub fn watch<G: VideoGenerator>(
    generator: &G,
    root: &ProjectRoot,
    job: VideoJob,
    closed: &AtomicBool,
    interval: Duration,
) -> Option<VideoDone> {
    let mut wait = Duration::ZERO;
    loop {
        if !sleep_unless_closed(closed, wait) {
            return None;
        }
        wait = interval;
        let state = match generator.query(&job.task) {
            Ok(state) => state,
            // Asking again is free; wait and ask again.
            Err(error) if generator.class(&error) == ErrorClass::Retryable => continue,
            Err(error) => {
                return Some(VideoDone {
                    job,
                    result: Err(failure(generator, error)),
                });
            }
        };
        let TaskState::Succeeded { file } = state else {
            continue;
        };
        match fetch(generator, root, &job, &file) {
            Ok(()) => {
                return Some(VideoDone {
                    job,
                    result: Ok(()),
                });
            }
            Err(error) if error.class == ErrorClass::Retryable => {}
            Err(error) => {
                return Some(VideoDone {
                    job,
                    result: Err(error),
                });
            }
        }
    }
}

fn sleep_unless_closed(closed: &AtomicBool, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    loop {
        if closed.load(Ordering::SeqCst) {
            return false;
        }
        let now = Instant::now();
        if now >= deadline {
            return true;
        }
        thread::sleep(WAKE_INTERVAL.min(deadline - now));
    }
}

fn fetch<G: VideoGenerator>(
    generator: &G,
    root: &ProjectRoot,
    job: &VideoJob,
    file: &str,
) -> Result<(), VideoFailure> {
    let bytes = generator
        .fetch(file)
        .map_err(|error| failure(generator, error))?;
    let local = |class, error: VideoError| VideoFailure {
        class,
        source: Box::new(error),
    };
    let media = video_media_path(&job.shot, &job.artifact_id).map_err(|_| {
        local(
            ErrorClass::Terminal,
            VideoError::Receipt("video path is invalid"),
        )
    })?;
    // Writing can fail for a while (a full disk); asking again is free.
    root.write_atomic(&media, &bytes)
        .map_err(|error| local(ErrorClass::Retryable, VideoError::Fs(error)))?;
    let mut receipt =
        read_receipt(root, &job.artifact_id).map_err(|error| local(ErrorClass::Terminal, error))?;
    receipt.content_hash = Some(ContentHash::of(&bytes).as_str().to_owned());
    write_receipt(root, &job.artifact_id, &receipt)
        .map_err(|error| local(ErrorClass::Retryable, error))
}

/// Attach a finished video, or retire a failed one so it can be asked for
/// again. Session worker only.
pub fn finish(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    done: VideoDone,
) -> Result<AttachOutcome, GenerationError> {
    let runtime = runtime(project, done.job.provider);
    let job_id = JobId::new(done.job.artifact_id.as_str())?;
    if let Err(failure) = done.result {
        runtime.retire_remote_failure(&job_id)?;
        remove_receipt(project.root(), &done.job.artifact_id);
        return Err(GenerationError::Provider {
            class: failure.class,
            source: failure.source,
        });
    }
    let receipt = read_receipt(project.root(), &done.job.artifact_id).map_err(video_error)?;
    let content_hash = receipt
        .content_hash
        .as_deref()
        .map(ContentHash::new)
        .transpose()
        .ok()
        .flatten()
        .ok_or_else(|| video_error(VideoError::Receipt("the video was not recorded")))?;
    let input_hash = InputHash::new(receipt.input_hash)
        .map_err(|_| video_error(VideoError::Receipt("input hash is invalid")))?;
    let plan = VideoPlan::for_artifact(&done.job.artifact_id, input_hash)?;
    let keyframe = ArtifactDependency {
        artifact_id: ArtifactId::new(receipt.keyframe_artifact_id)
            .map_err(|_| video_error(VideoError::Receipt("keyframe ID is invalid")))?,
        content_hash: ContentHash::new(receipt.keyframe_content_hash)
            .map_err(|_| video_error(VideoError::Receipt("keyframe hash is invalid")))?,
        impact: DependencyImpact::Semantic,
        field: Some("first_frame".to_owned()),
    };
    let outcome = shot.attach_video(
        project,
        &plan,
        &VideoOutput {
            artifact_id: done.job.artifact_id.clone(),
            content_hash,
            provider: done.job.provider.to_owned(),
            model: receipt.model,
            media_path: video_media_path(shot.key(), &done.job.artifact_id)?,
            provenance: Some(receipt.provenance),
            keyframe,
        },
    )?;
    let _ = runtime.acknowledge_result(&job_id)?;
    remove_receipt(project.root(), &done.job.artifact_id);
    Ok(outcome)
}

/// Video jobs submitted by an earlier session and not yet attached. Their
/// tasks are watched again; nothing is submitted.
#[derive(Debug, Default)]
pub struct InFlight {
    pub jobs: Vec<VideoJob>,
    /// Shots whose last submission may have been accepted but left no task;
    /// see [`resubmit`].
    pub unresolved: Vec<ShotKey>,
    /// Records this build cannot read or place.
    pub needs_attention: usize,
}

/// The video jobs recorded under `provider` that an earlier session left.
pub fn in_flight(
    project: &CanonicalProject,
    provider: &'static str,
) -> Result<InFlight, JobRuntimeError> {
    let runtime = runtime(project, provider);
    let mut found = InFlight::default();
    let mut deferred = Deferred {
        root: project.root(),
    };
    for outcome in runtime.reconcile_startup(&mut deferred)? {
        match outcome {
            StartupOutcome::StillPending { job_id } => {
                match job_from_intent(project, &runtime, provider, &job_id)? {
                    Some(job) => found.jobs.push(job),
                    None => found.needs_attention += 1,
                }
            }
            // Never submitted, or the provider refused it: the director asks again
            // when they want it. Any other attempt may have been charged.
            StartupOutcome::RetryPrepared { job_id, .. } => {
                if let Some(record) = runtime.load(&job_id)?
                    && last_attempt(project.root(), &record) == LastAttempt::Unresolved
                {
                    match record
                        .intent()
                        .artifact_id()
                        .and_then(|artifact_id| {
                            VideoPlan::for_artifact(
                                artifact_id,
                                record.intent().input_hash().clone(),
                            )
                            .ok()
                        })
                        .and_then(|plan| locate(project, plan.shot_id()).ok().flatten())
                    {
                        Some(shot) => found.unresolved.push(shot),
                        None => found.needs_attention += 1,
                    }
                }
            }
            StartupOutcome::ResultReady { .. }
            | StartupOutcome::NeedsAttention { .. }
            | StartupOutcome::UnreadableIntent { .. }
            | StartupOutcome::ProviderError { .. } => found.needs_attention += 1,
        }
    }
    Ok(found)
}

/// The client type the session uses.
pub type VideoClient = MinimaxClient<SharedTransport>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_frame_the_provider_cannot_take_is_refused_by_capability() {
        let minimax = video_capabilities();
        let png = (vec![0x89; 64], ImageFormat::Png);
        assert!(check_capabilities(&minimax, Some(&png)).is_ok());
        assert!(check_capabilities(&demo_video_capabilities(), None).is_ok());

        let narrow = VideoCapabilities {
            start_frame_formats: [ImageFormat::Jpeg].into_iter().collect(),
            max_start_frame_bytes: Some(32),
            ..minimax
        };
        let Err(GenerationError::Incapable(missing)) = check_capabilities(&narrow, Some(&png))
        else {
            panic!("the frame must be refused");
        };
        assert_eq!(
            missing,
            ["video.start_frame_format", "video.start_frame_bytes"]
        );
    }

    #[test]
    fn clip_lengths_come_from_the_declared_lengths_or_range() {
        assert_eq!(clip_lengths(&video_capabilities()), [6, 10]);
        assert_eq!(clip_lengths(&demo_video_capabilities()), [2]);
        let ranged = VideoCapabilities {
            min_duration_ms: Some(2_500),
            max_duration_ms: Some(5_000),
            ..VideoCapabilities::default()
        };
        assert_eq!(clip_lengths(&ranged), [3, 4, 5]);
        assert!(clip_lengths(&VideoCapabilities::default()).is_empty());
    }

    #[test]
    fn a_paid_video_provider_is_used_only_when_named() {
        use kineto_providers::secret::EnvSecretSource;
        let with_key = EnvSecretSource::with_lookup(|_| Some("sk-test".into()));
        let without_key = EnvSecretSource::with_lookup(|_| None);
        for unnamed in [None, Some(""), Some(" ")] {
            assert!(VideoProvider::select(unnamed, &with_key).unwrap().is_none());
        }
        assert!(matches!(
            VideoProvider::select(Some("local-demo"), &with_key).unwrap(),
            Some(VideoProvider::LocalDemo)
        ));
        assert!(matches!(
            VideoProvider::select(Some("minimax"), &with_key).unwrap(),
            Some(VideoProvider::Minimax(Some(_)))
        ));
        assert!(matches!(
            VideoProvider::select(Some("minimax"), &without_key).unwrap(),
            Some(VideoProvider::Minimax(None))
        ));
        assert!(VideoProvider::select(Some("sora"), &with_key).is_err());
    }

    #[test]
    fn the_demo_clip_is_an_mp4_and_free() {
        assert_eq!(&DEMO_CLIP[4..8], b"ftyp");
        let info = VideoProvider::LocalDemo.info();
        assert!(info.ready);
        assert_eq!((info.clip_cost_micros, info.clip_seconds), (0, 2));
        let minimax = VideoProvider::Minimax(None).info();
        assert!(!minimax.ready);
        assert_eq!(
            (minimax.clip_cost_micros, minimax.clip_seconds),
            (280_000, 6)
        );
    }
}
