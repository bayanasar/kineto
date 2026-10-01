//! Documents a language model writes — the story analysis, the screenplay —
//! as durable write-ahead jobs.
//!
//! A model's answer is only as durable as what is kept of it, as with
//! keyframes: the answer is parsed into its typed document, written as the
//! next revision's file, and recorded in a receipt under `.kineto/generation/`
//! before the job is marked invoked. A restart completes the revision from
//! its receipt and never asks the model again.
//!
//! ```text
//! prepare -> ask (llm.complete) -> document file + receipt -> invoked -> attach revision -> acknowledge
//! ```

use std::{error::Error, fmt, num::NonZeroU16};

use kineto_job_runtime::{
    DispatchOutcome, Invocation, InvokePreparedError, JobRuntime, JobRuntimeError,
    JobRuntimePolicy, PaidJobAdapter, PrepareOutcome, ProviderKey, Reconciliation, StartupOutcome,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, ErrorClass, FallbackPolicy, IdempotencyKey, IntentState, JobId,
    PaidFallback, ProviderExecutionPolicy, ProviderJobId,
};
use kineto_llm::{LlmBackend, LlmRequest, Message, OutputSchema, complete_structured};
use kineto_project::{
    ArtifactDependency, ArtifactId, ContentHash, InputHash, InputHashMaterial,
    fs::{ProjectFsError, ProjectRelativePath},
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, Origin, RevisionRecord, RevisionStatus},
    shot::PromptProvenance,
};
use kineto_prompts::CompiledPrompt;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::generation::{GenerationError, invoke_when_permitted, provenance_of};

/// The capability documents are written with.
pub const LLM_OPERATION: &str = "llm.complete";

/// What to ask for.
#[derive(Debug, Clone)]
pub struct DocumentSpec {
    pub kind: DocumentKind,
    /// Version of the instructions; change it whenever they change.
    pub compiler_version: &'static str,
    /// The instructions, stored verbatim with every revision.
    pub instructions: String,
    /// What the instructions apply to, such as the source text. Hashed into
    /// the provenance but not stored with it; `input_label` stands in.
    pub input: String,
    pub input_label: String,
    pub schema_name: &'static str,
    pub schema_json: &'static str,
    /// What the document is made from.
    pub dependencies: Vec<ArtifactDependency>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct DocumentJobError {
    message: String,
    class: ErrorClass,
}

impl fmt::Display for DocumentJobError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for DocumentJobError {}

impl DocumentJobError {
    fn terminal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            class: ErrorClass::Terminal,
        }
    }
}

fn receipt_path(artifact_id: &ArtifactId) -> Result<ProjectRelativePath, DocumentJobError> {
    ProjectRelativePath::new(format!(".kineto/generation/{}.json", artifact_id.as_str()))
        .map_err(|_| DocumentJobError::terminal("artifact ID is not a valid file name"))
}

fn read_receipt(
    project: &CanonicalProject,
    artifact_id: &ArtifactId,
) -> Result<Option<(DocumentKind, RevisionRecord)>, DocumentJobError> {
    let bytes = match project.root().read(&receipt_path(artifact_id)?) {
        Ok(bytes) => bytes,
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => {
            return Err(DocumentJobError {
                message: error.to_string(),
                class: ErrorClass::Retryable,
            });
        }
    };
    let (kind, record): (DocumentKind, RevisionRecord) = serde_json::from_slice(&bytes)
        .map_err(|_| DocumentJobError::terminal("document receipt is not readable"))?;
    if record.artifact_id != artifact_id.as_str() {
        return Err(DocumentJobError::terminal(
            "document receipt belongs to another revision",
        ));
    }
    // The receipt is only as good as the file it names.
    let path = ProjectRelativePath::new(&record.path)
        .map_err(|_| DocumentJobError::terminal("receipt names an invalid path"))?;
    let file = project
        .root()
        .read(&path)
        .map_err(|_| DocumentJobError::terminal("the receipt's document is missing"))?;
    if ContentHash::of(&file).as_str() != record.content_hash {
        return Err(DocumentJobError::terminal(
            "the receipt's document was changed",
        ));
    }
    Ok(Some((kind, record)))
}

