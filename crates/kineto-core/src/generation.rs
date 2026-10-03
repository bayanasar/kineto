//! Shot candidate generation as durable write-ahead jobs.
//!
//! Each planned candidate is one job: its artifact ID is the job ID and the
//! target artifact, and the operation is `image.generate`. A generation runs
//!
//! ```text
//! prepare every intent (write-ahead) -> invoke all -> attach all candidates -> acknowledge
//! ```
//!
//! Candidates are invoked in parallel, one call each. [`complete`] drives a
//! plan to the end from whichever step it last reached durably, so a normal
//! run and the recovery of an interrupted one take the same path. Canonical
//! state changes only in the attach step, which is idempotent.
//!
//! Who generates is a [`CandidateGenerator`]: the deterministic local demo
//! generator or a real provider. Workflow code reaches them through
//! [`ImageProvider`] and never names a vendor.

use std::{
    error::Error,
    fmt,
    num::NonZeroU16,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use kineto_job_runtime::{
    DispatchOutcome, Invocation, InvokePreparedError, JobRuntime, JobRuntimeError,
    JobRuntimePolicy, PaidJobAdapter, PrepareOutcome, ProviderKey, Reconciliation, StartupOutcome,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, CurrencyCode, ErrorClass, FallbackPolicy, IdempotencyKey,
    IntentState, JobId, JobValueError, PaidFallback, ProviderExecutionPolicy, ProviderJobId,
};
use kineto_project::{
    ArtifactId, ContentHash, DependencyImpact, InputHash,
    bible::Bible,
    cast::{self, CastError, CastImageKind, CharacterLock, LockedImage},
    fs::ProjectRelativePath,
    manifest::CanonicalProject,
    revisions::{self, DependencyRecord, DocumentKind, RevisionStatus},
    shot::{
        self, AttachOutcome, CandidateOutput, DEMO_PROVIDER, Degradation, DroppedConstraintRecord,
        GENERATED_CANDIDATE_COUNT, GenerationPlan, PromptProvenance, ShotIntent, ShotKey,
        ShotWorkflow, ShotWorkflowError, demo_candidate_output, load_scene_intent,
    },
    style::StyleLock,
};
use kineto_prompts::{CompiledPrompt, DegradationLevel, PromptHash};
use kineto_provider_minimax::{MinimaxClient, PROVIDER as MINIMAX_PROVIDER, SharedTransport};
use kineto_providers::{
    AspectRatio, ImageCapabilities, ImageRequirements, Price, SeedSupport, evaluate_image,
    secret::{SecretError, SecretSource},
};

use crate::{
    documents::document_error,
    keyframe_prompt::{self, KeyframePrompt, KeyframePromptError, Locks},
    minimax::MinimaxKeyframes,
};

/// The capability a shot candidate is generated with.
pub const GENERATE_OPERATION: &str = "image.generate";

/// What a generated image belongs to, which decides where it is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateOwner {
    /// A keyframe candidate of this shot.
    Shot(ShotKey),
    /// A sample of a style direction.
    Style,
    /// A casting image of a character.
    Character { id: String, kind: CastImageKind },
}

/// The frame keyframes are generated in.
pub const KEYFRAME_ASPECT: AspectRatio = AspectRatio {
    width: 16,
    height: 9,
};

/// An approved picture of the person an image must show, sent to providers
/// that take an identity reference.
#[derive(Clone, PartialEq, Eq)]
pub struct IdentityReference {
    pub artifact_id: ArtifactId,
    pub bytes: Arc<[u8]>,
}

impl fmt::Debug for IdentityReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "IdentityReference({}, {} bytes)",
            self.artifact_id.as_str(),
            self.bytes.len()
        )
    }
}

/// One candidate to generate, as the adapter receives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRequest {
    pub artifact_id: ArtifactId,
    pub input_hash: InputHash,
    pub owner: CandidateOwner,
    pub aspect_ratio: AspectRatio,
    /// Approved pictures of the people the image must show, at most as many
    /// as the provider takes.
    pub identities: Vec<IdentityReference>,
    /// The compiled prompt, for generators that take one. Every candidate of
    /// a generation shares it.
    pub prompt: Option<Arc<KeyframePrompt>>,
}

/// A generated candidate together with the input it was generated for, so an
/// interrupted generation can be regrouped into its plan after a restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedCandidate {
    pub output: CandidateOutput,
    pub input_hash: InputHash,
}

/// A source of shot candidates behind the paid-job contract.
pub trait CandidateGenerator:
    PaidJobAdapter<Request = CandidateRequest, Output = GeneratedCandidate> + Clone + Send
{
    /// The provider key its jobs are recorded under.
    fn provider(&self) -> ProviderKey;

    fn policy(&self) -> JobRuntimePolicy;

    /// Whether candidates need a compiled prompt.
    fn needs_prompt(&self) -> bool;

    /// What the provider can preserve; prompt compilation reads this, never
    /// the provider's name. A generator without a provider declares nothing.
    fn capabilities(&self) -> ImageCapabilities {
        ImageCapabilities::default()
    }

    /// Whether new provider calls can be made. A generator that cannot may
    /// still complete calls that already finished.
    fn can_invoke(&self) -> bool {
        true
    }

    /// Whether completing an interrupted generation when a project opens may
    /// make new provider calls. Only a free generator may; a paid call is
    /// made only when the director asks for a generation.
    fn invokes_on_recovery(&self) -> bool {
        false
    }

    /// The result of a candidate whose provider call already finished,
    /// without calling the provider again.
    fn recover(&mut self, request: &CandidateRequest) -> Result<GeneratedCandidate, Self::Error>;

    /// Whether a candidate whose provider call was attempted but left no
    /// result may be called again without risking a second charge: its
    /// result is kept, or the provider certainly did not accept the attempt.
    /// A free generator always may.
    fn may_call_again(&self, _request: &CandidateRequest) -> bool {
        true
    }

    /// Whether a failed call certainly cost nothing: it never reached the
    /// provider, or the provider refused it. A free generator's never cost.
    fn certainly_not_charged(&self, _error: &Self::Error) -> bool {
        true
    }

    /// Forget runtime state kept for a candidate that is now acknowledged.
    fn release(&mut self, _request: &CandidateRequest) {}
}

/// The image provider a session generates keyframes with.
#[derive(Debug, Default)]
pub enum ImageProvider {
    /// Deterministic local candidates without media. Free.
    #[default]
    LocalDemo,
    /// MiniMax `image-01`. Paid. Without a client, finished calls can still
    /// be completed from their receipts but no new call is made.
    Minimax(Option<MinimaxClient<SharedTransport>>),
}

/// Which image provider a session generates with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageProviderKind {
    LocalDemo,
    Minimax,
}

/// What the director needs to know about the session's image provider before
/// asking for a generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageProviderInfo {
    pub kind: ImageProviderKind,
    /// Whether a generation can be made: a paid provider needs credentials.
    pub ready: bool,
    /// Estimated cost of one generation (every candidate), in USD micros.
    pub generation_cost_micros: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSelectionError {
    /// The selection names no provider this build has.
    Unknown(String),
    Secret(SecretError),
}

impl fmt::Display for ProviderSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown(name) => write!(formatter, "unknown image provider {name:?}"),
            Self::Secret(error) => error.fmt(formatter),
        }
    }
}

impl Error for ProviderSelectionError {}

impl ImageProvider {
    /// The provider named by `selection`: none or `local-demo` for the free
    /// demo generator, `minimax` for MiniMax with the key `secrets` holds.
    /// A paid provider is used only when it is named, never because a key
    /// happens to be present.
    pub fn select(
        selection: Option<&str>,
        secrets: &impl SecretSource,
    ) -> Result<Self, ProviderSelectionError> {
        match selection.map(str::trim) {
            None | Some("" | DEMO_PROVIDER) => Ok(Self::LocalDemo),
            Some(MINIMAX_PROVIDER) => Ok(Self::Minimax(
                secrets
                    .api_key(MINIMAX_PROVIDER)
                    .map_err(ProviderSelectionError::Secret)?
                    .map(MinimaxClient::shared),
            )),
            Some(other) => Err(ProviderSelectionError::Unknown(other.to_owned())),
        }
    }

    /// How the provider is given a character's identity, as a cast lock
    /// records it: the provider and the request field the reference travels
    /// in. `None` when it takes no identity reference.
    #[must_use]
    pub const fn identity_mechanism(&self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::LocalDemo => None,
            Self::Minimax(_) => Some((MINIMAX_PROVIDER, "subject_reference")),
        }
    }

    /// What the provider declares it can do; workflow code reads this, never
    /// the provider's name.
    #[must_use]
    pub fn capabilities(&self) -> ImageCapabilities {
        match self {
            Self::LocalDemo => local_demo_capabilities(),
            Self::Minimax(_) => kineto_provider_minimax::image_capabilities(),
        }
    }

    #[must_use]
    pub fn info(&self) -> ImageProviderInfo {
        let candidates =
            u64::try_from(GENERATED_CANDIDATE_COUNT).expect("candidate count fits in u64");
        let generation_cost_micros = self
            .capabilities()
            .price_per_image
            .map_or(0, |price| price.amount_micros * candidates);
        match self {
            Self::LocalDemo => ImageProviderInfo {
                kind: ImageProviderKind::LocalDemo,
                ready: true,
                generation_cost_micros,
            },
            Self::Minimax(client) => ImageProviderInfo {
                kind: ImageProviderKind::Minimax,
                ready: client.is_some(),
                generation_cost_micros,
            },
        }
    }
}

/// The local demo generator: no prompt, no references, an answer at once,
/// free.
#[must_use]
pub fn local_demo_capabilities() -> ImageCapabilities {
    ImageCapabilities {
        async_jobs: false,
        price_per_image: Some(Price::usd(0)),
        ..ImageCapabilities::default()
    }
}

