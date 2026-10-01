//! A shot's video as an asynchronous remote job.
//!
//! Submitting is the only paid step, and it runs as a write-ahead job: the
//! intent is prepared, a receipt with everything the video's record needs is
//! written, the task is submitted, and the intent is marked invoked with
//! MiniMax's task ID before the command returns. Watching the task is free and
//! runs on its own thread; it downloads the finished file into the shot's
//! `videos/` directory and hands the job back to the session worker, the only
//! code that changes canonical state, to attach:
//!
//! ```text
//! worker:  plan -> prepare -> receipt -> submit (paid) -> intent invoked(task)
//! watcher: query ... query (free) -> download -> media + receipt hash
//! worker:  attach -> acknowledge -> drop receipt
//! ```
//!
//! A restart finds invoked video intents and watches their tasks again; it
//! never submits. A task MiniMax reports as failed is retired, so asking again
//! submits afresh.

use std::{
    error::Error,
    fmt,
    num::NonZeroU16,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

use kineto_job_runtime::{
    DispatchOutcome, Invocation, InvokePreparedError, JobRuntime, JobRuntimeError,
    JobRuntimePolicy, PaidJobAdapter, PrepareOutcome, ProviderKey, Reconciliation, StartupOutcome,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, ErrorClass, FallbackPolicy, IdempotencyKey, IntentState, JobId,
    PaidFallback, ProviderExecutionPolicy, ProviderJobId,
};
use kineto_project::{
    ArtifactDependency, ArtifactId, ContentHash, DependencyImpact, InputHash,
    fs::{ProjectFsError, ProjectRelativePath, ProjectRoot},
    manifest::CanonicalProject,
    shot::{
        AttachOutcome, PromptProvenance, ShotWorkflow, VideoGenerationSpec, VideoOutput, VideoPlan,
        video_media_path,
    },
};
use kineto_provider_minimax::{
    MinimaxClient, MinimaxError, SharedTransport, Transport, VIDEO_MODEL, VideoDuration,
    VideoRequest, VideoResolution, VideoTask, VideoTaskState, sniff,
};
use serde::{Deserialize, Serialize};

use crate::{
    generation::{GenerationError, provenance_of},
    video_prompt::{self, VIDEO_COMPILER_VERSION},
};

/// The capability a shot video is generated with.
pub const VIDEO_OPERATION: &str = "video.generate";
/// Provider key video jobs are recorded under. Separate from MiniMax image
/// jobs, whose recovery must never see a video's remote task.
pub const VIDEO_PROVIDER: &str = "minimax-video";
/// Clip lengths the video model renders, in seconds.
const DURATIONS: [u8; 2] = [6, 10];
const RESOLUTION: VideoResolution = VideoResolution::P768;
const RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAX_VIDEO_PROMPT_CHARS: usize = 2000;
/// How often a watcher asks MiniMax about its task, and how often it checks
/// that its session is still open while it waits.
pub const POLL_INTERVAL: Duration = Duration::from_secs(10);
const WAKE_INTERVAL: Duration = Duration::from_millis(200);

/// A submitted video, as a watcher follows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoJob {
    pub shot_number: u32,
    pub revision: u16,
    pub artifact_id: ArtifactId,
    pub task: VideoTask,
}

/// What a watcher found.
#[derive(Debug)]
pub struct VideoDone {
    pub job: VideoJob,
    /// The video is on disk and its receipt names its bytes, or MiniMax
    /// reported the task failed.
    pub result: Result<(), MinimaxError>,
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

fn remove_receipt(root: &ProjectRoot, artifact_id: &ArtifactId) {
    // Best effort: a receipt left behind is never read again once its job
    // is acknowledged or retired.
    if let Ok(path) = receipt_path(artifact_id) {
        let _ = std::fs::remove_file(root.resolve(&path));
    }
}

fn provider_key() -> ProviderKey {
    ProviderKey::new(VIDEO_PROVIDER).expect("static provider key is valid")
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

fn runtime(project: &CanonicalProject) -> JobRuntime<'_> {
    JobRuntime::new(project, provider_key(), policy())
}

fn duration(seconds: u8) -> VideoDuration {
    if seconds >= 10 {
        VideoDuration::Ten
    } else {
        VideoDuration::Six
    }
}

/// Estimated cost of one video with the default clip settings, in USD micros.
#[must_use]
pub fn estimated_cost_micros() -> u64 {
    // Published price of a 6-second 768P Hailuo clip.
    280_000
}

/// Submits a planned video and records its task.
struct Submit<'a, T> {
    client: &'a MinimaxClient<T>,
}

