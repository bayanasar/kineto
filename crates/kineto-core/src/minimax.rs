//! MiniMax `image-01` keyframes behind the paid-job contract.
//!
//! A provider call is only as durable as what is kept of its answer. The
//! adapter therefore writes the image into the shot's `keyframes/` directory
//! and then a receipt under `.kineto/generation/` before it returns, and the
//! job runtime marks the job invoked only after that. A restart finds the
//! receipt and completes the candidate from disk instead of paying again:
//!
//! ```text
//! call MiniMax -> write keyframe -> write receipt -> intent invoked -> attach -> acknowledge
//! ```
//!
//! MiniMax keeps no record a request could be looked up by, so the receipt
//! is the only way back to a finished call. A call that was made but left no
//! receipt (a crash before it was written, or a failure after the request
//! was sent) may have been charged; it is called again only on the
//! director's explicit decision to risk paying twice. Only a call MiniMax
//! certainly did not accept leaves a refusal record next to where its
//! receipt would be, and that proof lets the candidate be asked for again.

use std::{error::Error, fmt, num::NonZeroU16};

use kineto_job_runtime::{
    Invocation, JobRuntimePolicy, PaidJobAdapter, ProviderKey, Reconciliation,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, ErrorClass, FallbackPolicy, IdempotencyKey, PaidFallback,
    ProviderExecutionPolicy, ProviderJobId,
};
use kineto_project::{
    ArtifactDependency, ArtifactId, ContentHash, InputHash, cast,
    fs::{ProjectFsError, ProjectRelativePath},
    manifest::CanonicalProject,
    revisions::DependencyRecord,
    shot::{CandidateOutput, PromptProvenance, keyframe_media_path},
    style,
};
use kineto_provider_minimax::{
    AspectRatio, HttpsTransport, IMAGE_MODEL, ImageRequest, MinimaxClient, MinimaxError, PROVIDER,
    SubjectReference, Transport,
};
use kineto_providers::{ImageCapabilities, ImageFormat};
use serde::{Deserialize, Serialize};

use crate::generation::{
    CandidateGenerator, CandidateOwner, CandidateRequest, GeneratedCandidate, parse_candidate_key,
};

const RECEIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug)]
pub enum MinimaxKeyframeError {
    /// MiniMax refused or could not be reached.
    Provider(MinimaxError),
    /// No API key is configured, so no call can be made.
    NotConfigured,
    /// The candidate has no compiled prompt to send.
    MissingPrompt,
    /// A finished call's receipt or keyframe is missing, unreadable, or does
    /// not match. Nothing is regenerated; a person needs to look.
    Receipt(&'static str),
    /// Writing down a call's answer failed after MiniMax gave it.
    Fs(ProjectFsError),
    /// The project could not be prepared for a call, so none was made.
    Unsent(ProjectFsError),
    /// The request asks for something `image-01` cannot do; nothing was
    /// sent.
    Unsupported(&'static str),
}

impl fmt::Display for MinimaxKeyframeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => error.fmt(formatter),
            Self::NotConfigured => formatter.write_str("no MiniMax API key is configured"),
            Self::MissingPrompt => formatter.write_str("the candidate has no compiled prompt"),
            Self::Receipt(what) => write!(formatter, "MiniMax keyframe receipt: {what}"),
            Self::Fs(error) => error.fmt(formatter),
            Self::Unsent(error) => write!(formatter, "no MiniMax call was made: {error}"),
            Self::Unsupported(what) => write!(formatter, "no MiniMax call was made: {what}"),
        }
    }
}

impl Error for MinimaxKeyframeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Provider(error) => Some(error),
            Self::Fs(error) | Self::Unsent(error) => Some(error),
            Self::NotConfigured | Self::MissingPrompt | Self::Receipt(_) | Self::Unsupported(_) => {
                None
            }
        }
    }
}