/// Refuse a provider whose images arrive as remote jobs: candidates and
/// samples keep the image the provider answers with and follow no job.
pub(crate) fn require_synchronous(capabilities: &ImageCapabilities) -> Result<(), GenerationError> {
    let report = evaluate_image(
        ImageRequirements {
            required_reference_role: None,
            minimum_seed_support: SeedSupport::None,
            requires_negative_prompt: false,
            requires_mask: false,
            requires_synchronous: true,
        },
        capabilities,
    );
    if report.is_compatible() {
        Ok(())
    } else {
        Err(GenerationError::Prerequisite(
            Prerequisite::SynchronousImageProvider,
        ))
    }
}

/// Split `{operation}:{artifact_id}:{input_hash}`, as the job runtime derives
/// candidate keys, back into its candidate and input hash.
pub fn parse_candidate_key(key: &IdempotencyKey) -> Result<(ArtifactId, InputHash), KeyError> {
    parse_job_key(GENERATE_OPERATION, key)
}

/// Split `{operation}:{artifact_id}:{input_hash}`, as the job runtime derives
/// idempotency keys, back into its artifact and input hash.
pub fn parse_job_key(
    operation: &str,
    key: &IdempotencyKey,
) -> Result<(ArtifactId, InputHash), KeyError> {
    // Artifact IDs cannot contain ':', so the first separator after the
    // operation ends the artifact ID.
    let rest = key
        .as_str()
        .strip_prefix(operation)
        .and_then(|rest| rest.strip_prefix(':'))
        .ok_or(KeyError("idempotency key is for another operation"))?;
    let (artifact_id, input_hash) = rest
        .split_once(':')
        .ok_or(KeyError("idempotency key has no input hash"))?;
    Ok((
        ArtifactId::new(artifact_id)
            .map_err(|_| KeyError("idempotency key names an invalid artifact"))?,
        InputHash::new(input_hash)
            .map_err(|_| KeyError("idempotency key has an invalid input hash"))?,
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyError(&'static str);

impl fmt::Display for KeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for KeyError {}

/// The provenance an artifact records for a compiled keyframe prompt.
#[must_use]
pub fn provenance(prompt: &KeyframePrompt) -> PromptProvenance {
    provenance_of(&prompt.compiled, &prompt.prompt_hash)
}

/// The provenance an artifact records for a compiled prompt and its hash.
#[must_use]
pub fn provenance_of(compiled: &CompiledPrompt, prompt_hash: &PromptHash) -> PromptProvenance {
    PromptProvenance {
        compiler_version: compiled.compiler_version().to_owned(),
        compiled_prompt: compiled.prompt().to_owned(),
        prompt_hash: prompt_hash.as_str().to_owned(),
        applied_constraints: compiled
            .applied_constraints()
            .iter()
            .map(|constraint| constraint.as_str().to_owned())
            .collect(),
        dropped_constraints: compiled
            .dropped_constraints()
            .iter()
            .map(|dropped| DroppedConstraintRecord {
                constraint: dropped.constraint.as_str().to_owned(),
                reason: dropped.reason.clone(),
                degradation: match dropped.degradation {
                    DegradationLevel::None => Degradation::None,
                    DegradationLevel::Cosmetic => Degradation::Cosmetic,
                    DegradationLevel::IdentityAffecting => Degradation::IdentityAffecting,
                },
            })
            .collect(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDemoError(&'static str);

impl fmt::Display for LocalDemoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl Error for LocalDemoError {}

/// The deterministic local demo generator.
///
/// It keeps no remote state: its result for an idempotency key is a pure
/// function of the candidate and input hash the key names. Reconciling by key
/// therefore recomputes the result instead of looking anything up, and never
/// reports a job as missing.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalDemoAdapter;

impl LocalDemoAdapter {
    fn generated(
        artifact_id: &ArtifactId,
        input_hash: &InputHash,
    ) -> Result<GeneratedCandidate, LocalDemoError> {
        let output = demo_candidate_output(artifact_id)
            .map_err(|_| LocalDemoError("candidate cannot be generated"))?;
        Ok(GeneratedCandidate {
            output,
            input_hash: input_hash.clone(),
        })
    }
}

impl PaidJobAdapter for LocalDemoAdapter {
    type Request = CandidateRequest;
    type Output = GeneratedCandidate;
    type Error = LocalDemoError;

    fn estimate_cost(&self, _request: &CandidateRequest) -> Result<CostEstimate, LocalDemoError> {
        Ok(CostEstimate {
            call_count: 1,
            amount_micros: 0,
            currency: CurrencyCode::new(*b"USD").expect("static currency code is valid"),
            estimated_duration_ms: Some(0),
        })
    }

    fn invoke(
        &mut self,
        request: &CandidateRequest,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<GeneratedCandidate>, LocalDemoError> {
        Self::generated(&request.artifact_id, &request.input_hash).map(Invocation::Completed)
    }

    fn reconcile(
        &mut self,
        _provider_job_id: &ProviderJobId,
    ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
        // The demo generator never hands out remote job handles.
        Ok(Reconciliation::NotFound)
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
        let (artifact_id, input_hash) =
            parse_candidate_key(idempotency_key).map_err(|error| LocalDemoError(error.0))?;
        Ok(Reconciliation::Completed {
            output: Self::generated(&artifact_id, &input_hash)?,
            provider_job_id: None,
        })
    }

    fn classify_error(&self, _error: &LocalDemoError) -> ErrorClass {
        // A deterministic local computation that failed once fails again.
        ErrorClass::Terminal
    }
}

impl CandidateGenerator for LocalDemoAdapter {
    fn provider(&self) -> ProviderKey {
        ProviderKey::new(DEMO_PROVIDER).expect("static provider key is valid")
    }

    fn policy(&self) -> JobRuntimePolicy {
        JobRuntimePolicy {
            // A local deterministic computation has no provider-side
            // concurrency limit.
            execution: ProviderExecutionPolicy::new(
                NonZeroU16::MAX,
                BackoffPolicy::new(0, 0, 3).expect("static backoff policy is valid"),
            ),
            // The demo generator is free, and no paid fallback exists for it.
            fallback_policy: FallbackPolicy {
                paid_fallback: PaidFallback::Forbidden,
                max_paid_amount_micros: Some(0),
                require_user_approval: true,
            },
        }
    }

    fn needs_prompt(&self) -> bool {
        false
    }

    fn capabilities(&self) -> ImageCapabilities {
        local_demo_capabilities()
    }

    fn invokes_on_recovery(&self) -> bool {
        true
    }

    fn recover(
        &mut self,
        request: &CandidateRequest,
    ) -> Result<GeneratedCandidate, LocalDemoError> {
        Self::generated(&request.artifact_id, &request.input_hash)
    }
}

/// A step a generation passes durably. Tests stop the process at these points
/// to prove recovery; production passes [`NoHooks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenerationPoint {
    /// The write-ahead intent for candidate `index` is on disk.
    Prepared { index: usize },
    /// Every candidate has a result and no canonical state has changed.
    Invoked,
    /// The generation is in canonical state; the intents are not yet
    /// acknowledged.
    Attached,
}

pub trait GenerationHooks {
    fn reached(&mut self, point: GenerationPoint);
}

pub struct NoHooks;

impl GenerationHooks for NoHooks {
    fn reached(&mut self, _point: GenerationPoint) {}
}

#[derive(Debug)]
pub enum GenerationError {
    Shot(ShotWorkflowError),
    Runtime(JobRuntimeError),
    /// The shot's intent could not be compiled into a prompt.
    Prompt(KeyframePromptError),
    /// The provider refused or failed. `class` says whether trying again can
    /// succeed.
    Provider {
        source: Box<dyn Error + Send + Sync>,
        class: ErrorClass,
    },
    /// The chosen provider has no credentials, so no new call can be made.
    NotConfigured,
    /// Completing the generation needs a paid provider call, which is made
    /// only when the director asks for a generation again.
    NeedsProviderCall,
    /// The locked keyframe has no image to start a video from (a demo
    /// keyframe).
    NoFirstFrame,
    /// The provider cannot do what the generation needs, named by
    /// capability, such as `video.start_frame`.
    Incapable(Vec<&'static str>),
    /// What this is made from does not exist yet.
    Prerequisite(Prerequisite),
    /// No language model is configured, so no document can be asked for.
    ModelNotConfigured,
    /// The last paid submission may have been accepted but left no record of
    /// it, so submitting again may pay twice. Only the director decides that.
    UnresolvedSubmission,
    /// The keyframe would lose these identity constraints, and the project's
    /// policy asks for the director's consent first.
    IdentityDrift(Vec<String>),
    JobId(JobValueError),
    /// The provider reported something the generation path never expects,
    /// such as a pending remote job for a synchronous image call.
    Unexpected(&'static str),
}

impl fmt::Display for GenerationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shot(error) => error.fmt(formatter),
            Self::Runtime(error) => error.fmt(formatter),
            Self::Prompt(error) => error.fmt(formatter),
            Self::Provider { source, .. } => source.fmt(formatter),
            Self::NotConfigured => {
                formatter.write_str("the image provider has no credentials configured")
            }
            Self::NoFirstFrame => {
                formatter.write_str("the locked keyframe has no image to start a video from")
            }
            Self::Incapable(missing) => write!(
                formatter,
                "the provider cannot do what this generation needs: {}",
                missing.join(", ")
            ),
            Self::UnresolvedSubmission => formatter.write_str(
                "the last submission may already have been accepted and charged; submitting \
                 again may pay twice",
            ),
            Self::IdentityDrift(constraints) => write!(
                formatter,
                "the keyframe would show {} without a reference and they could look different \
                 in every take; the project asks for the director's consent first",
                constraints.join(", ")
            ),
            Self::NeedsProviderCall => formatter.write_str(
                "finishing this generation needs a paid provider call; generate again to make it",
            ),
            Self::JobId(error) => error.fmt(formatter),
            Self::Prerequisite(missing) => missing.fmt(formatter),
            Self::ModelNotConfigured => {
                formatter.write_str("no language model is configured for documents")
            }
            Self::Unexpected(message) => formatter.write_str(message),
        }
    }
}

impl Error for GenerationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Shot(error) => Some(error),
            Self::Runtime(error) => Some(error),
            Self::Prompt(error) => Some(error),
            Self::Provider { source, .. } => Some(source.as_ref()),
            Self::JobId(error) => Some(error),
            Self::NotConfigured
            | Self::NeedsProviderCall
            | Self::NoFirstFrame
            | Self::Incapable(_)
            | Self::Prerequisite(_)
            | Self::ModelNotConfigured
            | Self::UnresolvedSubmission
            | Self::IdentityDrift(_)
            | Self::Unexpected(_) => None,
        }
    }
}

/// What has to exist before a document can be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prerequisite {
    /// A screenplay is written from a story analysis.
    StoryAnalysis,
    /// A scene is rewritten within a screenplay.
    Screenplay,
    /// The screenplay has no scene with the ID asked for.
    Scene,
    /// The bible is written from the locked screenplay.
    LockedScreenplay,
    /// Style samples need a real image provider, not the local demo.
    RealImageProvider,
    /// There is no style preset with the ID asked for.
    StylePreset,
    /// There is no style sample with the ID asked for.
    StyleSample,
    /// The image provider delivers through remote jobs, which keyframes and
    /// style samples do not follow.
    SynchronousImageProvider,
    /// Casting is done from the bible.
    Bible,
    /// The bible has no character with the ID asked for.
    Character,
    /// Casting follows the locked style.
    LockedStyle,
    /// A reference pack is made from the selected portrait.
    SelectedPortrait,
    /// The image provider takes no identity reference, or not this portrait.
    IdentityReference,
    /// Shots are planned from the locked bible as well as the screenplay.
    LockedBible,
    /// A scene is planned again within a shot plan.
    ShotPlan,
    /// A character's voice is cast after its look is locked.
    LockedCast,
    /// A line is dubbed in its character's locked voice.
    LockedVoice,
    /// The scene has no line with the number asked for, or no one says it.
    Line,
    /// Suggestions are drawn from the director's choices; none is made yet.
    Choices,
}

impl fmt::Display for Prerequisite {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::StoryAnalysis => "analyse the story before writing the screenplay",
            Self::Screenplay => "write the screenplay before rewriting a scene",
            Self::Scene => "the screenplay has no scene with that id",
            Self::LockedScreenplay => "lock the screenplay first",
            Self::RealImageProvider => "style samples need an image provider such as MiniMax",
            Self::StylePreset => "there is no style preset with that id",
            Self::StyleSample => "there is no style sample with that id",
            Self::SynchronousImageProvider => {
                "the image provider delivers images as remote jobs, which this step does not follow"
            }
            Self::Bible => "write the bible first",
            Self::Character => "the bible has no character with that id",
            Self::LockedStyle => "lock a style first",
            Self::SelectedPortrait => "select a portrait first",
            Self::IdentityReference => {
                "the image provider takes no identity reference, or not this portrait"
            }
            Self::LockedBible => "lock the bible first",
            Self::ShotPlan => "plan the shots before planning a scene again",
            Self::LockedCast => "lock the character's cast before casting its voice",
            Self::LockedVoice => "lock the character's voice before dubbing its lines",
            Self::Line => "the scene has no line with that number spoken by a character",
            Self::Choices => "choose among some takes before asking what they show",
        })
    }
}