impl<T: Transport> PaidJobAdapter for Submit<'_, T> {
    type Request = VideoRequest;
    type Output = ();
    type Error = MinimaxError;

    fn estimate_cost(&self, request: &VideoRequest) -> Result<CostEstimate, MinimaxError> {
        Ok(self
            .client
            .estimate_video_cost(request.duration, request.resolution))
    }

    fn invoke(
        &mut self,
        request: &VideoRequest,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<()>, MinimaxError> {
        let task = self.client.submit_video(request)?;
        ProviderJobId::new(task.as_str())
            .map(Invocation::Pending)
            .map_err(|_| MinimaxError::Malformed("task ID is not a job handle"))
    }

    fn reconcile(&mut self, _id: &ProviderJobId) -> Result<Reconciliation<()>, MinimaxError> {
        unreachable!("submission never reconciles; watchers follow tasks")
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        _key: &IdempotencyKey,
    ) -> Result<Reconciliation<()>, MinimaxError> {
        unreachable!("submission never reconciles; watchers follow tasks")
    }

    fn classify_error(&self, error: &MinimaxError) -> ErrorClass {
        error.class()
    }
}

/// Startup reconciliation that leaves every remote task to a watcher, so
/// opening a project never waits on the network.
struct Deferred;

impl PaidJobAdapter for Deferred {
    type Request = VideoRequest;
    type Output = ();
    type Error = MinimaxError;

    fn estimate_cost(&self, _request: &VideoRequest) -> Result<CostEstimate, MinimaxError> {
        Err(MinimaxError::Malformed("recovery estimates nothing"))
    }

    fn invoke(
        &mut self,
        _request: &VideoRequest,
        _key: &IdempotencyKey,
    ) -> Result<Invocation<()>, MinimaxError> {
        Err(MinimaxError::Malformed("recovery submits nothing"))
    }

    fn reconcile(&mut self, id: &ProviderJobId) -> Result<Reconciliation<()>, MinimaxError> {
        Ok(Reconciliation::Pending {
            provider_job_id: Some(id.clone()),
        })
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        _key: &IdempotencyKey,
    ) -> Result<Reconciliation<()>, MinimaxError> {
        // MiniMax keeps no record a submission could be found by.
        Ok(Reconciliation::NotFound)
    }

    fn classify_error(&self, error: &MinimaxError) -> ErrorClass {
        error.class()
    }
}

fn job_from_intent(
    runtime: &JobRuntime<'_>,
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
    Ok(VideoTask::new(task.as_str()).ok().map(|task| VideoJob {
        shot_number: plan.shot_number(),
        revision: plan.revision(),
        artifact_id: artifact_id.clone(),
        task,
    }))
}