/// What is kept of a finished MiniMax call until its candidate is
/// acknowledged. Runtime state, never canonical: the canonical record is the
/// artifact this becomes.
#[derive(Debug, Serialize, Deserialize)]
struct Receipt {
    schema_version: u32,
    provider: String,
    model: String,
    request_id: String,
    artifact_id: String,
    input_hash: String,
    media_path: String,
    content_hash: String,
    provenance: PromptProvenance,
    /// What the prompt was compiled from; older receipts have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<DependencyRecord>,
}

/// Proof that MiniMax certainly did not accept a candidate's latest call.
#[derive(Debug, Serialize, Deserialize)]
struct Refusal {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
}

/// MiniMax keyframes for one project. Cheap to copy: every candidate of a
/// generation gets its own copy and they run in parallel.
pub struct MinimaxKeyframes<'a, T = HttpsTransport> {
    project: &'a CanonicalProject,
    client: Option<&'a MinimaxClient<T>>,
    answered: Option<&'a (dyn Fn() + Sync)>,
}

impl<T> Clone for MinimaxKeyframes<'_, T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for MinimaxKeyframes<'_, T> {}

impl<'a, T: Transport> MinimaxKeyframes<'a, T> {
    /// Without a client the adapter can still complete finished calls from
    /// their receipts, but cannot make new ones.
    #[must_use]
    pub const fn new(project: &'a CanonicalProject, client: Option<&'a MinimaxClient<T>>) -> Self {
        Self {
            project,
            client,
            answered: None,
        }
    }

    /// Run `hook` as soon as MiniMax has answered a call, before anything is
    /// written: where a crash test stops.
    #[must_use]
    pub const fn when_answered(mut self, hook: &'a (dyn Fn() + Sync)) -> Self {
        self.answered = Some(hook);
        self
    }

    fn refusal_path(artifact_id: &ArtifactId) -> Result<ProjectRelativePath, MinimaxKeyframeError> {
        ProjectRelativePath::new(format!(
            ".kineto/generation/{}.refused.json",
            artifact_id.as_str()
        ))
        .map_err(|_| MinimaxKeyframeError::Receipt("artifact ID is not a valid file name"))
    }

    fn refused(&self, request: &CandidateRequest) -> bool {
        Self::refusal_path(&request.artifact_id)
            .ok()
            .and_then(|path| self.project.root().read(&path).ok())
            .and_then(|bytes| serde_json::from_slice::<Refusal>(&bytes).ok())
            .is_some_and(|refusal| {
                refusal.schema_version == RECEIPT_SCHEMA_VERSION
                    && refusal.artifact_id == request.artifact_id.as_str()
                    && refusal.input_hash == request.input_hash.as_str()
            })
    }

    fn record_refusal(&self, request: &CandidateRequest) -> Result<(), MinimaxKeyframeError> {
        let mut encoded = serde_json::to_vec_pretty(&Refusal {
            schema_version: RECEIPT_SCHEMA_VERSION,
            artifact_id: request.artifact_id.as_str().to_owned(),
            input_hash: request.input_hash.as_str().to_owned(),
        })
        .map_err(|_| MinimaxKeyframeError::Receipt("refusal did not encode"))?;
        encoded.push(b'\n');
        self.project
            .root()
            .write_atomic(&Self::refusal_path(&request.artifact_id)?, &encoded)
            .map_err(MinimaxKeyframeError::Fs)
    }