impl From<ShotWorkflowError> for GenerationError {
    fn from(error: ShotWorkflowError) -> Self {
        Self::Shot(error)
    }
}

impl From<JobRuntimeError> for GenerationError {
    fn from(error: JobRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl From<JobValueError> for GenerationError {
    fn from(error: JobValueError) -> Self {
        Self::JobId(error)
    }
}

impl From<KeyframePromptError> for GenerationError {
    fn from(error: KeyframePromptError) -> Self {
        Self::Prompt(error)
    }
}

fn provider_error<G: CandidateGenerator>(generator: &G, source: G::Error) -> GenerationError {
    GenerationError::Provider {
        class: generator.classify_error(&source),
        source: Box::new(source),
    }
}

/// Generate the shot's next candidates as write-ahead jobs.
pub fn generate(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    provider: &ImageProvider,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => generate_with(project, shot, LocalDemoAdapter, hooks),
        ImageProvider::Minimax(client) => generate_with(
            project,
            shot,
            MinimaxKeyframes::new(project, client.as_ref()),
            hooks,
        ),
    }
}

/// Generate the shot's candidates again although a paid call of the last
/// attempt may have been accepted, and charged for, without leaving its
/// result. Only on the director's explicit decision to risk paying twice.
pub fn resubmit(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    provider: &ImageProvider,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => generate_consenting(
            project,
            shot,
            LocalDemoAdapter,
            hooks,
            Consent::RISK_PAYING_TWICE,
        ),
        ImageProvider::Minimax(client) => generate_consenting(
            project,
            shot,
            MinimaxKeyframes::new(project, client.as_ref()),
            hooks,
            Consent::RISK_PAYING_TWICE,
        ),
    }
}

/// What the director has explicitly accepted for one generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Consent {
    /// The last paid call may have been charged without its result; ask
    /// again anyway.
    pub risk_paying_twice: bool,
    /// Characters may be shown without an identity reference, though the
    /// project's policy blocks that.
    pub identity_drift: bool,
}

impl Consent {
    pub const RISK_PAYING_TWICE: Self = Self {
        risk_paying_twice: true,
        identity_drift: false,
    };
}

/// What a generation does when a keyframe would show a character without an
/// identity reference.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IdentityPolicy {
    /// Generate, and report it on the candidates.
    #[default]
    Report,
    /// Refuse until the director consents for the generation.
    Block,
}

/// The project's identity policy: `identity_policy = "block"` under
/// `[workflow]` in `project.toml` blocks; anything else reports.
#[must_use]
pub fn identity_policy(project: &CanonicalProject) -> IdentityPolicy {
    match project
        .manifest()
        .workflow
        .extra
        .get("identity_policy")
        .and_then(|value| value.as_str())
    {
        Some("block") => IdentityPolicy::Block,
        _ => IdentityPolicy::Report,
    }
}

/// [`generate`] with what the director has consented to.
pub fn generate_with_consent(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    provider: &ImageProvider,
    hooks: &mut dyn GenerationHooks,
    consent: Consent,
) -> Result<AttachOutcome, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => {
            generate_consenting(project, shot, LocalDemoAdapter, hooks, consent)
        }
        ImageProvider::Minimax(client) => generate_consenting(
            project,
            shot,
            MinimaxKeyframes::new(project, client.as_ref()),
            hooks,
            consent,
        ),
    }
}

/// [`generate`] with an explicit generator.
pub fn generate_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    generator: G,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    generate_consenting(project, shot, generator, hooks, Consent::default())
}

/// [`resubmit`] with an explicit generator.
pub fn resubmit_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    generator: G,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    generate_consenting(project, shot, generator, hooks, Consent::RISK_PAYING_TWICE)
}

/// [`generate_with_consent`] with an explicit generator.
pub fn generate_consenting<G: CandidateGenerator>(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    generator: G,
    hooks: &mut dyn GenerationHooks,
    consent: Consent,
) -> Result<AttachOutcome, GenerationError> {
    generate_noted(project, shot, generator, hooks, consent, None)
}

/// Generate the shot's candidates again as the director's `note` asks,
/// such as "warmer light, tighter on his hands". The note reaches the
/// prompt as `director.note` for this generation only; everything else is
/// as [`generate_with_consent`].
pub fn refine(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    provider: &ImageProvider,
    hooks: &mut dyn GenerationHooks,
    consent: Consent,
    note: &str,
) -> Result<AttachOutcome, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => {
            generate_noted(project, shot, LocalDemoAdapter, hooks, consent, Some(note))
        }
        ImageProvider::Minimax(client) => generate_noted(
            project,
            shot,
            MinimaxKeyframes::new(project, client.as_ref()),
            hooks,
            consent,
            Some(note),
        ),
    }
}

/// [`generate_consenting`] with the director's note for this generation.
pub fn generate_noted<G: CandidateGenerator>(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    generator: G,
    hooks: &mut dyn GenerationHooks,
    consent: Consent,
    note: Option<&str>,
) -> Result<AttachOutcome, GenerationError> {
    let risk_paying_twice = consent.risk_paying_twice;
    shot.plan_generation(project)?;
    // Refuse before writing anything: a provider that cannot be called or a
    // shot that cannot be compiled must not leave intents behind.
    if !generator.can_invoke() {
        return Err(GenerationError::NotConfigured);
    }
    require_synchronous(&generator.capabilities())?;
    let prompt = compile_prompt(project, shot, &generator, note)?;
    if let Some(prompt) = &prompt
        && !consent.identity_drift
        && identity_policy(project) == IdentityPolicy::Block
    {
        let lost: Vec<String> = prompt
            .compiled
            .dropped_constraints()
            .iter()
            .filter(|dropped| dropped.degradation == DegradationLevel::IdentityAffecting)
            .map(|dropped| dropped.constraint.as_str().to_owned())
            .collect();
        if !lost.is_empty() {
            return Err(GenerationError::IdentityDrift(lost));
        }
    }
    // The candidates depend on the locks their prompt follows.
    let plan = shot.plan_generation_from(
        project,
        prompt
            .as_ref()
            .map_or(&[][..], |prompt| prompt.dependencies.as_slice()),
    )?;
    let runtime = JobRuntime::new(project, generator.provider(), generator.policy());
    // A candidate whose last call may have been accepted without leaving its
    // result is never called again unasked: that could pay twice. Checked
    // for every candidate before anything is written or called.
    let mut unresolved = Vec::new();
    for candidate in plan.candidate_ids() {
        let job_id = JobId::new(candidate.as_str())?;
        if let Some(record) = runtime.load(&job_id)?
            && record.intent().state() == IntentState::Prepared
            && record.attempts() > 0
            && !generator.may_call_again(&CandidateRequest {
                artifact_id: candidate.clone(),
                input_hash: record.intent().input_hash().clone(),
                owner: CandidateOwner::Shot(shot.key().clone()),
                aspect_ratio: KEYFRAME_ASPECT,
                identities: Vec::new(),
                prompt: None,
            })
        {
            unresolved.push(job_id);
        }
    }
    if !unresolved.is_empty() && !risk_paying_twice {
        return Err(GenerationError::UnresolvedSubmission);
    }
    for job_id in &unresolved {
        runtime.retire_unresolved(job_id)?;
    }
    drop(runtime);
    complete_inner(project, shot, &plan, generator, Some(prompt), hooks)
}