fn remove_receipt(project: &CanonicalProject, artifact_id: &ArtifactId) {
    if let Ok(path) = receipt_path(artifact_id) {
        let _ = std::fs::remove_file(project.root().resolve(&path));
    }
}

fn policy() -> JobRuntimePolicy {
    JobRuntimePolicy {
        execution: ProviderExecutionPolicy::new(
            NonZeroU16::new(1).expect("static concurrency is non-zero"),
            BackoffPolicy::new(2_000, 60_000, 3).expect("static backoff policy is valid"),
        ),
        // A document is an explicit request; nothing falls back to a paid model.
        fallback_policy: FallbackPolicy {
            paid_fallback: PaidFallback::Forbidden,
            max_paid_amount_micros: Some(0),
            require_user_approval: true,
        },
    }
}

/// One planned revision, with what its record needs.
struct Planned {
    kind: DocumentKind,
    revision: u32,
    artifact_id: ArtifactId,
    path: ProjectRelativePath,
    input_hash: InputHash,
    provider: String,
    model: String,
    provenance: PromptProvenance,
    dependencies: Vec<ArtifactDependency>,
}

/// Asks the model and keeps its answer, or completes from a receipt.
struct DocumentAdapter<'a, B, T> {
    project: &'a CanonicalProject,
    backend: Option<&'a B>,
    planned: Option<&'a Planned>,
    _document: std::marker::PhantomData<fn() -> T>,
}

impl<B: LlmBackend, T: Serialize + DeserializeOwned> PaidJobAdapter for DocumentAdapter<'_, B, T> {
    type Request = LlmRequest;
    type Output = (DocumentKind, RevisionRecord);
    type Error = DocumentJobError;

    fn estimate_cost(&self, request: &LlmRequest) -> Result<CostEstimate, DocumentJobError> {
        let backend = self
            .backend
            .ok_or_else(|| DocumentJobError::terminal("no model is configured"))?;
        kineto_llm::estimate_cost(&backend.info(), request, 4_000)
            .ok_or_else(|| DocumentJobError::terminal("the model's cost is not known"))
    }

    fn invoke(
        &mut self,
        request: &LlmRequest,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<Self::Output>, DocumentJobError> {
        let (Some(backend), Some(planned)) = (self.backend, self.planned) else {
            return Err(DocumentJobError::terminal("no model is configured"));
        };
        let structured =
            complete_structured::<B, T>(backend, request).map_err(|error| DocumentJobError {
                class: error.class(),
                message: error.to_string(),
            })?;
        let bytes = revisions::encode(&structured.value)
            .map_err(|error| DocumentJobError::terminal(error.to_string()))?;
        let next = revisions::revision_at(planned.kind, planned.revision)
            .map_err(|error| DocumentJobError::terminal(error.to_string()))?;
        let content_hash =
            revisions::write_document(self.project, &next, &bytes).map_err(|error| {
                DocumentJobError {
                    message: error.to_string(),
                    class: ErrorClass::Retryable,
                }
            })?;
        let record = RevisionRecord {
            revision: planned.revision,
            artifact_id: planned.artifact_id.as_str().to_owned(),
            path: planned.path.as_path().to_string_lossy().replace('\\', "/"),
            status: RevisionStatus::Candidate,
            origin: Origin::Generated,
            content_hash: content_hash.as_str().to_owned(),
            input_hash: Some(planned.input_hash.as_str().to_owned()),
            provider: Some(planned.provider.clone()),
            // The model that actually answered, when the backend says.
            model: Some(if structured.response.model.trim().is_empty() {
                planned.model.clone()
            } else {
                structured.response.model.clone()
            }),
            provenance: Some(planned.provenance.clone()),
            based_on: None,
            dependencies: planned
                .dependencies
                .iter()
                .map(revisions::DependencyRecord::from)
                .collect(),
        };
        let mut receipt = serde_json::to_vec_pretty(&(planned.kind, &record))
            .map_err(|_| DocumentJobError::terminal("receipt did not encode"))?;
        receipt.push(b'\n');
        self.project
            .root()
            .write_atomic(&receipt_path(&planned.artifact_id)?, &receipt)
            .map_err(|error| DocumentJobError {
                message: error.to_string(),
                class: ErrorClass::Retryable,
            })?;
        Ok(Invocation::Completed((planned.kind, record)))
    }

    fn reconcile(
        &mut self,
        _id: &ProviderJobId,
    ) -> Result<Reconciliation<Self::Output>, DocumentJobError> {
        Ok(Reconciliation::NotFound)
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        key: &IdempotencyKey,
    ) -> Result<Reconciliation<Self::Output>, DocumentJobError> {
        // `llm.complete:{artifact_id}:{input_hash}`
        let artifact_id = key
            .as_str()
            .strip_prefix(LLM_OPERATION)
            .and_then(|rest| rest.strip_prefix(':'))
            .and_then(|rest| rest.split_once(':'))
            .and_then(|(artifact_id, _)| ArtifactId::new(artifact_id).ok())
            .ok_or_else(|| DocumentJobError::terminal("not a document job key"))?;
        Ok(match read_receipt(self.project, &artifact_id)? {
            Some(output) => Reconciliation::Completed {
                output,
                provider_job_id: None,
            },
            None => Reconciliation::NotFound,
        })
    }

    fn classify_error(&self, error: &DocumentJobError) -> ErrorClass {
        error.class
    }
}