    /// Forget the proof of an earlier refusal before a new call: it speaks
    /// for that call only.
    fn clear_refusal(&self, artifact_id: &ArtifactId) -> Result<(), MinimaxKeyframeError> {
        let path = self
            .project
            .root()
            .resolve(&Self::refusal_path(artifact_id)?);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(MinimaxKeyframeError::Unsent(ProjectFsError::Io(error))),
        }
    }

    fn receipt_path(artifact_id: &ArtifactId) -> Result<ProjectRelativePath, MinimaxKeyframeError> {
        ProjectRelativePath::new(format!(".kineto/generation/{}.json", artifact_id.as_str()))
            .map_err(|_| MinimaxKeyframeError::Receipt("artifact ID is not a valid file name"))
    }

    fn read_receipt(
        &self,
        artifact_id: &ArtifactId,
    ) -> Result<Option<Receipt>, MinimaxKeyframeError> {
        match self.project.root().read(&Self::receipt_path(artifact_id)?) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| MinimaxKeyframeError::Receipt("receipt is not readable")),
            Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(MinimaxKeyframeError::Fs(error)),
        }
    }

    /// Rebuild a candidate from its receipt, after proving the keyframe on
    /// disk is the one the receipt names.
    fn candidate_from_receipt(
        &self,
        artifact_id: &ArtifactId,
        input_hash: &InputHash,
        receipt: Receipt,
    ) -> Result<GeneratedCandidate, MinimaxKeyframeError> {
        if receipt.schema_version != RECEIPT_SCHEMA_VERSION
            || receipt.artifact_id != artifact_id.as_str()
            || receipt.input_hash != input_hash.as_str()
            || receipt.provider != PROVIDER
        {
            return Err(MinimaxKeyframeError::Receipt(
                "receipt belongs to another candidate",
            ));
        }
        let media_path = ProjectRelativePath::new(&receipt.media_path)
            .map_err(|_| MinimaxKeyframeError::Receipt("receipt media path is invalid"))?;
        let content_hash = ContentHash::new(receipt.content_hash)
            .map_err(|_| MinimaxKeyframeError::Receipt("receipt content hash is invalid"))?;
        let bytes = match self.project.root().read(&media_path) {
            Ok(bytes) => bytes,
            Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(MinimaxKeyframeError::Receipt("keyframe file is missing"));
            }
            Err(error) => return Err(MinimaxKeyframeError::Fs(error)),
        };
        if ContentHash::of(&bytes) != content_hash {
            return Err(MinimaxKeyframeError::Receipt(
                "keyframe file does not match its receipt",
            ));
        }
        Ok(GeneratedCandidate {
            output: CandidateOutput {
                artifact_id: artifact_id.clone(),
                content_hash,
                provider: receipt.provider,
                model: receipt.model,
                media_path: Some(media_path),
                provenance: Some(receipt.provenance),
                dependencies: receipt
                    .dependencies
                    .iter()
                    .map(|dependency| {
                        Ok(ArtifactDependency {
                            artifact_id: ArtifactId::new(dependency.artifact_id.clone()).map_err(
                                |_| MinimaxKeyframeError::Receipt("a dependency is invalid"),
                            )?,
                            content_hash: ContentHash::new(dependency.content_hash.clone())
                                .map_err(|_| {
                                    MinimaxKeyframeError::Receipt("a dependency is invalid")
                                })?,
                            impact: dependency.impact,
                            field: dependency.field.clone(),
                        })
                    })
                    .collect::<Result<_, MinimaxKeyframeError>>()?,
            },
            input_hash: input_hash.clone(),
        })
    }
}

fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::Webp => "webp",
    }
}