/// Drive a planned generation to the end from whatever it reached durably.
pub fn complete(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    plan: &GenerationPlan,
    provider: &ImageProvider,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    match provider {
        ImageProvider::LocalDemo => complete_with(project, shot, plan, LocalDemoAdapter, hooks),
        ImageProvider::Minimax(client) => complete_with(
            project,
            shot,
            plan,
            MinimaxKeyframes::new(project, client.as_ref()),
            hooks,
        ),
    }
}

/// [`complete`] with an explicit generator.
pub fn complete_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    plan: &GenerationPlan,
    generator: G,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    // Recovery makes no paid call. Candidates that finished keep the prompt
    // they were generated from; a free generator compiles one if needed.
    complete_inner(project, shot, plan, generator, None, hooks)
}

fn compile_prompt<G: CandidateGenerator>(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
    generator: &G,
    note: Option<&str>,
) -> Result<Option<Arc<KeyframePrompt>>, GenerationError> {
    if !generator.needs_prompt() {
        return Ok(None);
    }
    let scene = load_scene_intent(project, shot.key().scene())?;
    let intent = shot.intent()?;
    let (style, characters) = locks_of(project, intent.as_ref())?;
    let bible = locked_bible(project)?;
    let preferences = crate::preferences::in_force(project)?;
    Ok(Some(Arc::new(keyframe_prompt::compile(
        scene.as_ref(),
        intent.as_ref(),
        shot.direction(),
        Locks {
            style: style.as_ref(),
            characters: &characters,
            bible: bible.as_ref(),
            note,
            preferences: preferences.as_ref(),
        },
        &generator.capabilities(),
    )?)))
}

/// The locked bible, which keyframes read continuity from; `None` when no
/// bible is locked.
fn locked_bible(project: &CanonicalProject) -> Result<Option<Bible>, GenerationError> {
    let Some(record) = revisions::revisions(project, DocumentKind::Bible)
        .map_err(document_error)?
        .into_iter()
        .find(|record| record.status == RevisionStatus::Locked)
    else {
        return Ok(None);
    };
    revisions::read(project, &record)
        .map(Some)
        .map_err(document_error)
}

/// The locked style and the locked casts of the shot's characters.
fn locks_of(
    project: &CanonicalProject,
    intent: Option<&ShotIntent>,
) -> Result<(Option<StyleLock>, Vec<CharacterLock>), GenerationError> {
    let style = kineto_project::style::locked(project).map_err(document_error)?;
    let mut characters = Vec::new();
    for character in intent.map_or(&[][..], |intent| intent.characters.as_slice()) {
        match cast::locked(project, character) {
            Ok(Some(lock)) => characters.push(lock),
            // An ID that cannot name a cast is never cast.
            Ok(None) | Err(CastError::InvalidCharacter(_)) => {}
            Err(error) => return Err(document_error(error)),
        }
    }
    Ok((style, characters))
}

/// The canonical portrait `locked` names, read and checked against its
/// hash, to send as the identity reference.
fn identity_reference(
    project: &CanonicalProject,
    locked: &LockedImage,
) -> Result<IdentityReference, GenerationError> {
    let path = ProjectRelativePath::new(&locked.media_path).map_err(document_error)?;
    let bytes = project.root().read(&path).map_err(document_error)?;
    if ContentHash::of(&bytes).as_str() != locked.content_hash {
        return Err(document_error(CastError::Mismatch(
            locked.media_path.clone(),
        )));
    }
    Ok(IdentityReference {
        artifact_id: ArtifactId::new(locked.artifact_id.clone())
            .map_err(|_| GenerationError::Unexpected("a locked portrait has an invalid ID"))?,
        bytes: Arc::from(bytes),
    })
}

/// What makes each current candidate of `shot` stale, in candidate order:
/// each intent field changed since it was made (`intent.<field>`), each
/// lock or bible entry its prompt followed that changed or went, and each
/// lock made since that it did not follow, by artifact ID. Empty for a
/// fresh candidate; a demo candidate follows no lock.
pub fn keyframe_staleness(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
) -> Result<Vec<Vec<String>>, GenerationError> {
    let intent = shot.intent()?;
    let (style, characters) = locks_of(project, intent.as_ref())?;
    let mut current: Vec<_> = style
        .as_ref()
        .and_then(kineto_project::style::lock_dependency)
        .into_iter()
        .collect();
    current.extend(characters.iter().filter_map(cast::lock_dependency));
    let scene = load_scene_intent(project, shot.key().scene())?;
    current.extend(keyframe_prompt::continuity_dependencies(
        scene.as_ref(),
        intent.as_ref(),
        locked_bible(project)?.as_ref(),
    ));
    let intent_changes = shot.candidate_intent_changes()?;
    Ok(shot
        .candidate_dependencies()?
        .iter()
        .zip(shot.candidate_origins()?)
        .zip(intent_changes)
        .map(|((recorded, origin), mut stale)| {
            // A demo candidate follows no lock, but it was made for an intent.
            if origin.provider != DEMO_PROVIDER {
                // Only what the keyframe was made from can make it stale;
                // the preferences it was made with cannot.
                let recorded: Vec<DependencyRecord> = recorded
                    .iter()
                    .filter(|dependency| dependency.impact == DependencyImpact::Semantic)
                    .map(DependencyRecord::from)
                    .collect();
                stale.extend(cast::stale_parents(&recorded, &current));
            }
            stale
        })
        .collect())
}

/// For each current keyframe candidate of `shot`, whether it was made with
/// preferences other than those in force. Never a reason for staleness;
/// it says what a candidate not yet chosen was asked for.
pub fn preferences_changed(
    project: &CanonicalProject,
    shot: &ShotWorkflow,
) -> Result<Vec<bool>, GenerationError> {
    let in_force = crate::preferences::in_force(project)?;
    Ok(shot
        .candidate_dependencies()?
        .iter()
        .zip(shot.candidate_origins()?)
        .map(|(recorded, origin)| {
            origin.provider != DEMO_PROVIDER
                && kineto_project::preferences::changed_since(
                    recorded,
                    in_force.as_ref().map(|in_force| &in_force.record),
                )
        })
        .collect())
}

/// Lock the shot's selected keyframe as production truth, unless a lock its
/// prompt followed has changed or gone since, or a lock it did not follow
/// has been made: the shot alone checks only its own intent.
pub fn lock(project: &CanonicalProject, shot: &mut ShotWorkflow) -> Result<(), GenerationError> {
    let snapshot = shot.snapshot()?;
    if !snapshot.locked
        && let Some(selected) = snapshot.selected_index
        && keyframe_staleness(project, shot)?
            .get(usize::from(selected))
            .is_some_and(|causes| !causes.is_empty())
    {
        return Err(ShotWorkflowError::StaleSelection.into());
    }
    Ok(shot.lock(project)?)
}

