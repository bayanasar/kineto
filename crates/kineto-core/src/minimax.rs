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
//! is the only way back to a finished call. A crash before the receipt is
//! written loses the answer, and the retry pays again.

use std::{error::Error, fmt, num::NonZeroU16};

use kineto_job_runtime::{
    Invocation, JobRuntimePolicy, PaidJobAdapter, ProviderKey, Reconciliation,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, ErrorClass, FallbackPolicy, IdempotencyKey, PaidFallback,
    ProviderExecutionPolicy, ProviderJobId,
};
use kineto_project::{
    ArtifactId, ContentHash, InputHash,
    fs::{ProjectFsError, ProjectRelativePath},
    manifest::CanonicalProject,
    shot::{CandidateOutput, PromptProvenance, keyframe_media_path},
};
use kineto_provider_minimax::{
    AspectRatio, HttpsTransport, IMAGE_MODEL, ImageRequest, MinimaxClient, MinimaxError, PROVIDER,
    Transport,
};
use kineto_providers::ImageFormat;
use serde::{Deserialize, Serialize};

use crate::generation::{
    CandidateGenerator, CandidateRequest, GeneratedCandidate, parse_candidate_key,
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
    Fs(ProjectFsError),
}

impl fmt::Display for MinimaxKeyframeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => error.fmt(formatter),
            Self::NotConfigured => formatter.write_str("no MiniMax API key is configured"),
            Self::MissingPrompt => formatter.write_str("the candidate has no compiled prompt"),
            Self::Receipt(what) => write!(formatter, "MiniMax keyframe receipt: {what}"),
            Self::Fs(error) => error.fmt(formatter),
        }
    }
}

impl Error for MinimaxKeyframeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Provider(error) => Some(error),
            Self::Fs(error) => Some(error),
            Self::NotConfigured | Self::MissingPrompt | Self::Receipt(_) => None,
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
}

/// MiniMax keyframes for one project. Cheap to copy: every candidate of a
/// generation gets its own copy and they run in parallel.
pub struct MinimaxKeyframes<'a, T = HttpsTransport> {
    project: &'a CanonicalProject,
    client: Option<&'a MinimaxClient<T>>,
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
        Self { project, client }
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
        let image = client
            .generate_image(&ImageRequest {
                prompt: prompt.compiled.prompt().to_owned(),
                aspect_ratio: AspectRatio::Landscape16x9,
            })
            .map_err(MinimaxKeyframeError::Provider)?;

        let media_path = keyframe_media_path(
            request.shot_number,
            &request.artifact_id,
            extension(image.format),
        )
        .map_err(|_| MinimaxKeyframeError::Receipt("keyframe path is invalid"))?;
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
            MinimaxKeyframeError::Fs(_) => ErrorClass::Retryable,
            MinimaxKeyframeError::NotConfigured
            | MinimaxKeyframeError::MissingPrompt
            | MinimaxKeyframeError::Receipt(_) => ErrorClass::Terminal,
        }
    }
}

impl<T: Transport + Sync> CandidateGenerator for MinimaxKeyframes<'_, T> {
    fn provider(&self) -> ProviderKey {
        ProviderKey::new(PROVIDER).expect("static provider key is valid")
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

    fn release(&mut self, request: &CandidateRequest) {
        // Best effort: a receipt left behind is never read again once its
        // candidate is acknowledged.
        if let Ok(path) = Self::receipt_path(&request.artifact_id) {
            let _ = std::fs::remove_file(self.project.root().resolve(&path));
        }
    }
}