impl<T: Transport> PaidJobAdapter for MinimaxKeyframes<'_, T> {
    type Request = CandidateRequest;
    type Output = GeneratedCandidate;
    type Error = MinimaxKeyframeError;

    fn estimate_cost(&self, _request: &CandidateRequest) -> Result<CostEstimate, Self::Error> {
        self.client
            .map(MinimaxClient::estimate_image_cost)
            .ok_or(MinimaxKeyframeError::NotConfigured)
    }

    fn invoke(
        &mut self,
        request: &CandidateRequest,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<GeneratedCandidate>, Self::Error> {
        let client = self.client.ok_or(MinimaxKeyframeError::NotConfigured)?;
        let prompt = request
            .prompt
            .as_deref()
            .ok_or(MinimaxKeyframeError::MissingPrompt)?;
        // An earlier call answered and wrote its receipt, but the intent never
        // said so: use that answer instead of paying for another.
        if let Some(receipt) = self.read_receipt(&request.artifact_id)? {
            return self
                .candidate_from_receipt(&request.artifact_id, &request.input_hash, receipt)
                .map(Invocation::Completed);
        }
        self.clear_refusal(&request.artifact_id)?;
        let aspect_ratio = match (request.aspect_ratio.width, request.aspect_ratio.height) {
            (1, 1) => AspectRatio::Square,
            (16, 9) => AspectRatio::Landscape16x9,
            (9, 16) => AspectRatio::Portrait9x16,
            _ => {
                let _ = self.record_refusal(request);
                return Err(MinimaxKeyframeError::Unsupported(
                    "image-01 takes 1:1, 16:9 or 9:16",
                ));
            }
        };
        // `image-01` takes one subject reference, as its capabilities say.
        let subject_reference = match request.identities.as_slice() {
            [] => None,
            [identity] => Some(SubjectReference(identity.bytes.to_vec())),
            _ => {
                let _ = self.record_refusal(request);
                return Err(MinimaxKeyframeError::Unsupported(
                    "image-01 takes one identity reference per image",
                ));
            }
        };
        let image = match client.generate_image(&ImageRequest {
            prompt: prompt.compiled.prompt().to_owned(),
            aspect_ratio,
            subject_reference,
        }) {
            Ok(image) => image,
            Err(error) => {
                // Without this proof the call stays unresolved, which fails
                // closed.
                if error.certainly_not_accepted() {
                    let _ = self.record_refusal(request);
                }
                return Err(MinimaxKeyframeError::Provider(error));
            }
        };
        if let Some(answered) = self.answered {
            answered();
        }

        let media_path = match &request.owner {
            CandidateOwner::Shot(key) => {
                keyframe_media_path(key, &request.artifact_id, extension(image.format))
                    .map_err(|_| MinimaxKeyframeError::Receipt("keyframe path is invalid"))?
            }
            CandidateOwner::Style => {
                style::sample_media_path(&request.artifact_id, extension(image.format))
                    .map_err(|_| MinimaxKeyframeError::Receipt("style sample path is invalid"))?
            }
            CandidateOwner::Character { id, kind } => {
                cast::media_path(id, &request.artifact_id, *kind, extension(image.format))
                    .map_err(|_| MinimaxKeyframeError::Receipt("casting image path is invalid"))?
            }
        };
        let root = self.project.root();
        root.write_atomic(&media_path, &image.bytes)
            .map_err(MinimaxKeyframeError::Fs)?;

        let receipt = Receipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            provider: PROVIDER.to_owned(),
            model: IMAGE_MODEL.to_owned(),
            request_id: image.request_id,
            artifact_id: request.artifact_id.as_str().to_owned(),
            input_hash: request.input_hash.as_str().to_owned(),
            media_path: media_path.as_path().to_string_lossy().into_owned(),
            content_hash: ContentHash::of(&image.bytes).as_str().to_owned(),
            provenance: crate::generation::provenance(prompt),
            dependencies: prompt
                .dependencies
                .iter()
                .map(DependencyRecord::from)
                .collect(),
        };
        let mut encoded = serde_json::to_vec_pretty(&receipt)
            .map_err(|_| MinimaxKeyframeError::Receipt("receipt did not encode"))?;
        encoded.push(b'\n');
        root.write_atomic(&Self::receipt_path(&request.artifact_id)?, &encoded)
            .map_err(MinimaxKeyframeError::Fs)?;

        self.candidate_from_receipt(&request.artifact_id, &request.input_hash, receipt)
            .map(Invocation::Completed)
    }

    fn reconcile(
        &mut self,
        _provider_job_id: &ProviderJobId,
    ) -> Result<Reconciliation<GeneratedCandidate>, Self::Error> {
        // Image calls are synchronous; no remote job handle is ever recorded.
        Ok(Reconciliation::NotFound)
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Reconciliation<GeneratedCandidate>, Self::Error> {
        let (artifact_id, input_hash) = parse_candidate_key(idempotency_key)
            .map_err(|_| MinimaxKeyframeError::Receipt("idempotency key is not a candidate key"))?;
        match self.read_receipt(&artifact_id)? {
            None => Ok(Reconciliation::NotFound),
            Some(receipt) => Ok(Reconciliation::Completed {
                output: self.candidate_from_receipt(&artifact_id, &input_hash, receipt)?,
                provider_job_id: None,
            }),
        }
    }

    fn classify_error(&self, error: &MinimaxKeyframeError) -> ErrorClass {
        match error {
            MinimaxKeyframeError::Provider(error) => error.class(),
            // Writing the answer down failed; the disk may recover.
            MinimaxKeyframeError::Fs(_) | MinimaxKeyframeError::Unsent(_) => ErrorClass::Retryable,
            MinimaxKeyframeError::NotConfigured
            | MinimaxKeyframeError::MissingPrompt
            | MinimaxKeyframeError::Receipt(_)
            | MinimaxKeyframeError::Unsupported(_) => ErrorClass::Terminal,
        }
    }
}