/// `requested` holds the compiled prompt of a generation the director asked
/// for, and is `None` when an interrupted generation is being recovered.
fn complete_inner<G: CandidateGenerator>(
    project: &CanonicalProject,
    shot: &mut ShotWorkflow,
    plan: &GenerationPlan,
    mut generator: G,
    requested: Option<Option<Arc<KeyframePrompt>>>,
    hooks: &mut dyn GenerationHooks,
) -> Result<AttachOutcome, GenerationError> {
    let runtime = JobRuntime::new(project, generator.provider(), generator.policy());
    let mut requests = Vec::with_capacity(plan.candidate_ids().len());
    let mut states = Vec::with_capacity(plan.candidate_ids().len());
    let recovering = requested.is_none();
    let mut prompt = requested.flatten();

    for (index, candidate) in plan.candidate_ids().iter().enumerate() {
        let job_id = JobId::new(candidate.as_str())?;
        if !recovering {
            // Asking again is what retries a candidate whose call failed: its
            // failed intent would otherwise refuse every new attempt.
            runtime.retire_failed(&job_id)?;
        }
        let state = match runtime.prepare(
            job_id.clone(),
            GENERATE_OPERATION,
            plan.input_hash().clone(),
            candidate.clone(),
        ) {
            Ok(PrepareOutcome::Created(record) | PrepareOutcome::Existing(record)) => {
                Some(record.intent().state())
            }
            // Acknowledged long ago and aged out of retention: already attached.
            Err(JobRuntimeError::ReconciledIntentPruned) => None,
            Err(error) => return Err(error.into()),
        };
        hooks.reached(GenerationPoint::Prepared { index });
        requests.push((
            job_id,
            CandidateRequest {
                artifact_id: candidate.clone(),
                input_hash: plan.input_hash().clone(),
                owner: CandidateOwner::Shot(shot.key().clone()),
                aspect_ratio: KEYFRAME_ASPECT,
                identities: Vec::new(),
                prompt: None,
            },
        ));
        states.push(state);
    }

    if shot.is_attached(plan) {
        // Stopped after attaching: only the acknowledgements are missing. An
        // attached candidate's intent can only be invoked or already gone.
        // Before acknowledging, prove canonical state holds the results the
        // provider returned; an edited or corrupted record is not one.
        let mut outputs = Vec::new();
        for ((_, request), state) in requests.iter().zip(&states) {
            if *state == Some(IntentState::Invoked) {
                let output = generator
                    .recover(request)
                    .map_err(|source| provider_error(&generator, source))?;
                outputs.push(output.output);
            }
        }
        shot.check_attached(plan, &outputs)?;
        let invoked = states
            .iter()
            .map(|state| matches!(state, Some(IntentState::Invoked | IntentState::Reconciled)))
            .collect::<Vec<_>>();
        acknowledge(&runtime, &mut generator, &requests, &invoked)?;
        return Ok(AttachOutcome::AlreadyAttached);
    }

    let to_invoke = states
        .iter()
        .filter(|state| **state == Some(IntentState::Prepared))
        .count();
    if to_invoke > 0 {
        if recovering && !generator.invokes_on_recovery() {
            return Err(GenerationError::NeedsProviderCall);
        }
        if !generator.can_invoke() {
            return Err(GenerationError::NotConfigured);
        }
        if prompt.is_none() {
            prompt = compile_prompt(project, shot, &generator, None)?;
        }
    }
    let identities = match &prompt {
        Some(prompt) if to_invoke > 0 => prompt
            .identities
            .iter()
            .map(|locked| identity_reference(project, locked))
            .collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    for (_, request) in &mut requests {
        request.prompt.clone_from(&prompt);
        request.identities.clone_from(&identities);
    }

    // Every provider call runs on its own thread; recovering a finished call
    // reads only local state.
    let results = thread::scope(|scope| {
        let runtime = &runtime;
        let handles = requests
            .iter()
            .zip(&states)
            .map(|((job_id, request), state)| {
                let mut generator = generator.clone();
                let invoke = *state == Some(IntentState::Prepared);
                scope.spawn(move || {
                    if !invoke {
                        return generator
                            .recover(request)
                            .map_err(|source| provider_error(&generator, source));
                    }
                    match invoke_when_permitted(runtime, job_id, &mut generator, request) {
                        Ok(DispatchOutcome::Completed { output, .. }) => Ok(output),
                        Ok(DispatchOutcome::Pending { .. }) => Err(GenerationError::Unexpected(
                            "an image provider returned a pending job",
                        )),
                        Err(InvokePreparedError::Runtime(error)) => Err(error.into()),
                        // A failure that may have reached the provider may
                        // have been charged.
                        Err(InvokePreparedError::Provider { source, .. })
                            if !generator.certainly_not_charged(&source) =>
                        {
                            Err(GenerationError::UnresolvedSubmission)
                        }
                        Err(InvokePreparedError::Provider { source, class, .. }) => {
                            Err(GenerationError::Provider {
                                source: Box::new(source),
                                class,
                            })
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or(Err(GenerationError::Unexpected("a provider call panicked")))
            })
            .collect::<Vec<_>>()
    });
    // Candidates that finished stay invoked with their results kept; the
    // next open completes the generation without paying for them again.
    if results
        .iter()
        .any(|result| matches!(result, Err(GenerationError::UnresolvedSubmission)))
    {
        return Err(GenerationError::UnresolvedSubmission);
    }
    let outputs = results
        .into_iter()
        .map(|result| result.map(|generated| generated.output))
        .collect::<Result<Vec<_>, _>>()?;
    hooks.reached(GenerationPoint::Invoked);

    let outcome = shot.attach_generation(project, plan, &outputs)?;
    hooks.reached(GenerationPoint::Attached);

    let invoked = states.iter().map(Option::is_some).collect::<Vec<_>>();
    acknowledge(&runtime, &mut generator, &requests, &invoked)?;
    Ok(outcome)
}

/// How long a candidate waits for a provider permit before giving up.
const PERMIT_WAIT: Duration = Duration::from_secs(600);
const PERMIT_POLL: Duration = Duration::from_millis(200);

/// Invoke a prepared candidate, waiting while the provider's process-wide
/// concurrency ceiling is reached. The runtime refuses rather than queues, and
/// a refusal spends no attempt, so waiting here is the scheduler.
pub(crate) fn invoke_when_permitted<A: PaidJobAdapter>(
    runtime: &JobRuntime<'_>,
    job_id: &JobId,
    adapter: &mut A,
    request: &A::Request,
) -> Result<DispatchOutcome<A::Output>, InvokePreparedError<A::Error>> {
    let deadline = Instant::now() + PERMIT_WAIT;
    loop {
        match runtime.invoke_prepared(job_id, adapter, request) {
            Err(InvokePreparedError::Runtime(JobRuntimeError::ConcurrencyLimitReached))
                if Instant::now() < deadline =>
            {
                thread::sleep(PERMIT_POLL);
            }
            outcome => return outcome,
        }
    }
}

/// Acknowledge every candidate whose intent is invoked, now that the
/// generation is in canonical state.
fn acknowledge<G: CandidateGenerator>(
    runtime: &JobRuntime<'_>,
    generator: &mut G,
    requests: &[(JobId, CandidateRequest)],
    invoked: &[bool],
) -> Result<(), GenerationError> {
    for ((job_id, request), invoked) in requests.iter().zip(invoked) {
        if *invoked {
            // Retention warnings are housekeeping after the durable
            // acknowledgement and do not change the result.
            let _ = runtime.acknowledge_result(job_id)?;
        }
        generator.release(request);
    }
    Ok(())
}

/// Unfinished generations found in a project's job intents.
#[derive(Debug, Default)]
pub struct Unfinished {
    /// One plan per interrupted generation, each ready for [`complete`].
    pub plans: Vec<GenerationPlan>,
    /// Intents this build cannot read or cannot place; they are left alone and
    /// need a person to look at them.
    pub needs_attention: usize,
    /// Shots with a candidate whose paid call may have been accepted without
    /// leaving its result; see [`resubmit`].
    pub unresolved: Vec<ShotKey>,
}

/// Find generations that stopped before they were acknowledged.
///
/// Runs the job runtime's startup reconciliation for the provider, then
/// regroups the candidates it reports into the plans they belong to.
pub fn unfinished(
    project: &CanonicalProject,
    provider: &ImageProvider,
) -> Result<Unfinished, JobRuntimeError> {
    match provider {
        ImageProvider::LocalDemo => unfinished_with(project, LocalDemoAdapter),
        ImageProvider::Minimax(client) => {
            unfinished_with(project, MinimaxKeyframes::new(project, client.as_ref()))
        }
    }
}

/// [`unfinished`] with an explicit generator.
pub fn unfinished_with<G: CandidateGenerator>(
    project: &CanonicalProject,
    mut generator: G,
) -> Result<Unfinished, JobRuntimeError> {
    let runtime = JobRuntime::new(project, generator.provider(), generator.policy());
    let mut unfinished = Unfinished::default();
    let mut awaiting_call = Vec::new();
    for outcome in runtime.reconcile_startup(&mut generator)? {
        match outcome {
            StartupOutcome::ResultReady { job_id, output } => {
                let plan = ArtifactId::new(job_id.as_str()).ok().and_then(|candidate| {
                    GenerationPlan::for_candidate(&candidate, output.input_hash).ok()
                });
                match plan {
                    Some(plan) if !unfinished.plans.contains(&plan) => unfinished.plans.push(plan),
                    Some(_) => {}
                    None => unfinished.needs_attention += 1,
                }
            }
            // A candidate whose call never finished waits for the director to
            // generate again; recovery does not pay for it.
            StartupOutcome::RetryPrepared { job_id, .. } => awaiting_call.push(job_id),
            StartupOutcome::StillPending { .. }
            | StartupOutcome::NeedsAttention { .. }
            | StartupOutcome::UnreadableIntent { .. }
            | StartupOutcome::ProviderError { .. } => unfinished.needs_attention += 1,
        }
    }
    for job_id in &awaiting_call {
        let Some(record) = runtime.load(job_id)? else {
            continue;
        };
        let Ok(candidate) = ArtifactId::new(job_id.as_str()) else {
            unfinished.needs_attention += 1;
            continue;
        };
        let input_hash = record.intent().input_hash().clone();
        let Ok(plan) = GenerationPlan::for_candidate(&candidate, input_hash.clone()) else {
            unfinished.needs_attention += 1;
            continue;
        };
        let Ok(Some(key)) = shot::locate(project, plan.shot_id()) else {
            unfinished.needs_attention += 1;
            continue;
        };
        if record.attempts() > 0
            && !generator.may_call_again(&CandidateRequest {
                artifact_id: candidate,
                input_hash,
                owner: CandidateOwner::Shot(key.clone()),
                aspect_ratio: KEYFRAME_ASPECT,
                identities: Vec::new(),
                prompt: None,
            })
            && !unfinished.unresolved.contains(&key)
        {
            unfinished.unresolved.push(key);
        }
    }
    if !generator.invokes_on_recovery() {
        unfinished.plans.retain(|plan| {
            !plan.candidate_ids().iter().any(|candidate| {
                awaiting_call
                    .iter()
                    .any(|job| job.as_str() == candidate.as_str())
            })
        });
    }
    unfinished.plans.sort_by(|left, right| {
        (left.shot_id(), left.revision()).cmp(&(right.shot_id(), right.revision()))
    });
    Ok(unfinished)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

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
                "kineto-generation-test-{}-{nonce}",
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

    fn project(target: &Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "generation_test".to_owned(),
                title: "Generation Test".to_owned(),
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
            b"source\n",
        )
        .unwrap()
    }

    fn intent_states(target: &Path) -> Vec<String> {
        let mut states = fs::read_dir(target.join(".kineto/jobs/intents"))
            .map(|entries| {
                entries
                    .map(|entry| {
                        let bytes = fs::read(entry.unwrap().path()).unwrap();
                        let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                        value["state"].as_str().unwrap().to_owned()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        states.sort();
        states
    }

    /// Stops a generation at one point, as a crash there would: nothing after
    /// the point runs and nothing is rolled back.
    struct StopAt(GenerationPoint);

    impl GenerationHooks for StopAt {
        fn reached(&mut self, point: GenerationPoint) {
            if point == self.0 {
                panic!("stopped at {point:?}");
            }
        }
    }

    fn stop_at(target: &Path, point: GenerationPoint) {
        let project = project(target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            generate(
                &project,
                &mut shot,
                &ImageProvider::LocalDemo,
                &mut StopAt(point),
            )
        }));
        assert!(stopped.is_err(), "generation should stop at {point:?}");
    }

    fn recover(target: &Path) -> (usize, AttachOutcome) {
        let project = CanonicalProject::open(target).unwrap();
        let found = unfinished(&project, &ImageProvider::LocalDemo).unwrap();
        assert_eq!(found.needs_attention, 0);
        assert_eq!(found.plans.len(), 1, "exactly one interrupted generation");
        let plan = &found.plans[0];
        let mut shot = ShotWorkflow::load(
            &project,
            &shot::locate(&project, plan.shot_id()).unwrap().unwrap(),
        )
        .unwrap();
        let outcome = complete(
            &project,
            &mut shot,
            plan,
            &ImageProvider::LocalDemo,
            &mut NoHooks,
        )
        .unwrap();
        assert!(
            unfinished(&project, &ImageProvider::LocalDemo)
                .unwrap()
                .plans
                .is_empty()
        );
        (found.plans.len(), outcome)
    }

    fn assert_one_generation(target: &Path) {
        let project = CanonicalProject::open(target).unwrap();
        let snapshot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1))
            .unwrap()
            .snapshot()
            .unwrap();
        assert!(snapshot.generated);
        assert_eq!(snapshot.generation_revision, 1);
        assert_eq!(snapshot.candidate_count, 3);
        assert_eq!(snapshot.superseded_count, 0);
        assert_eq!(intent_states(target), vec!["reconciled"; 3]);
    }

    #[test]
    fn a_generation_leaves_one_reconciled_intent_per_candidate() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();

        assert_eq!(
            generate(&project, &mut shot, &ImageProvider::LocalDemo, &mut NoHooks).unwrap(),
            AttachOutcome::Attached
        );
        drop(project);
        assert_one_generation(&target);

        let project = CanonicalProject::open(&target).unwrap();
        assert!(
            unfinished(&project, &ImageProvider::LocalDemo)
                .unwrap()
                .plans
                .is_empty()
        );
    }

    #[test]
    fn a_generation_stopped_after_its_first_intent_completes_on_recovery() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        stop_at(&target, GenerationPoint::Prepared { index: 0 });
        assert_eq!(intent_states(&target), vec!["prepared"]);

        assert_eq!(recover(&target), (1, AttachOutcome::Attached));
        assert_one_generation(&target);
    }

    #[test]
    fn a_generation_stopped_before_attaching_attaches_on_recovery() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        stop_at(&target, GenerationPoint::Invoked);
        assert_eq!(intent_states(&target), vec!["invoked"; 3]);

        assert_eq!(recover(&target), (1, AttachOutcome::Attached));
        assert_one_generation(&target);
    }

    #[test]
    fn a_generation_stopped_after_attaching_is_only_acknowledged_on_recovery() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        stop_at(&target, GenerationPoint::Attached);
        assert_eq!(intent_states(&target), vec!["invoked"; 3]);
        let attached =
            fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap();

        assert_eq!(recover(&target), (1, AttachOutcome::AlreadyAttached));
        assert_one_generation(&target);
        assert_eq!(
            fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap(),
            attached
        );
    }

    #[test]
    fn an_attached_candidate_that_was_changed_is_not_acknowledged() {
        let other_hash = format!("sha256:{}", "e".repeat(64));
        let edits: [(&str, serde_json::Value); 4] = [
            ("content_hash", other_hash.clone().into()),
            ("input_hash", other_hash.into()),
            ("provider", "another-provider".into()),
            ("model", "another-model".into()),
        ];
        for (field, value) in edits {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            stop_at(&target, GenerationPoint::Attached);
            // Edited offline between the attach and the acknowledgement.
            let path = target.join("scenes/scene_001/shots/shot_001/artifacts.json");
            let mut artifacts: Vec<serde_json::Value> =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            artifacts[1][field] = value;
            fs::write(&path, serde_json::to_vec_pretty(&artifacts).unwrap()).unwrap();

            let project = CanonicalProject::open(&target).unwrap();
            let found = unfinished(&project, &ImageProvider::LocalDemo).unwrap();
            assert_eq!(found.plans.len(), 1);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let error = complete(
                &project,
                &mut shot,
                &found.plans[0],
                &ImageProvider::LocalDemo,
                &mut NoHooks,
            )
            .unwrap_err();
            assert!(
                matches!(
                    error,
                    GenerationError::Shot(ShotWorkflowError::AttachedRecordMismatch(ref id))
                        if id == "shot_001_g0001_candidate_b"
                ),
                "{field}: {error:?}"
            );
            assert_eq!(intent_states(&target), vec!["invoked"; 3], "{field}");
        }
    }

    #[test]
    fn the_demo_adapter_recomputes_the_same_result_from_its_key() {
        let candidate = ArtifactId::new("shot_001_g0001_candidate_b").unwrap();
        let input = InputHash::new(format!("sha256:{}", "a".repeat(64))).unwrap();
        let key = IdempotencyKey::new(format!(
            "{GENERATE_OPERATION}:{}:{}",
            candidate.as_str(),
            input.as_str()
        ))
        .unwrap();
        let mut adapter = LocalDemoAdapter;
        let request = CandidateRequest {
            artifact_id: candidate,
            input_hash: input,
            owner: CandidateOwner::Shot(ShotKey::bootstrap(1)),
            aspect_ratio: KEYFRAME_ASPECT,
            identities: Vec::new(),
            prompt: None,
        };

        let Invocation::Completed(invoked) = adapter.invoke(&request, &key).unwrap() else {
            panic!("demo generation completes synchronously");
        };
        let Reconciliation::Completed { output, .. } =
            adapter.reconcile_by_idempotency_key(&key).unwrap()
        else {
            panic!("demo generation is always reconcilable by key");
        };
        assert_eq!(output, invoked);
        assert!(
            adapter
                .reconcile_by_idempotency_key(&IdempotencyKey::new("video.generate:x:y").unwrap())
                .is_err()
        );
    }

    /// The demo generator, declaring that its images arrive as remote jobs.
    #[derive(Clone)]
    struct RemoteJobs;

    impl PaidJobAdapter for RemoteJobs {
        type Request = CandidateRequest;
        type Output = GeneratedCandidate;
        type Error = LocalDemoError;

        fn estimate_cost(
            &self,
            request: &CandidateRequest,
        ) -> Result<CostEstimate, LocalDemoError> {
            LocalDemoAdapter.estimate_cost(request)
        }

        fn invoke(
            &mut self,
            request: &CandidateRequest,
            key: &IdempotencyKey,
        ) -> Result<Invocation<GeneratedCandidate>, LocalDemoError> {
            LocalDemoAdapter.invoke(request, key)
        }

        fn reconcile(
            &mut self,
            id: &ProviderJobId,
        ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
            LocalDemoAdapter.reconcile(id)
        }

        fn reconcile_by_idempotency_key(
            &mut self,
            key: &IdempotencyKey,
        ) -> Result<Reconciliation<GeneratedCandidate>, LocalDemoError> {
            LocalDemoAdapter.reconcile_by_idempotency_key(key)
        }

        fn classify_error(&self, error: &LocalDemoError) -> ErrorClass {
            LocalDemoAdapter.classify_error(error)
        }
    }

    impl CandidateGenerator for RemoteJobs {
        fn provider(&self) -> ProviderKey {
            ProviderKey::new("fake-remote-images").unwrap()
        }

        fn policy(&self) -> JobRuntimePolicy {
            LocalDemoAdapter.policy()
        }

        fn needs_prompt(&self) -> bool {
            false
        }

        fn capabilities(&self) -> ImageCapabilities {
            ImageCapabilities {
                async_jobs: true,
                ..local_demo_capabilities()
            }
        }

        fn recover(
            &mut self,
            request: &CandidateRequest,
        ) -> Result<GeneratedCandidate, LocalDemoError> {
            LocalDemoAdapter.recover(request)
        }
    }

    #[test]
    fn providers_declare_how_images_arrive_and_what_one_costs() {
        for capabilities in [
            LocalDemoAdapter.capabilities(),
            ImageProvider::LocalDemo.capabilities(),
        ] {
            assert!(!capabilities.async_jobs);
            assert_eq!(capabilities.price_per_image, Some(Price::usd(0)));
            assert_eq!(capabilities.max_reference_images, 0, "no references");
        }
        let minimax = ImageProvider::Minimax(None).capabilities();
        assert!(!minimax.async_jobs);
        let price = minimax.price_per_image.unwrap();
        assert_eq!(price, Price::usd(3_500));
        // What the director is shown comes from the declared price.
        assert_eq!(ImageProvider::LocalDemo.info().generation_cost_micros, 0);
        assert_eq!(
            ImageProvider::Minimax(None).info().generation_cost_micros,
            price.amount_micros * u64::try_from(GENERATED_CANDIDATE_COUNT).unwrap()
        );
    }

    #[test]
    fn an_image_provider_of_remote_jobs_is_refused_before_anything_is_written() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert!(matches!(
            generate_with(&project, &mut shot, RemoteJobs, &mut NoHooks),
            Err(GenerationError::Prerequisite(
                Prerequisite::SynchronousImageProvider
            ))
        ));
        assert!(intent_states(&target).is_empty());
        assert!(!shot.snapshot().unwrap().generated);
        // The same project generates with a provider that answers at once.
        assert_eq!(
            generate_with(&project, &mut shot, LocalDemoAdapter, &mut NoHooks).unwrap(),
            AttachOutcome::Attached
        );
    }

    mod minimax {
        use super::*;
        use crate::minimax::MinimaxKeyframes;
        use base64::Engine as _;
        use kineto_project::ContentHash;
        use kineto_provider_minimax::{HttpResponse, Transport, TransportFailure};
        use kineto_providers::secret::SecretString;
        use std::sync::{
            Arc, Mutex,
            atomic::{AtomicU32, Ordering},
        };

        /// Answers every image request with a distinct tiny JPEG, or with the
        /// refusal it was given.
        #[derive(Default)]
        struct FakeMinimax {
            calls: Arc<AtomicU32>,
            refuse: Mutex<Option<String>>,
            /// A call that goes out and gets no answer.
            lose: Mutex<Option<TransportFailure>>,
        }

        impl Transport for FakeMinimax {
            fn post_json(
                &self,
                _url: &str,
                _api_key: &SecretString,
                body: &[u8],
            ) -> Result<HttpResponse, TransportFailure> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                let sent: serde_json::Value = serde_json::from_slice(body).unwrap();
                assert!(sent["prompt"].as_str().unwrap().contains("filmmaker"));
                if let Some(failure) = self.lose.lock().unwrap().take() {
                    return Err(failure);
                }
                let body = match self.refuse.lock().unwrap().take() {
                    Some(refusal) => refusal,
                    None => {
                        let jpeg = [0xff, 0xd8, 0xff, 0xe0, 0x4b, u8::try_from(call).unwrap()];
                        format!(
                            r#"{{"id":"req-{call}","data":{{"image_base64":["{}"]}},"base_resp":{{"status_code":0,"status_msg":"success"}}}}"#,
                            base64::engine::general_purpose::STANDARD.encode(jpeg)
                        )
                    }
                };
                Ok(HttpResponse {
                    status: 200,
                    body: body.into_bytes(),
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

        fn client(fake: FakeMinimax) -> (MinimaxClient<FakeMinimax>, Arc<AtomicU32>) {
            let calls = Arc::clone(&fake.calls);
            let client = MinimaxClient::with_transport(
                fake,
                SecretString::new("sk-test".to_owned()).unwrap(),
                "https://example.test",
            );
            assert!(!format!("{client:?}").contains("sk-test"));
            (client, calls)
        }

        fn bootstrap_project(target: &Path) -> CanonicalProject {
            let project = project(target);
            for (path, bytes) in crate::bootstrap::scene_files() {
                project.root().write_atomic(&path, &bytes).unwrap();
            }
            project
        }

        fn artifacts(target: &Path) -> Vec<serde_json::Value> {
            let bytes =
                fs::read(target.join("scenes/scene_001/shots/shot_001/artifacts.json")).unwrap();
            serde_json::from_slice(&bytes).unwrap()
        }

        /// Names of the files under `.kineto/generation/`.
        fn generation_files(target: &Path) -> Vec<String> {
            fs::read_dir(target.join(".kineto/generation")).map_or(Vec::new(), |entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
        }

        fn receipts(target: &Path) -> usize {
            generation_files(target)
                .iter()
                .filter(|name| !name.ends_with(".refused.json"))
                .count()
        }

        fn refusals(target: &Path) -> usize {
            generation_files(target)
                .iter()
                .filter(|name| name.ends_with(".refused.json"))
                .count()
        }

        #[test]
        fn keyframes_land_in_the_shot_with_their_provenance() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let (client, calls) = client(FakeMinimax::default());

            let outcome = generate_with(
                &project,
                &mut shot,
                MinimaxKeyframes::new(&project, Some(&client)),
                &mut NoHooks,
            )
            .unwrap();

            assert_eq!(outcome, AttachOutcome::Attached);
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            assert_eq!(intent_states(&target), vec!["reconciled"; 3]);
            assert_eq!(receipts(&target), 0, "receipts go once acknowledged");
            let media = shot.candidate_media().unwrap();
            assert_eq!(media.len(), 3);
            for (artifact, path) in artifacts(&target).iter().zip(&media) {
                let path = path.as_ref().expect("every candidate has a keyframe");
                let bytes = project.root().read(path).unwrap();
                assert_eq!(
                    artifact["content_hash"],
                    ContentHash::of(&bytes).as_str(),
                    "the content hash is the keyframe's own"
                );
                assert_eq!(artifact["provider"], "minimax");
                assert_eq!(artifact["model"], "image-01");
                assert_eq!(artifact["media_state"], "present");
                assert_eq!(artifact["compiler_version"], "keyframe-prompt/4");
                assert_eq!(artifact["degradation"], "identity_affecting");
                assert!(artifact["media_path"].as_str().unwrap().starts_with(
                    "scenes/scene_001/shots/shot_001/keyframes/shot_001_g0001_candidate_"
                ));
                assert!(
                    artifact["compiled_prompt"]
                        .as_str()
                        .unwrap()
                        .contains("filmmaker")
                );
            }
        }

        #[test]
        fn a_generation_stopped_after_its_calls_finishes_without_paying_again() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            {
                let project = bootstrap_project(&target);
                let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
                let (client, calls) = client(FakeMinimax::default());
                let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    generate_with(
                        &project,
                        &mut shot,
                        MinimaxKeyframes::new(&project, Some(&client)),
                        &mut StopAt(GenerationPoint::Invoked),
                    )
                }));
                assert!(stopped.is_err());
                assert_eq!(calls.load(Ordering::SeqCst), 3);
            }
            assert_eq!(intent_states(&target), vec!["invoked"; 3]);
            assert_eq!(receipts(&target), 3);

            // No key at all: the finished calls complete from their receipts.
            let project = CanonicalProject::open(&target).unwrap();
            let found = unfinished_with(
                &project,
                MinimaxKeyframes::<FakeMinimax>::new(&project, None),
            )
            .unwrap();
            assert_eq!(found.needs_attention, 0);
            assert_eq!(found.plans.len(), 1);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let outcome = complete_with(
                &project,
                &mut shot,
                &found.plans[0],
                MinimaxKeyframes::<FakeMinimax>::new(&project, None),
                &mut NoHooks,
            )
            .unwrap();
            assert_eq!(outcome, AttachOutcome::Attached);
            assert_eq!(intent_states(&target), vec!["reconciled"; 3]);
            assert_eq!(shot.snapshot().unwrap().candidate_count, 3);
            assert_eq!(receipts(&target), 0);
        }

        #[test]
        fn an_attached_keyframe_edited_before_acknowledgement_is_not_acknowledged() {
            for edit in ["intact", "content_hash", "media_path"] {
                let temp = TempDir::new();
                let target = temp.0.join("film");
                {
                    let project = bootstrap_project(&target);
                    let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
                    let (client, _calls) = client(FakeMinimax::default());
                    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        generate_with(
                            &project,
                            &mut shot,
                            MinimaxKeyframes::new(&project, Some(&client)),
                            &mut StopAt(GenerationPoint::Attached),
                        )
                    }));
                    assert!(stopped.is_err());
                }
                let path = target.join("scenes/scene_001/shots/shot_001/artifacts.json");
                let mut artifacts: Vec<serde_json::Value> =
                    serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                match edit {
                    "content_hash" => {
                        artifacts[1]["content_hash"] = format!("sha256:{}", "e".repeat(64)).into();
                    }
                    "media_path" => {
                        artifacts[1]["media_path"] =
                            "scenes/scene_001/shots/shot_001/keyframes/other.jpg".into();
                    }
                    _ => {}
                }
                fs::write(&path, serde_json::to_vec_pretty(&artifacts).unwrap()).unwrap();

                let project = CanonicalProject::open(&target).unwrap();
                let found = unfinished_with(
                    &project,
                    MinimaxKeyframes::<FakeMinimax>::new(&project, None),
                )
                .unwrap();
                let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
                let outcome = complete_with(
                    &project,
                    &mut shot,
                    &found.plans[0],
                    MinimaxKeyframes::<FakeMinimax>::new(&project, None),
                    &mut NoHooks,
                );
                if edit == "intact" {
                    assert_eq!(outcome.unwrap(), AttachOutcome::AlreadyAttached);
                    assert_eq!(intent_states(&target), vec!["reconciled"; 3]);
                    assert_eq!(receipts(&target), 0);
                } else {
                    assert!(
                        matches!(
                            outcome,
                            Err(GenerationError::Shot(
                                ShotWorkflowError::AttachedRecordMismatch(_)
                            ))
                        ),
                        "{edit}: {outcome:?}"
                    );
                    assert_eq!(intent_states(&target), vec!["invoked"; 3], "{edit}");
                    assert_eq!(receipts(&target), 3, "{edit}: receipts are kept");
                }
            }
        }

        #[test]
        fn a_receipt_that_no_longer_matches_its_keyframe_is_not_attached() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            {
                let project = bootstrap_project(&target);
                let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
                let (client, _calls) = client(FakeMinimax::default());
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    generate_with(
                        &project,
                        &mut shot,
                        MinimaxKeyframes::new(&project, Some(&client)),
                        &mut StopAt(GenerationPoint::Invoked),
                    )
                }));
            }
            let keyframes = target.join("scenes/scene_001/shots/shot_001/keyframes");
            let tampered = fs::read_dir(&keyframes)
                .unwrap()
                .next()
                .unwrap()
                .unwrap()
                .path();
            fs::write(&tampered, b"other bytes").unwrap();

            let project = CanonicalProject::open(&target).unwrap();
            let found = unfinished_with(
                &project,
                MinimaxKeyframes::<FakeMinimax>::new(&project, None),
            )
            .unwrap();
            assert_eq!(
                found.needs_attention, 1,
                "the tampered candidate needs a person"
            );
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            assert!(
                complete_with(
                    &project,
                    &mut shot,
                    &found.plans[0],
                    MinimaxKeyframes::<FakeMinimax>::new(&project, None),
                    &mut NoHooks,
                )
                .is_err()
            );
            assert!(!shot.snapshot().unwrap().generated);
        }

        #[test]
        fn a_refused_candidate_fails_the_generation_and_keeps_the_others() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let fake = FakeMinimax::default();
            *fake.refuse.lock().unwrap() = Some(
                r#"{"base_resp":{"status_code":1026,"status_msg":"input new_sensitive"}}"#
                    .to_owned(),
            );
            let (client, calls) = client(fake);

            let error = generate_with(
                &project,
                &mut shot,
                MinimaxKeyframes::new(&project, Some(&client)),
                &mut NoHooks,
            )
            .unwrap_err();

            assert!(matches!(
                error,
                GenerationError::Provider {
                    class: ErrorClass::Terminal,
                    ..
                }
            ));
            assert!(!shot.snapshot().unwrap().generated);
            assert_eq!(intent_states(&target), ["invoked", "invoked", "prepared"]);
            assert_eq!(receipts(&target), 2);
            // MiniMax answered with a refusal, so nothing was charged for it.
            assert_eq!(refusals(&target), 1);
            assert_eq!(calls.load(Ordering::SeqCst), 3);

            // Reopening completes nothing and pays for nothing: the refused
            // candidate waits for the director.
            let found =
                unfinished_with(&project, MinimaxKeyframes::new(&project, Some(&client))).unwrap();
            assert!(found.plans.is_empty());
            assert_eq!(found.needs_attention, 0);
            assert_eq!(calls.load(Ordering::SeqCst), 3);

            // Asking again pays only for the refused candidate.
            let outcome = generate_with(
                &project,
                &mut shot,
                MinimaxKeyframes::new(&project, Some(&client)),
                &mut NoHooks,
            )
            .unwrap();
            assert_eq!(outcome, AttachOutcome::Attached);
            assert_eq!(calls.load(Ordering::SeqCst), 4);
            assert_eq!(shot.snapshot().unwrap().generation_revision, 1);
            assert_eq!(intent_states(&target), vec!["reconciled"; 3]);
            assert_eq!(receipts(&target), 0);
            assert_eq!(refusals(&target), 0);
        }

        #[test]
        fn a_call_answered_before_anything_was_written_is_never_paid_for_again_unasked() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let (client, calls) = client(FakeMinimax::default());
            let crash = || panic!("the process dies as MiniMax answers");

            let error = generate_with(
                &project,
                &mut shot,
                MinimaxKeyframes::new(&project, Some(&client)).when_answered(&crash),
                &mut NoHooks,
            )
            .unwrap_err();
            assert!(matches!(error, GenerationError::Unexpected(_)), "{error}");
            // Charged three times, and nothing written down.
            assert_eq!(calls.load(Ordering::SeqCst), 3);
            assert_eq!(intent_states(&target), vec!["prepared"; 3]);
            assert_eq!((receipts(&target), refusals(&target)), (0, 0));

            let minimax = || MinimaxKeyframes::new(&project, Some(&client));
            assert!(matches!(
                generate_with(&project, &mut shot, minimax(), &mut NoHooks),
                Err(GenerationError::UnresolvedSubmission)
            ));
            let found = unfinished_with(&project, minimax()).unwrap();
            assert_eq!(
                (found.plans.len(), found.unresolved),
                (0, vec![ShotKey::bootstrap(1)])
            );
            assert_eq!(calls.load(Ordering::SeqCst), 3, "never paid twice unasked");

            // The director decides to risk it.
            let outcome = resubmit_with(&project, &mut shot, minimax(), &mut NoHooks).unwrap();
            assert_eq!(outcome, AttachOutcome::Attached);
            assert_eq!(calls.load(Ordering::SeqCst), 6);
            assert_eq!(intent_states(&target), vec!["reconciled"; 3]);
        }

        #[test]
        fn a_kept_answer_is_used_even_if_its_intent_never_said_so() {
            struct StopAfterCalls;
            impl GenerationHooks for StopAfterCalls {
                fn reached(&mut self, point: GenerationPoint) {
                    if point == GenerationPoint::Invoked {
                        panic!("stopped after the calls");
                    }
                }
            }
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let (client, calls) = client(FakeMinimax::default());
            let minimax = || MinimaxKeyframes::new(&project, Some(&client));
            let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                generate_with(&project, &mut shot, minimax(), &mut StopAfterCalls)
            }));
            assert!(stopped.is_err());
            assert_eq!(receipts(&target), 3);
            // The state a failed intent save after the receipt leaves.
            for entry in fs::read_dir(target.join(".kineto/jobs/intents")).unwrap() {
                let path = entry.unwrap().path();
                if path
                    .extension()
                    .is_some_and(|extension| extension == "json")
                {
                    let mut intent: serde_json::Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    intent["state"] = "prepared".into();
                    fs::write(&path, serde_json::to_vec_pretty(&intent).unwrap()).unwrap();
                }
            }

            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let outcome = generate_with(&project, &mut shot, minimax(), &mut NoHooks).unwrap();
            assert_eq!(outcome, AttachOutcome::Attached);
            assert_eq!(
                calls.load(Ordering::SeqCst),
                3,
                "the kept answers were used"
            );
            assert_eq!(intent_states(&target), vec!["reconciled"; 3]);
        }

        #[test]
        fn a_call_lost_after_it_was_sent_waits_for_the_director() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let fake = FakeMinimax::default();
            *fake.lose.lock().unwrap() = Some(TransportFailure::Timeout);
            let (client, calls) = client(fake);
            let minimax = || MinimaxKeyframes::new(&project, Some(&client));

            for _ in 0..2 {
                assert!(matches!(
                    generate_with(&project, &mut shot, minimax(), &mut NoHooks),
                    Err(GenerationError::UnresolvedSubmission)
                ));
            }
            assert_eq!(
                calls.load(Ordering::SeqCst),
                3,
                "the second ask paid nothing"
            );
            assert_eq!(intent_states(&target), ["invoked", "invoked", "prepared"]);
            assert_eq!(refusals(&target), 0);

            let outcome = resubmit_with(&project, &mut shot, minimax(), &mut NoHooks).unwrap();
            assert_eq!(outcome, AttachOutcome::Attached);
            assert_eq!(calls.load(Ordering::SeqCst), 4);
        }

        #[test]
        fn recovery_never_makes_a_paid_call() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let (client, calls) = client(FakeMinimax::default());
            let plan = shot.plan_generation(&project).unwrap();
            // A candidate prepared but never invoked, as after a crash.
            let runtime = JobRuntime::new(
                &project,
                ProviderKey::new("minimax").unwrap(),
                MinimaxKeyframes::new(&project, Some(&client)).policy(),
            );
            runtime
                .prepare(
                    JobId::new(plan.candidate_ids()[0].as_str()).unwrap(),
                    GENERATE_OPERATION,
                    plan.input_hash().clone(),
                    plan.candidate_ids()[0].clone(),
                )
                .unwrap();
            drop(runtime);

            assert!(matches!(
                complete_with(
                    &project,
                    &mut shot,
                    &plan,
                    MinimaxKeyframes::new(&project, Some(&client)),
                    &mut NoHooks,
                ),
                Err(GenerationError::NeedsProviderCall)
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }

        #[test]
        fn nothing_is_written_without_a_key_or_an_intent() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            assert!(matches!(
                generate_with(
                    &project,
                    &mut shot,
                    MinimaxKeyframes::<FakeMinimax>::new(&project, None),
                    &mut NoHooks,
                ),
                Err(GenerationError::NotConfigured)
            ));

            let bare_target = temp.0.join("bare");
            let bare = project_without_intent(&bare_target);
            let mut shot = ShotWorkflow::load(&bare, &ShotKey::bootstrap(1)).unwrap();
            let (client, calls) = client(FakeMinimax::default());
            assert!(matches!(
                generate_with(
                    &bare,
                    &mut shot,
                    MinimaxKeyframes::new(&bare, Some(&client)),
                    &mut NoHooks,
                ),
                Err(GenerationError::Prompt(KeyframePromptError::MissingIntent))
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(intent_states(&target).is_empty());
            assert!(intent_states(&bare_target).is_empty());
        }

        #[test]
        fn a_prompt_longer_than_minimax_takes_is_refused_before_anything_is_written() {
            let temp = TempDir::new();
            let target = temp.0.join("film");
            let project = bootstrap_project(&target);
            let path = target.join("scenes/scene_001/shots/shot_001/shot.json");
            let mut intent: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            intent["action"] = format!("A filmmaker {}", "waits ".repeat(300)).into();
            fs::write(&path, serde_json::to_vec_pretty(&intent).unwrap()).unwrap();
            let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            let (client, calls) = client(FakeMinimax::default());

            assert!(matches!(
                generate_with(
                    &project,
                    &mut shot,
                    MinimaxKeyframes::new(&project, Some(&client)),
                    &mut NoHooks,
                ),
                Err(GenerationError::Prompt(KeyframePromptError::TooLong {
                    max: kineto_provider_minimax::MAX_PROMPT_CHARS,
                    ..
                }))
            ));
            assert_eq!(calls.load(Ordering::SeqCst), 0);
            assert!(intent_states(&target).is_empty());
        }

        fn project_without_intent(target: &Path) -> CanonicalProject {
            project(target)
        }
    }
}