fn framed(parts: &[&[u8]]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(
            u64::try_from(part.len())
                .expect("length fits in u64")
                .to_be_bytes(),
        );
        hasher.update(part);
    }
    hasher.finalize().to_vec()
}

fn attach_and_acknowledge(
    project: &CanonicalProject,
    runtime: &JobRuntime<'_>,
    job_id: &JobId,
    kind: DocumentKind,
    record: RevisionRecord,
) -> Result<RevisionRecord, GenerationError> {
    let artifact_id = ArtifactId::new(record.artifact_id.clone())
        .map_err(|_| GenerationError::Unexpected("a revision has an invalid artifact ID"))?;
    revisions::attach(project, kind, record.clone()).map_err(document_error)?;
    let _ = runtime.acknowledge_result(job_id)?;
    remove_receipt(project, &artifact_id);
    Ok(record)
}

fn document_error(error: impl Error + Send + Sync + 'static) -> GenerationError {
    GenerationError::Provider {
        source: Box::new(error),
        class: ErrorClass::Terminal,
    }
}

/// Ask `backend` for the next revision of a document and keep it.
pub fn generate<B: LlmBackend, T: Serialize + DeserializeOwned>(
    project: &CanonicalProject,
    backend: &B,
    spec: DocumentSpec,
) -> Result<RevisionRecord, GenerationError> {
    let info = backend.info();
    let provider = ProviderKey::new(info.provider.clone())
        .map_err(|_| GenerationError::Unexpected("the model's provider key is not portable"))?;
    let next = revisions::next(project, spec.kind).map_err(document_error)?;

    let compiled = CompiledPrompt::new(
        format!("{}\n\n{}", spec.instructions, spec.input_label),
        Vec::new(),
        Vec::new(),
        spec.compiler_version,
    )
    .map_err(|_| GenerationError::Unexpected("instructions have no compiler version"))?;
    let intent = framed(&[
        spec.instructions.as_bytes(),
        spec.input.as_bytes(),
        spec.schema_json.as_bytes(),
    ]);
    let prompt_hash = compiled.prompt_hash(&intent);
    let params = format!("max_output_tokens={:?}", spec.max_output_tokens);
    let input_hash = InputHash::derive(InputHashMaterial {
        dependencies: &spec.dependencies,
        canonical_intent: &intent,
        provider: &info.provider,
        model: &info.model,
        canonical_params: params.as_bytes(),
        compiler_version: spec.compiler_version,
    });
    let planned = Planned {
        kind: spec.kind,
        revision: next.revision,
        artifact_id: next.artifact_id.clone(),
        path: next.path.clone(),
        input_hash: input_hash.clone(),
        provider: info.provider.clone(),
        model: info.model.clone(),
        provenance: provenance_of(&compiled, &prompt_hash),
        dependencies: spec.dependencies.clone(),
    };
    let request = LlmRequest {
        messages: vec![
            Message::system(spec.instructions),
            Message::user(spec.input),
        ],
        schema: Some(OutputSchema {
            name: spec.schema_name.to_owned(),
            schema_json: spec.schema_json.to_owned(),
        }),
        max_output_tokens: spec.max_output_tokens,
    };

    let runtime = JobRuntime::new(project, provider, policy());
    let job_id = JobId::new(next.artifact_id.as_str())?;
    // Asking again is what retries a request that failed.
    runtime.retire_failed(&job_id)?;
    let state = match runtime.prepare(
        job_id.clone(),
        LLM_OPERATION,
        input_hash,
        next.artifact_id.clone(),
    ) {
        Ok(PrepareOutcome::Created(record) | PrepareOutcome::Existing(record)) => {
            record.intent().state()
        }
        Err(error) => return Err(error.into()),
    };
    let mut adapter = DocumentAdapter::<B, T> {
        project,
        backend: Some(backend),
        planned: Some(&planned),
        _document: std::marker::PhantomData,
    };
    let (kind, record) = match state {
        IntentState::Prepared => {
            match invoke_when_permitted(&runtime, &job_id, &mut adapter, &request) {
                Ok(DispatchOutcome::Completed { output, .. }) => output,
                Ok(DispatchOutcome::Pending { .. }) => {
                    return Err(GenerationError::Unexpected(
                        "a model answer was left pending",
                    ));
                }
                Err(InvokePreparedError::Runtime(error)) => return Err(error.into()),
                Err(InvokePreparedError::Provider { source, class, .. }) => {
                    return Err(GenerationError::Provider {
                        source: Box::new(source),
                        class,
                    });
                }
            }
        }
        // Answered before an interruption; keep that answer.
        IntentState::Invoked => read_receipt(project, &next.artifact_id)
            .map_err(document_error)?
            .ok_or(GenerationError::Unexpected(
                "an answered document has no receipt",
            ))?,
        IntentState::Reconciled => {
            return Err(GenerationError::Unexpected(
                "this revision was already recorded",
            ));
        }
    };
    attach_and_acknowledge(project, &runtime, &job_id, kind, record)
}

