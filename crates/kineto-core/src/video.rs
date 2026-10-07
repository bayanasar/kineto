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
//! do, never by name: MiniMax Hailuo and Google Veo, paid, and the local
//! demo, free, which runs through the same jobs and receipts and returns a
//! clip bundled with the build. Each records its jobs under its own
//! provider key.

use std::{
    convert::Infallible,
    error::Error,
    fmt,
    num::NonZeroU16,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
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
        AttachOutcome, DEMO_PROVIDER, END_FRAME_FIELD, EndFrame, MAX_VIDEO_CANDIDATES,
        PromptProvenance, ShotIntent, ShotKey, ShotWorkflow, VideoGenerationSpec, VideoOutput,
        VideoPlan, locate, video_media_path,
    },
    shot_plan::shots_in_play,
};
use kineto_provider_minimax::{
    MinimaxClient, MinimaxError, PROVIDER as MINIMAX_PROVIDER, SharedTransport, Transport,
    VIDEO_MODEL, VideoDuration, VideoRequest, VideoResolution, VideoTask, VideoTaskState,
    camera_command, sniff, video_capabilities,
};
use kineto_provider_veo::{
    OperationState, PROVIDER as VEO_PROVIDER, VeoClient, VeoError, VeoModel, VeoRequest,
};
use kineto_providers::{
    CameraControl, ImageFormat, Price, VideoCapabilities, VideoClip, VideoContainer, VideoMode,
    VideoRequirements, evaluate_video, secret::SecretSource,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    generation::{
        GenerationError, NoHooks, ProviderSelectionError, invoke_when_permitted, parse_job_key,
        provenance_of, selected_plugin,
    },
    plugin::{self, PluginProvider},
    video_prompt::{self, EndFrameUse, Successor, VIDEO_COMPILER_VERSION, VideoPromptInput},
};

/// The capability a shot video is generated with.
pub const VIDEO_OPERATION: &str = "video.generate";
/// Provider key MiniMax video jobs are recorded under. Separate from MiniMax
/// image jobs, whose recovery must never see a video's remote task.
pub const VIDEO_PROVIDER: &str = "minimax-video";
/// Provider key Veo video jobs are recorded under, whichever Veo model
/// made them.
pub const VEO_VIDEO_PROVIDER: &str = "veo-video";
/// Provider key the local demo's video jobs are recorded under.
pub const DEMO_VIDEO_PROVIDER: &str = "local-demo-video";
/// The model the local demo names: one bundled clip.
pub const DEMO_VIDEO_MODEL: &str = "demo-clip";
/// The local demo's clip: two seconds of a still, dark frame, 320x180 H.264
/// in MP4, made once with ffmpeg's colour source. Free and the same every
/// time.
pub const DEMO_CLIP: &[u8] = include_bytes!("../assets/demo-clip.mp4");
const DEMO_CLIP_SECONDS: u8 = 2;
/// The demo clip's height, in lines.
const DEMO_CLIP_LINES: u16 = 180;
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
    /// The clip's height in lines, one the provider declares; `None` for a
    /// provider that declares none and chooses itself.
    pub lines: Option<u16>,
    /// The locked keyframe of the shot that continues from this one, for
    /// the clip to close on, and its format.
    pub last_frame: Option<(Vec<u8>, ImageFormat)>,
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
            .field("lines", &self.lines)
            .field(
                "last_frame",
                &self
                    .last_frame
                    .as_ref()
                    .map(|(bytes, format)| (bytes.len(), *format)),
            )
            .finish()
    }
}

/// A video provider: what it can do, and its paid submission, free queries
/// and download.
pub trait VideoGenerator {
    type Error: Error + Send + Sync + 'static;