impl<T: Transport + Sync> CandidateGenerator for MinimaxKeyframes<'_, T> {
    fn provider(&self) -> ProviderKey {
        ProviderKey::new(PROVIDER).expect("static provider key is valid")
    }

    fn model(&self) -> String {
        IMAGE_MODEL.to_owned()
    }

    fn policy(&self) -> JobRuntimePolicy {
        JobRuntimePolicy {
            // One call per candidate of a generation at once.
            execution: ProviderExecutionPolicy::new(
                NonZeroU16::new(3).expect("static concurrency is non-zero"),
                BackoffPolicy::new(2_000, 30_000, 3).expect("static backoff policy is valid"),
            ),
            // Generation is an explicit, chosen spend; nothing falls back to it.
            fallback_policy: FallbackPolicy {
                paid_fallback: PaidFallback::Forbidden,
                max_paid_amount_micros: Some(0),
                require_user_approval: true,
            },
        }
    }

    fn needs_prompt(&self) -> bool {
        true
    }

    fn capabilities(&self) -> ImageCapabilities {
        kineto_provider_minimax::image_capabilities()
    }

    fn can_invoke(&self) -> bool {
        self.client.is_some()
    }

    fn recover(&mut self, request: &CandidateRequest) -> Result<GeneratedCandidate, Self::Error> {
        let receipt =
            self.read_receipt(&request.artifact_id)?
                .ok_or(MinimaxKeyframeError::Receipt(
                    "a finished call has no receipt",
                ))?;
        self.candidate_from_receipt(&request.artifact_id, &request.input_hash, receipt)
    }

    fn may_call_again(&self, request: &CandidateRequest) -> bool {
        // A kept answer is used without a call; a refusal proves nothing was
        // charged.
        matches!(self.read_receipt(&request.artifact_id), Ok(Some(_))) || self.refused(request)
    }

    fn certainly_not_charged(&self, error: &MinimaxKeyframeError) -> bool {
        match error {
            MinimaxKeyframeError::Provider(error) => error.certainly_not_accepted(),
            MinimaxKeyframeError::NotConfigured
            | MinimaxKeyframeError::MissingPrompt
            | MinimaxKeyframeError::Unsent(_)
            | MinimaxKeyframeError::Unsupported(_) => true,
            // Raised after MiniMax answered, or on a kept answer.
            MinimaxKeyframeError::Fs(_) | MinimaxKeyframeError::Receipt(_) => false,
        }
    }

    fn release(&mut self, request: &CandidateRequest) {
        // Best effort: a receipt or refusal left behind is never read again
        // once its candidate is acknowledged.
        for path in [
            Self::receipt_path(&request.artifact_id),
            Self::refusal_path(&request.artifact_id),
        ]
        .into_iter()
        .flatten()
        {
            let _ = std::fs::remove_file(self.project.root().resolve(&path));
        }
    }
}