/// Complete documents whose answers arrived before an interruption. Never
/// asks a model. Returns the revisions completed and how many records need a
/// person.
pub fn recover(
    project: &CanonicalProject,
    provider: &str,
) -> Result<(Vec<RevisionRecord>, usize), JobRuntimeError> {
    let Ok(key) = ProviderKey::new(provider) else {
        return Ok((Vec::new(), 0));
    };
    let runtime = JobRuntime::new(project, key, policy());
    let mut adapter = DocumentAdapter::<NoBackend, serde_json::Value> {
        project,
        backend: None,
        planned: None,
        _document: std::marker::PhantomData,
    };
    let mut completed = Vec::new();
    let mut attention = 0;
    for outcome in runtime.reconcile_startup(&mut adapter)? {
        match outcome {
            StartupOutcome::ResultReady {
                job_id,
                output: (kind, record),
            } => match attach_and_acknowledge(project, &runtime, &job_id, kind, record) {
                Ok(record) => completed.push(record),
                Err(_) => attention += 1,
            },
            // Never answered: the director asks again when they want it.
            StartupOutcome::RetryPrepared { .. } => {}
            _ => attention += 1,
        }
    }
    Ok((completed, attention))
}

/// Stands in for a model during recovery, which never asks one.
struct NoBackend;

impl LlmBackend for NoBackend {
    type Error = DocumentJobError;

    fn info(&self) -> kineto_llm::LlmBackendInfo {
        unreachable!("recovery never consults a model")
    }

    fn complete(&self, _request: &LlmRequest) -> Result<kineto_llm::LlmResponse, DocumentJobError> {
        Err(DocumentJobError::terminal("recovery never asks a model"))
    }

    fn classify(&self, error: &DocumentJobError) -> ErrorClass {
        error.class
    }
}

/// Every provider key a document job may be recorded under.
pub const DOCUMENT_PROVIDERS: [&str; 3] = [
    kineto_llm::openai::KIMI_PROVIDER,
    kineto_llm::acp::KIMI_CODE_PROVIDER,
    kineto_llm::openai::LOCAL_PROVIDER,
];