    /// The key its jobs are recorded under, which the videos it makes name
    /// as their provider.
    fn provider(&self) -> &str;
    fn model(&self) -> &str;
    fn capabilities(&self) -> VideoCapabilities;
    /// Whether a first frame must be an image: false only for the demo,
    /// which starts from a demo keyframe that has none.
    fn needs_frame_image(&self) -> bool {
        true
    }
    /// Its own command for a planned camera motion, where its camera
    /// control is parametric and it has one.
    fn camera_command(&self, _motion: &str) -> Option<&'static str> {
        None
    }
    /// The parameters a submission of `clip_seconds` at `lines` is made
    /// with, as they enter the input hash.
    fn canonical_params(&self, clip_seconds: u8, lines: Option<u16>) -> String;
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

    fn provider(&self) -> &str {
        VIDEO_PROVIDER
    }

    fn model(&self) -> &str {
        VIDEO_MODEL
    }

    fn capabilities(&self) -> VideoCapabilities {
        video_capabilities()
    }

    fn camera_command(&self, motion: &str) -> Option<&'static str> {
        camera_command(motion)
    }

    fn canonical_params(&self, clip_seconds: u8, lines: Option<u16>) -> String {
        format!(
            "duration={clip_seconds};resolution={}P",
            lines.unwrap_or(VideoResolution::P768.lines())
        )
    }

    fn submit(&self, request: &VideoSubmission) -> Result<String, MinimaxError> {
        let Some((frame, format)) = &request.first_frame else {
            return Err(MinimaxError::InvalidRequest("Hailuo needs a first frame"));
        };
        let resolution =
            VideoResolution::from_lines(request.lines.unwrap_or(VideoResolution::P768.lines()))
                .ok_or(MinimaxError::InvalidRequest(
                    "Hailuo renders 512P, 768P or 1080P",
                ))?;
        self.submit_video(&VideoRequest {
            prompt: request.prompt.clone(),
            first_frame: frame.clone(),
            first_frame_format: *format,
            last_frame: request.last_frame.clone(),
            duration: VideoDuration::nearest(f64::from(request.clip_seconds)),
            resolution,
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

impl<T: kineto_provider_veo::Transport> VideoGenerator for VeoClient<T> {
    type Error = VeoError;

    fn provider(&self) -> &str {
        VEO_VIDEO_PROVIDER
    }

    fn model(&self) -> &str {
        VeoClient::model(self).id()
    }

    fn capabilities(&self) -> VideoCapabilities {
        VeoClient::model(self).capabilities()
    }

    fn canonical_params(&self, clip_seconds: u8, lines: Option<u16>) -> String {
        format!(
            "duration={clip_seconds};resolution={}p;aspect=16:9",
            lines.unwrap_or(0)
        )
    }

    fn submit(&self, request: &VideoSubmission) -> Result<String, VeoError> {
        let first_frame = request
            .first_frame
            .clone()
            .ok_or(VeoError::InvalidRequest("Veo needs a first frame"))?;
        VeoClient::submit(
            self,
            &VeoRequest {
                prompt: request.prompt.clone(),
                first_frame,
                last_frame: request.last_frame.clone(),
                seconds: request.clip_seconds,
                lines: request
                    .lines
                    .ok_or(VeoError::InvalidRequest("Veo needs a clip height"))?,
            },
        )
    }

    fn query(&self, task: &ProviderJobId) -> Result<TaskState, VeoError> {
        Ok(match self.operation(task.as_str())? {
            OperationState::Running => TaskState::Pending,
            OperationState::Succeeded { video } => TaskState::Succeeded { file: video },
        })
    }

    fn fetch(&self, file: &str) -> Result<Vec<u8>, VeoError> {
        self.download(file)
    }

    fn class(&self, error: &VeoError) -> ErrorClass {
        error.class()
    }

    fn certainly_not_accepted(&self, error: &VeoError) -> bool {
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
        resolutions: [DEMO_CLIP_LINES].into_iter().collect(),
        clip_prices: [(
            VideoClip {
                seconds: DEMO_CLIP_SECONDS,
                lines: DEMO_CLIP_LINES,
            },
            Price::usd(0),
        )]
        .into_iter()
        .collect(),
        ..VideoCapabilities::default()
    }
}

impl VideoGenerator for LocalDemoVideos {
    type Error = Infallible;

    fn provider(&self) -> &str {
        DEMO_VIDEO_PROVIDER
    }

    fn model(&self) -> &str {
        DEMO_VIDEO_MODEL
    }

    fn capabilities(&self) -> VideoCapabilities {
        demo_video_capabilities()
    }

    fn needs_frame_image(&self) -> bool {
        false
    }

    fn canonical_params(&self, clip_seconds: u8, _lines: Option<u16>) -> String {
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
    /// Google Veo, one model of it. Paid. Without a client, as for
    /// MiniMax.
    Veo {
        model: VeoModel,
        client: Option<VeoVideoClient>,
    },
    /// A plugin the user registered (ADR-0006), as it described itself.
    /// One that declares no video makes none.
    Plugin(Arc<PluginProvider>),
}

/// Which video provider a session generates with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoProviderKind {
    LocalDemo,
    Minimax,
    Veo,
    Plugin,
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
    /// `minimax` for MiniMax with the key `secrets` holds, `veo` for Veo's
    /// Lite model with its key (`veo:fast` and `veo:standard` for the
    /// others), `plugin:<id>` for the plugin registered as `<id>`. `None`
    /// when nothing is named, so the session follows its image provider. A
    /// paid provider is used only when it is named.
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
            Some(veo) if veo == VEO_PROVIDER || veo.starts_with(&format!("{VEO_PROVIDER}:")) => {
                let model = match veo.strip_prefix(&format!("{VEO_PROVIDER}:")) {
                    None => VeoModel::default(),
                    Some(name) => VeoModel::parse(name)
                        .ok_or_else(|| ProviderSelectionError::Unknown(veo.to_owned()))?,
                };
                Ok(Some(Self::Veo {
                    model,
                    client: secrets
                        .api_key(VEO_PROVIDER)
                        .map_err(ProviderSelectionError::Secret)?
                        .map(|key| VeoClient::shared(key, model)),
                }))
            }
            Some(other) => match selected_plugin(other) {
                Some(selected) => selected.map(|plugin| Some(Self::Plugin(plugin))),
                None => Err(ProviderSelectionError::Unknown(other.to_owned())),
            },
        }
    }

    #[must_use]
    pub fn kind(&self) -> VideoProviderKind {
        match self {
            Self::LocalDemo => VideoProviderKind::LocalDemo,
            Self::Minimax(_) => VideoProviderKind::Minimax,
            Self::Veo { .. } => VideoProviderKind::Veo,
            Self::Plugin(_) => VideoProviderKind::Plugin,
        }
    }

    /// The key its video jobs are kept under.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::LocalDemo => DEMO_VIDEO_PROVIDER.to_owned(),
            Self::Minimax(_) => VIDEO_PROVIDER.to_owned(),
            Self::Veo { .. } => VEO_VIDEO_PROVIDER.to_owned(),
            Self::Plugin(plugin) => plugin::video_key(plugin.key()),
        }
    }

    #[must_use]
    pub fn capabilities(&self) -> VideoCapabilities {
        match self {
            Self::LocalDemo => demo_video_capabilities(),
            Self::Minimax(_) => video_capabilities(),
            Self::Veo { model, .. } => model.capabilities(),
            Self::Plugin(plugin) => plugin.video_capabilities().cloned().unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn info(&self) -> VideoProviderInfo {
        let capabilities = self.capabilities();
        let clip_seconds = capabilities.nearest_clip_seconds(0.0).unwrap_or(0);
        let price = capabilities
            .clip_price(clip_seconds)
            .map_or(0, |price| price.amount_micros);
        VideoProviderInfo {
            kind: self.kind(),
            ready: match self {
                Self::LocalDemo => true,
                Self::Minimax(client) => client.is_some(),
                Self::Veo { client, .. } => client.is_some(),
                Self::Plugin(plugin) => plugin.video_capabilities().is_some(),
            },
            clip_cost_micros: price,
            clip_seconds,
        }
    }
}

/// What a shot's next video costs before it is asked for: the clip length
/// its plan asks for, as the provider renders it, and that clip's price.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoEstimate {
    pub clip_seconds: u8,
    /// `None` when the provider declares no price for that length.
    pub cost_micros: Option<u64>,
}