/// Submit the shot's next video. This is the paid call; it returns as soon as
/// MiniMax has accepted the task.
pub fn start<T: Transport>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    client: &MinimaxClient<T>,
) -> Result<VideoJob, GenerationError> {
    let (keyframe, first_frame) = shot.first_frame()?.ok_or(GenerationError::Shot(
        kineto_project::shot::ShotWorkflowError::KeyframeNotLocked,
    ))?;
    let first_frame = first_frame.ok_or(GenerationError::NoFirstFrame)?;
    let prompt = video_prompt::compile(
        shot.intent()?.as_ref(),
        shot.direction(),
        keyframe.content_hash.as_str(),
        &DURATIONS,
        MAX_VIDEO_PROMPT_CHARS,
    )?;
    let params = format!("duration={};resolution=768P", prompt.duration_seconds);
    let plan = shot.plan_video(
        project,
        VideoGenerationSpec {
            provider: VIDEO_PROVIDER,
            model: VIDEO_MODEL,
            canonical_params: params.as_bytes(),
            compiler_version: VIDEO_COMPILER_VERSION,
        },
    )?;
    let frame = project
        .root()
        .read(&first_frame)
        .map_err(|error| video_error(VideoError::Fs(error)))?;
    if ContentHash::of(&frame) != keyframe.content_hash {
        return Err(GenerationError::Shot(
            kineto_project::shot::ShotWorkflowError::MediaMismatch(
                first_frame.as_path().to_string_lossy().into_owned(),
            ),
        ));
    }
    let format = sniff(&frame).ok_or(GenerationError::NoFirstFrame)?;

    let runtime = runtime(project);
    let job_id = JobId::new(plan.artifact_id().as_str())?;
    // Asking again is what retries a submission that failed.
    runtime.retire_failed(&job_id)?;
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
            return job_from_intent(&runtime, &job_id)?
                .ok_or(GenerationError::Unexpected("a submitted video has no task"));
        }
        IntentState::Reconciled => {
            return Err(GenerationError::Unexpected(
                "this video was already attached",
            ));
        }
    }

    write_receipt(
        project.root(),
        plan.artifact_id(),
        &Receipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            artifact_id: plan.artifact_id().as_str().to_owned(),
            input_hash: plan.input_hash().as_str().to_owned(),
            model: VIDEO_MODEL.to_owned(),
            keyframe_artifact_id: keyframe.artifact_id.as_str().to_owned(),
            keyframe_content_hash: keyframe.content_hash.as_str().to_owned(),
            provenance: provenance_of(&prompt.compiled, &prompt.prompt_hash),
            content_hash: None,
        },
    )
    .map_err(video_error)?;

    let request = VideoRequest {
        prompt: prompt.compiled.prompt().to_owned(),
        first_frame: frame,
        first_frame_format: format,
        duration: duration(prompt.duration_seconds),
        resolution: RESOLUTION,
    };
    match runtime.invoke_prepared(&job_id, &mut Submit { client }, &request) {
        Ok(DispatchOutcome::Pending {
            provider_job_id, ..
        }) => Ok(VideoJob {
            shot_number: plan.shot_number(),
            revision: plan.revision(),
            artifact_id: plan.artifact_id().clone(),
            task: VideoTask::new(provider_job_id.as_str())
                .map_err(|_| GenerationError::Unexpected("a task ID changed shape"))?,
        }),
        Ok(DispatchOutcome::Completed { .. }) => Err(GenerationError::Unexpected(
            "a video submission completed synchronously",
        )),
        Err(InvokePreparedError::Runtime(error)) => Err(error.into()),
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
pub fn watch<T: Transport>(
    client: &MinimaxClient<T>,
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
        let state = match client.query_video(&job.task) {
            Ok(state) => state,
            // Asking again is free; wait and ask again.
            Err(error) if error.class() == ErrorClass::Retryable => continue,
            Err(error) => {
                return Some(VideoDone {
                    job,
                    result: Err(error),
                });
            }
        };
        let VideoTaskState::Succeeded { file_id } = state else {
            continue;
        };
        match fetch(client, root, &job, &file_id) {
            Ok(()) => {
                return Some(VideoDone {
                    job,
                    result: Ok(()),
                });
            }
            Err(error) if error.class() == ErrorClass::Retryable => {}
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

fn fetch<T: Transport>(
    client: &MinimaxClient<T>,
    root: &ProjectRoot,
    job: &VideoJob,
    file_id: &str,
) -> Result<(), MinimaxError> {
    let bytes = client.fetch_video(file_id)?;
    let media = video_media_path(job.shot_number, &job.artifact_id)
        .map_err(|_| MinimaxError::Malformed("video path is invalid"))?;
    root.write_atomic(&media, &bytes)
        .map_err(|_| MinimaxError::Transport(kineto_provider_minimax::TransportFailure::Io))?;
    let mut receipt = read_receipt(root, &job.artifact_id)
        .map_err(|_| MinimaxError::Malformed("the video's receipt is missing"))?;
    receipt.content_hash = Some(ContentHash::of(&bytes).as_str().to_owned());
    write_receipt(root, &job.artifact_id, &receipt)
        .map_err(|_| MinimaxError::Transport(kineto_provider_minimax::TransportFailure::Io))
}

/// Attach a finished video, or retire a failed one so it can be asked for
/// again. Session worker only.
pub fn finish(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    done: VideoDone,
) -> Result<AttachOutcome, GenerationError> {
    let runtime = runtime(project);
    let job_id = JobId::new(done.job.artifact_id.as_str())?;
    if let Err(source) = done.result {
        runtime.retire_remote_failure(&job_id)?;
        remove_receipt(project.root(), &done.job.artifact_id);
        return Err(GenerationError::Provider {
            class: source.class(),
            source: Box::new(source),
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
            provider: VIDEO_PROVIDER.to_owned(),
            model: receipt.model,
            media_path: video_media_path(plan.shot_number(), &done.job.artifact_id)?,
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
    /// Records this build cannot read or place.
    pub needs_attention: usize,
}

pub fn in_flight(project: &CanonicalProject) -> Result<InFlight, JobRuntimeError> {
    let runtime = runtime(project);
    let mut found = InFlight::default();
    for outcome in runtime.reconcile_startup(&mut Deferred)? {
        match outcome {
            StartupOutcome::StillPending { job_id } => match job_from_intent(&runtime, &job_id)? {
                Some(job) => found.jobs.push(job),
                None => found.needs_attention += 1,
            },
            // Never submitted, or its submission failed: the director asks
            // again when they want it.
            StartupOutcome::RetryPrepared { .. } => {}
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
