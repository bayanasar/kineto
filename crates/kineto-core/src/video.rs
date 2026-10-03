//! A shot's video as an asynchronous remote job.
//!
//! Submitting is the only paid step, and it runs as a write-ahead job: the
//! intent is prepared, a receipt with everything the video's record needs is
//! written, the task is submitted, its ID is written into the receipt the
//! moment MiniMax accepts it, and the intent is marked invoked with the task
//! ID before the command returns. Watching the task is free and
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
//! MiniMax reports as failed is retired, so asking again submits afresh.
//!
//! An attempted submission with no task recorded is submitted again only if
//! its receipt shows MiniMax certainly did not accept it. Otherwise MiniMax
//! may have accepted and charged for it (a crash before the task was written
//! down, or a timeout after the request was sent), so it is unresolved:
//! asking again is refused until the director chooses to [`resubmit`],
//! accepting that the video may be paid for twice.

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
    JobRuntimePolicy, PaidJobAdapter, PrepareOutcome, ProviderKey, Reconciliation, RuntimeIntent,
    StartupOutcome,
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
        AttachOutcome, PromptProvenance, ShotKey, ShotWorkflow, VideoGenerationSpec, VideoOutput,
        VideoPlan, locate, video_media_path,
    },
};
use kineto_provider_minimax::{
    MinimaxClient, MinimaxError, SharedTransport, Transport, VIDEO_MODEL, VideoDuration,
    VideoRequest, VideoResolution, VideoTask, VideoTaskState, sniff,
};
use serde::{Deserialize, Serialize};

use crate::{
    generation::{GenerationError, NoHooks, parse_job_key, provenance_of},
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
    /// MiniMax's task, written as soon as the submission is accepted. From
    /// then on the video is paid for and is never submitted again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    /// The attempt MiniMax certainly did not accept: it was never sent or
    /// was refused. Any other attempt without a task may have been charged.
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

/// The task MiniMax accepted for this video, if its receipt recorded one.
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

fn record_task(
    root: &ProjectRoot,
    artifact_id: &ArtifactId,
    task: &VideoTask,
) -> Result<(), VideoError> {
    let mut receipt = read_receipt(root, artifact_id)?;
    receipt.task_id = Some(task.as_str().to_owned());
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
    /// MiniMax accepted it; its task is in the receipt.
    Accepted,
    /// MiniMax certainly did not accept it.
    Refused,
    /// MiniMax may have accepted it.
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

fn task_handle(task: &str) -> Result<ProviderJobId, MinimaxError> {
    ProviderJobId::new(task).map_err(|_| MinimaxError::Malformed("task ID is not a job handle"))
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
struct Submit<'a, T, H> {
    client: &'a MinimaxClient<T>,
    root: &'a ProjectRoot,
    artifact_id: &'a ArtifactId,
    input_hash: &'a InputHash,
    /// The attempt this submission is, as the intent will count it.
    attempt: u16,
    hooks: &'a mut H,
}

impl<T: Transport, H: VideoHooks> PaidJobAdapter for Submit<'_, T, H> {
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
        // Accepted earlier, but the intent was never marked invoked: follow
        // that task instead of paying for another.
        if let Some(task) = accepted_task(self.root, self.artifact_id, self.input_hash) {
            return task_handle(&task).map(Invocation::Pending);
        }
        let task = match self.client.submit_video(request) {
            Ok(task) => task,
            Err(error) => {
                // Only a proven refusal lets this video be submitted again
                // without the director's say; if it cannot be written down
                // the attempt stays unresolved, which fails closed.
                if error.certainly_not_accepted() {
                    let _ = record_refusal(self.root, self.artifact_id, self.attempt);
                }
                return Err(error);
            }
        };
        // MiniMax may have charged from here on. The task goes into the
        // receipt before anything else can fail. If that write fails the
        // runtime's intent save is the remaining record, so the task is
        // still returned for it to keep.
        self.hooks.reached(VideoPoint::Accepted);
        let _ = record_task(self.root, self.artifact_id, &task);
        self.hooks.reached(VideoPoint::Recorded);
        task_handle(task.as_str()).map(Invocation::Pending)
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
struct Deferred<'a> {
    root: &'a ProjectRoot,
}

impl PaidJobAdapter for Deferred<'_> {
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
        key: &IdempotencyKey,
    ) -> Result<Reconciliation<()>, MinimaxError> {
        // MiniMax keeps no record a submission could be found by, so the
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

    fn classify_error(&self, error: &MinimaxError) -> ErrorClass {
        error.class()
    }
}

fn job_from_intent(
    project: &CanonicalProject,
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
    let Ok(Some(shot)) = locate(project, plan.shot_id()) else {
        return Ok(None);
    };
    Ok(VideoTask::new(task.as_str()).ok().map(|task| VideoJob {
        shot,
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
    start_with(project, shot, client, &mut NoHooks)
}

/// [`start`], stopping at `hooks`' points.
pub fn start_with<T: Transport>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    client: &MinimaxClient<T>,
    hooks: &mut impl VideoHooks,
) -> Result<VideoJob, GenerationError> {
    submit(project, shot, client, hooks, false)
}

/// Submit the shot's video again although its last submission is
/// unresolved: MiniMax may already have accepted, and charged for, that
/// one. Only on the director's explicit decision to risk paying twice.
pub fn resubmit<T: Transport>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    client: &MinimaxClient<T>,
) -> Result<VideoJob, GenerationError> {
    submit(project, shot, client, &mut NoHooks, true)
}

fn submit<T: Transport>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    client: &MinimaxClient<T>,
    hooks: &mut impl VideoHooks,
    risk_paying_twice: bool,
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
            return job_from_intent(project, &runtime, &job_id)?
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
                model: VIDEO_MODEL.to_owned(),
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

    let request = VideoRequest {
        prompt: prompt.compiled.prompt().to_owned(),
        first_frame: frame,
        first_frame_format: format,
        duration: duration(prompt.duration_seconds),
        resolution: RESOLUTION,
    };
    let mut submit = Submit {
        client,
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
            task: VideoTask::new(provider_job_id.as_str())
                .map_err(|_| GenerationError::Unexpected("a task ID changed shape"))?,
        }),
        Ok(DispatchOutcome::Completed { .. }) => Err(GenerationError::Unexpected(
            "a video submission completed synchronously",
        )),
        Err(InvokePreparedError::Runtime(error)) => Err(error.into()),
        // A failure that may have reached MiniMax may have been charged.
        Err(InvokePreparedError::Provider { source, .. }) if !source.certainly_not_accepted() => {
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
    let media = video_media_path(&job.shot, &job.artifact_id)
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

pub fn in_flight(project: &CanonicalProject) -> Result<InFlight, JobRuntimeError> {
    let runtime = runtime(project);
    let mut found = InFlight::default();
    let mut deferred = Deferred {
        root: project.root(),
    };
    for outcome in runtime.reconcile_startup(&mut deferred)? {
        match outcome {
            StartupOutcome::StillPending { job_id } => {
                match job_from_intent(project, &runtime, &job_id)? {
                    Some(job) => found.jobs.push(job),
                    None => found.needs_attention += 1,
                }
            }
            // Never submitted, or MiniMax refused it: the director asks again
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