impl VideoProvider {
    /// What one take of a shot whose plan asks for `target` seconds, and
    /// whose dialogue takes `dialogue_ms`, costs.
    #[must_use]
    pub fn estimate(&self, target: Option<f64>, dialogue_ms: Option<u32>) -> Option<VideoEstimate> {
        let capabilities = self.capabilities();
        let clip_seconds = video_prompt::clip_seconds(&capabilities, target, dialogue_ms)?;
        Some(VideoEstimate {
            clip_seconds,
            cost_micros: capabilities
                .clip_price(clip_seconds)
                .map(|price| price.amount_micros),
        })
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
    pub provider: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// The shot's intent when the video was asked for, which the video
    /// records so a later change says it is stale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    intent: Option<serde_json::Value>,
    /// The director's notes it is made with, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    notes: Vec<String>,
    /// The take it branches from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    branched_from: Option<String>,
    /// The clip's length and the provider parameters it is asked with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    clip_seconds: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parameters: Option<String>,
    /// The keyframe the clip closes on and its shot, as `scene/shot`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    end_frame: Option<ReceiptEndFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ReceiptEndFrame {
    artifact_id: String,
    content_hash: String,
    shot: String,
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

fn provider_key(provider: &str) -> ProviderKey {
    ProviderKey::new(provider).expect("a video provider's key is a provider key")
}

/// How video jobs of `provider` run: two submissions at once to a paid
/// provider; the free demo answers at once, so it is not held back.
fn policy(provider: &str) -> JobRuntimePolicy {
    let concurrency = if provider == DEMO_VIDEO_PROVIDER {
        64
    } else {
        2
    };
    JobRuntimePolicy {
        execution: ProviderExecutionPolicy::new(
            NonZeroU16::new(concurrency).expect("static concurrency is non-zero"),
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

fn runtime<'a>(project: &'a CanonicalProject, provider: &str) -> JobRuntime<'a> {
    JobRuntime::new(project, provider_key(provider), policy(provider))
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
        let capabilities = self.generator.capabilities();
        let price = request
            .lines
            .and_then(|lines| {
                capabilities.clip_prices.get(&VideoClip {
                    seconds: request.clip_seconds,
                    lines,
                })
            })
            .copied()
            .ok_or(SubmitError::Handle(VideoFailure {
                class: ErrorClass::Terminal,
                source: Box::new(VideoError::Receipt("the clip has no price")),
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
    provider: &str,
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
        provider: provider.to_owned(),
        task: task.clone(),
    }))
}

/// What the director asks of a shot's next video generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoAsk {
    /// How many candidates, from 1 to [`MAX_VIDEO_CANDIDATES`].
    pub count: u8,
    /// The director's note, for this generation only.
    pub note: Option<String>,
    /// A take to branch from: its notes come before this one's, and the
    /// new candidates record it.
    pub branch_from: Option<String>,
}

impl Default for VideoAsk {
    fn default() -> Self {
        Self {
            count: 1,
            note: None,
            branch_from: None,
        }
    }
}

/// What submitting a generation started.
#[derive(Debug)]
pub struct Started {
    /// The candidates submitted, or submitted before and followed again, in
    /// candidate order.
    pub jobs: Vec<VideoJob>,
    /// Why the next candidate, and every one after it, was not submitted.
    pub error: Option<GenerationError>,
}

/// Submit the shot's next video, of one candidate. This is the paid call; it
/// returns as soon as the provider has accepted the task.
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
    single(start_videos(
        project,
        shot,
        generator,
        &VideoAsk::default(),
        hooks,
        false,
    )?)
}

/// Submit the shot's video again although its last submission is
/// unresolved: the provider may already have accepted, and charged for,
/// that one. Only on the director's explicit decision to risk paying twice.
pub fn resubmit<G: VideoGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
) -> Result<VideoJob, GenerationError> {
    single(start_videos(
        project,
        shot,
        generator,
        &VideoAsk::default(),
        &mut NoHooks,
        true,
    )?)
}

fn single(started: Started) -> Result<VideoJob, GenerationError> {
    match (started.jobs.into_iter().next(), started.error) {
        (_, Some(error)) => Err(error),
        (Some(job), None) => Ok(job),
        (None, None) => Err(GenerationError::Unexpected("nothing was submitted")),
    }
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
            requires_resumable_jobs: true,
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

/// The shot in play that continues from `key`, with its intent, and its
/// locked keyframe and that keyframe's image, when it has them.
#[allow(clippy::type_complexity)]
fn successor(
    project: &CanonicalProject,
    key: &ShotKey,
) -> Result<
    Option<(
        ShotKey,
        Option<ShotIntent>,
        Option<(
            kineto_project::ArtifactDependency,
            Option<ProjectRelativePath>,
        )>,
    )>,
    GenerationError,
> {
    let in_play = shots_in_play(project).map_err(|error| GenerationError::Provider {
        source: Box::new(error),
        class: ErrorClass::Terminal,
    })?;
    for other in in_play {
        if &other == key {
            continue;
        }
        let next = ShotWorkflow::load(project, &other)?;
        let intent = next.intent()?;
        if intent
            .as_ref()
            .and_then(|intent| intent.continuity_from.as_deref())
            .is_some_and(|from| from == key.shot())
        {
            return Ok(Some((other.clone(), intent, next.first_frame()?)));
        }
    }
    Ok(None)
}

/// The clip lengths `capabilities` render, shortest first.
#[cfg(test)]
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

/// The notes a generation is made with: those of the take it branches
/// from, then the director's new note.
fn notes_for(shot: &ShotWorkflow, ask: &VideoAsk) -> Result<Vec<String>, GenerationError> {
    let mut notes = match &ask.branch_from {
        Some(branch) => {
            shot.video_history()?
                .into_iter()
                .find(|take| &take.artifact_id == branch)
                .ok_or(GenerationError::Shot(
                    kineto_project::shot::ShotWorkflowError::InvalidCandidate,
                ))?
                .notes
        }
        None => Vec::new(),
    };
    if let Some(note) = ask
        .note
        .as_deref()
        .map(str::trim)
        .filter(|note| !note.is_empty())
    {
        notes.push(note.to_owned());
    }
    Ok(notes)
}

/// Submit the shot's next video generation as `ask` says: each candidate a
/// paid write-ahead job of its own. A candidate that cannot be submitted
/// stops the rest; those already submitted are followed. With
/// `risk_paying_twice`, a candidate whose last submission is unresolved is
/// submitted again.
pub fn start_videos<G: VideoGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
    ask: &VideoAsk,
    hooks: &mut impl VideoHooks,
    risk_paying_twice: bool,
) -> Result<Started, GenerationError> {
    if ask.count == 0 || ask.count > MAX_VIDEO_CANDIDATES {
        return Err(GenerationError::Shot(
            kineto_project::shot::ShotWorkflowError::InvalidCandidate,
        ));
    }
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
    let intent = shot.intent()?;
    let dialogue_ms = crate::dialogue::shot_dialogue_ms(
        project,
        shot.key().scene(),
        intent
            .as_ref()
            .and_then(|intent| intent.dialogue.as_deref()),
    )?;
    let successor = successor(project, shot.key())?;
    // A shot that continues from this one in the same setup closes the clip
    // on its locked keyframe, where the provider takes a last frame, in a
    // clip of this length, and that image. A cut is left to the editor.
    let cut = successor
        .as_ref()
        .and_then(|(_, next, _)| match (&intent, next) {
            (Some(this), Some(next)) => video_prompt::cut_to(this, next),
            _ => Some("the plan does not say how both shots are framed".to_owned()),
        });
    let clip = video_prompt::clip_seconds(
        &capabilities,
        intent.as_ref().and_then(|intent| intent.duration_target),
        dialogue_ms,
    );
    let too_short = clip
        .filter(|seconds| {
            !capabilities.end_frame_seconds.is_empty()
                && !capabilities.end_frame_seconds.contains(seconds)
        })
        .map(|seconds| {
            let lengths: Vec<String> = capabilities
                .end_frame_seconds
                .iter()
                .map(u8::to_string)
                .collect();
            format!(
                "the provider closes on a last frame only in a clip of {} s, and this clip is \
                 {seconds} s",
                lengths.join(" or ")
            )
        });
    let mut end_frame: Option<(EndFrame, Vec<u8>, ImageFormat)> = None;
    let end_frame_use = match &successor {
        None => None,
        Some(_) if !capabilities.end_frame => Some(EndFrameUse::NotTaken),
        Some(_) if cut.is_some() => cut.as_deref().map(EndFrameUse::Cut),
        Some(_) if too_short.is_some() => too_short.as_deref().map(EndFrameUse::Unfit),
        Some((_, _, None)) => Some(EndFrameUse::NotLocked),
        Some((_, _, Some((_, None)))) => Some(EndFrameUse::NoImage),
        Some((next, _, Some((keyframe, Some(path))))) => {
            let bytes = project
                .root()
                .read(path)
                .map_err(|error| video_error(VideoError::Fs(error)))?;
            if ContentHash::of(&bytes) != keyframe.content_hash {
                return Err(GenerationError::Shot(
                    kineto_project::shot::ShotWorkflowError::MediaMismatch(
                        path.as_path().to_string_lossy().into_owned(),
                    ),
                ));
            }
            let fits = |format: &ImageFormat| {
                capabilities.end_frame_formats.contains(format)
                    && capabilities
                        .max_end_frame_bytes
                        .is_none_or(|max| bytes.len() as u64 <= max)
            };
            match sniff(&bytes).filter(fits) {
                Some(format) => {
                    end_frame = Some((
                        EndFrame {
                            keyframe: kineto_project::ArtifactDependency {
                                field: Some(END_FRAME_FIELD.to_owned()),
                                ..keyframe.clone()
                            },
                            shot: next.clone(),
                        },
                        bytes,
                        format,
                    ));
                    Some(EndFrameUse::Sent)
                }
                None => Some(EndFrameUse::Unfit(
                    "its keyframe is not an image the provider takes as a last frame",
                )),
            }
        }
    };
    let notes = notes_for(shot, ask)?;
    let style = kineto_project::style::locked(project)
        .map_err(|error| GenerationError::Provider {
            source: Box::new(error),
            class: ErrorClass::Terminal,
        })?
        .map(|lock| match lock.note {
            Some(note) => format!("{}. {note}", lock.directives.trim_end_matches(['.', ' '])),
            None => lock.directives,
        });
    let preferences = crate::preferences::in_force(project)?
        .and_then(|in_force| in_force.sentence(&crate::preferences::VIDEO_DIMENSIONS));
    let prompt = video_prompt::compile(VideoPromptInput {
        shot: intent.as_ref(),
        direction: shot.direction(),
        keyframe_hash: keyframe.content_hash.as_str(),
        capabilities: &capabilities,
        camera_command: intent
            .as_ref()
            .and_then(|intent| intent.camera_motion.as_deref())
            .and_then(|motion| generator.camera_command(motion)),
        successor: successor
            .as_ref()
            .zip(end_frame_use)
            .map(|((next, _, _), end_frame)| Successor {
                shot_id: next.shot(),
                end_frame,
            }),
        notes: &notes,
        style: style.as_deref(),
        dialogue_ms,
        preferences: preferences.as_deref(),
    })?;
    let lines = capabilities.clip_lines(prompt.duration_seconds);
    let params = generator.canonical_params(prompt.duration_seconds, lines);
    let plans = shot.plan_videos(
        project,
        VideoGenerationSpec {
            provider: generator.provider(),
            model: generator.model(),
            canonical_params: params.as_bytes(),
            compiler_version: VIDEO_COMPILER_VERSION,
            notes: &notes,
            branched_from: ask.branch_from.as_deref(),
            end_frame: end_frame.as_ref().map(|(end, _, _)| &end.keyframe),
        },
        ask.count,
    )?;
    let receipt = Receipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        artifact_id: String::new(),
        input_hash: String::new(),
        model: generator.model().to_owned(),
        keyframe_artifact_id: keyframe.artifact_id.as_str().to_owned(),
        keyframe_content_hash: keyframe.content_hash.as_str().to_owned(),
        provenance: provenance_of(&prompt.compiled, &prompt.prompt_hash),
        task_id: None,
        refused_attempt: None,
        content_hash: None,
        intent: Some(shot.intent_record()?),
        notes,
        branched_from: ask.branch_from.clone(),
        clip_seconds: Some(prompt.duration_seconds),
        parameters: Some(params.clone()),
        end_frame: end_frame.as_ref().map(|(end, _, _)| ReceiptEndFrame {
            artifact_id: end.keyframe.artifact_id.as_str().to_owned(),
            content_hash: end.keyframe.content_hash.as_str().to_owned(),
            shot: format!("{}/{}", end.shot.scene(), end.shot.shot()),
        }),
    };
    let request = VideoSubmission {
        prompt: prompt.compiled.prompt().to_owned(),
        first_frame: frame,
        clip_seconds: prompt.duration_seconds,
        lines,
        last_frame: end_frame.map(|(_, bytes, format)| (bytes, format)),
    };
    let mut started = Started {
        jobs: Vec::with_capacity(plans.len()),
        error: None,
    };
    for plan in &plans {
        let submission = Submission {
            plan,
            receipt: &receipt,
            request: &request,
            risk_paying_twice,
        };
        match submit_one(project, shot, generator, hooks, &submission) {
            Ok(job) => started.jobs.push(job),
            Err(error) => {
                started.error = Some(error);
                break;
            }
        }
    }
    Ok(started)
}

/// One planned candidate and what it is submitted with.
struct Submission<'a> {
    plan: &'a VideoPlan,
    /// The generation's receipt, without the candidate's ID and hash.
    receipt: &'a Receipt,
    request: &'a VideoSubmission,
    risk_paying_twice: bool,
}

/// Submit one planned candidate, or follow it if it was submitted before.
fn submit_one<G: VideoGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
    hooks: &mut impl VideoHooks,
    submission: &Submission<'_>,
) -> Result<VideoJob, GenerationError> {
    let plan = submission.plan;
    let provider = generator.provider();
    let runtime = runtime(project, provider);
    let job_id = JobId::new(plan.artifact_id().as_str())?;
    if let Some(record) = runtime.load(&job_id)?
        && record.intent().state() == IntentState::Prepared
    {
        match last_attempt(project.root(), &record) {
            // Never sent, but planned for other notes or another branch:
            // nothing reached the provider, so it is planned afresh.
            LastAttempt::None if record.intent().input_hash() != plan.input_hash() => {
                if runtime.retire_unattempted(&job_id)? {
                    remove_receipt(project.root(), plan.artifact_id());
                }
            }
            LastAttempt::None | LastAttempt::Accepted => {}
            // Asking again is what retries a submission the provider refused.
            LastAttempt::Refused => {
                if runtime.retire_failed(&job_id)? {
                    remove_receipt(project.root(), plan.artifact_id());
                }
            }
            LastAttempt::Unresolved if submission.risk_paying_twice => {
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
                artifact_id: plan.artifact_id().as_str().to_owned(),
                input_hash: plan.input_hash().as_str().to_owned(),
                ..submission.receipt.clone()
            },
        )
        .map_err(video_error)?;
    }

    let mut submit = Submit {
        generator,
        root: project.root(),
        artifact_id: plan.artifact_id(),
        input_hash: plan.input_hash(),
        attempt: record.attempts().saturating_add(1),
        hooks,
    };
    match invoke_when_permitted(&runtime, &job_id, &mut submit, submission.request) {
        Ok(DispatchOutcome::Pending {
            provider_job_id, ..
        }) => Ok(VideoJob {
            shot: shot.key().clone(),
            revision: plan.revision(),
            artifact_id: plan.artifact_id().clone(),
            provider: provider.to_owned(),
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
    let runtime = runtime(project, &done.job.provider);
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
            provider: done.job.provider.clone(),
            model: receipt.model,
            media_path: video_media_path(shot.key(), &done.job.artifact_id)?,
            provenance: Some(receipt.provenance),
            keyframe,
            intent: receipt.intent,
            notes: receipt.notes,
            branched_from: receipt.branched_from,
            clip_seconds: receipt.clip_seconds,
            parameters: receipt.parameters,
            end_frame: receipt
                .end_frame
                .map(|end| {
                    let invalid = || video_error(VideoError::Receipt("end frame is invalid"));
                    let (scene, shot_id) = end.shot.split_once('/').ok_or_else(invalid)?;
                    Ok::<_, GenerationError>(EndFrame {
                        keyframe: kineto_project::ArtifactDependency {
                            artifact_id: ArtifactId::new(end.artifact_id).map_err(|_| invalid())?,
                            content_hash: ContentHash::new(end.content_hash)
                                .map_err(|_| invalid())?,
                            impact: DependencyImpact::Semantic,
                            field: Some(END_FRAME_FIELD.to_owned()),
                        },
                        shot: ShotKey::new(scene, shot_id).map_err(|_| invalid())?,
                    })
                })
                .transpose()?,
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
pub fn in_flight(project: &CanonicalProject, provider: &str) -> Result<InFlight, JobRuntimeError> {
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
                    let placed = record
                        .intent()
                        .artifact_id()
                        .and_then(|artifact_id| {
                            VideoPlan::for_artifact(
                                artifact_id,
                                record.intent().input_hash().clone(),
                            )
                            .ok()
                        })
                        .and_then(|plan| {
                            let shot = locate(project, plan.shot_id()).ok().flatten()?;
                            let current = ShotWorkflow::load(project, &shot)
                                .ok()?
                                .snapshot()
                                .ok()?
                                .video_revision;
                            Some((shot, plan.revision() > current))
                        });
                    match placed {
                        // A candidate of a generation that has a video since
                        // can no longer become the shot's; asking for another
                        // video is a new request, not this one again.
                        Some((_, false)) => {}
                        Some((shot, true)) => found.unresolved.push(shot),
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

/// The Veo client type the session uses.
pub type VeoVideoClient = VeoClient<kineto_provider_veo::SharedTransport>;

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
        assert!(matches!(
            VideoProvider::select(Some("veo"), &with_key).unwrap(),
            Some(VideoProvider::Veo {
                model: VeoModel::Lite,
                client: Some(_)
            })
        ));
        assert!(matches!(
            VideoProvider::select(Some("veo:standard"), &without_key).unwrap(),
            Some(VideoProvider::Veo {
                model: VeoModel::Standard,
                client: None
            })
        ));
        assert!(VideoProvider::select(Some("veo:ultra"), &with_key).is_err());
        assert!(VideoProvider::select(Some("sora"), &with_key).is_err());
    }

    #[test]
    fn veo_is_priced_per_second_at_720p_and_ready_only_with_its_key() {
        let lite = VideoProvider::Veo {
            model: VeoModel::Lite,
            client: None,
        };
        assert_eq!(lite.key(), VEO_VIDEO_PROVIDER);
        let info = lite.info();
        assert!(!info.ready);
        assert_eq!((info.clip_seconds, info.clip_cost_micros), (4, 200_000));
        assert_eq!(
            lite.estimate(Some(8.0), None),
            Some(VideoEstimate {
                clip_seconds: 8,
                cost_micros: Some(400_000)
            })
        );
    }

    #[test]
    fn a_take_is_priced_at_the_length_the_plan_asks_for() {
        let minimax = VideoProvider::Minimax(None);
        let estimate = |target| minimax.estimate(target, None).unwrap();
        assert_eq!(
            estimate(Some(10.0)),
            VideoEstimate {
                clip_seconds: 10,
                cost_micros: Some(560_000)
            }
        );
        // A tie goes to the shorter, cheaper clip, as the compiler does.
        assert_eq!(estimate(Some(8.0)).clip_seconds, 6);
        assert_eq!(estimate(Some(7.0)).cost_micros, Some(280_000));
        assert_eq!(estimate(None).clip_seconds, 6);
        assert_eq!(
            VideoProvider::LocalDemo.estimate(Some(10.0), None),
            Some(VideoEstimate {
                clip_seconds: 2,
                cost_micros: Some(0)
            })
        );
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
