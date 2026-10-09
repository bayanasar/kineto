//! Documents a language model writes — the story analysis, the screenplay —
//! as durable write-ahead jobs.
//!
//! A model's answer is only as durable as what is kept of it, as with
//! keyframes: the answer is parsed into its typed document, written as the
//! next revision's file, and recorded in a receipt under `.kineto/generation/`
//! before the job is marked invoked. A restart completes the revision from
//! its receipt and never asks the model again.
//!
//! A model whose calls are charged one by one is never asked twice for one
//! answer unasked. The attempt is recorded before the call; a call that
//! failed without an answer leaves a refusal record beside the receipts. An
//! attempt with neither, such as one cut off by a crash after the model
//! answered, is unresolved: the next request is refused until the director
//! chooses to risk paying twice ([`risk_paying_twice`]).
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
use kineto_llm::agents::AgentKind;
use kineto_llm::{
    Billing, LlmBackend, LlmBackendInfo, LlmRequest, Message, OutputSchema, StructuredError,
    complete_checked,
};
use kineto_project::{
    ArtifactDependency, ArtifactId, ContentHash, InputHash, InputHashMaterial,
    fs::{ProjectFsError, ProjectRelativePath},
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, Origin, RevisionRecord, RevisionStatus},
    shot::PromptProvenance,
};
use kineto_prompts::CompiledPrompt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
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
    /// The revision this one reworks, such as a screenplay one scene of
    /// which is written again.
    pub based_on: Option<u32>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct DocumentJobError {
    message: String,
    class: ErrorClass,
    /// The model may have answered, and charged for it, without the answer
    /// arriving.
    answer_lost: bool,
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
            answer_lost: false,
        }
    }

    fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            class: ErrorClass::Retryable,
            answer_lost: false,
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
        Err(error) => return Err(DocumentJobError::retryable(error.to_string())),
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

/// Proof that the latest call of a document job left no answer behind: it
/// failed before the model answered, or its answer arrived and was refused.
#[derive(Debug, Serialize, Deserialize)]
struct Refusal {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
}

fn refusal_path(artifact_id: &ArtifactId) -> Result<ProjectRelativePath, DocumentJobError> {
    ProjectRelativePath::new(format!(
        ".kineto/generation/{}.refused.json",
        artifact_id.as_str()
    ))
    .map_err(|_| DocumentJobError::terminal("artifact ID is not a valid file name"))
}

fn write_refusal(project: &CanonicalProject, planned: &Planned) -> Result<(), DocumentJobError> {
    let mut bytes = serde_json::to_vec_pretty(&Refusal {
        schema_version: 1,
        artifact_id: planned.artifact_id.as_str().to_owned(),
        input_hash: planned.input_hash.as_str().to_owned(),
    })
    .map_err(|_| DocumentJobError::terminal("refusal did not encode"))?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(&refusal_path(&planned.artifact_id)?, &bytes)
        .map_err(|error| DocumentJobError::retryable(error.to_string()))
}

/// Whether the latest call for `artifact_id` at `input_hash` certainly left
/// no answer behind.
fn refused(project: &CanonicalProject, artifact_id: &ArtifactId, input_hash: &InputHash) -> bool {
    refusal_path(artifact_id)
        .ok()
        .and_then(|path| project.root().read(&path).ok())
        .and_then(|bytes| serde_json::from_slice::<Refusal>(&bytes).ok())
        .is_some_and(|refusal| {
            refusal.artifact_id == artifact_id.as_str() && refusal.input_hash == input_hash.as_str()
        })
}

/// Remove the refusal record of `artifact_id`, which explained an attempt
/// that is now retired or about to be made again. Failing to remove it is
/// an error: a stale record could later pass an unresolved call as refused.
fn forget_refusal(
    project: &CanonicalProject,
    artifact_id: &ArtifactId,
) -> Result<(), GenerationError> {
    let path = refusal_path(artifact_id).map_err(document_error)?;
    match std::fs::remove_file(project.root().resolve(&path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(GenerationError::Provider {
            source: Box::new(error),
            class: ErrorClass::Retryable,
        }),
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
    based_on: Option<u32>,
}

/// An accepted answer, encoded as the document it becomes.
struct Answer {
    bytes: Vec<u8>,
    /// The model that actually answered, when the backend says.
    model: String,
}

type Ask<'a> = dyn Fn(&LlmRequest) -> Result<Answer, DocumentJobError> + 'a;

/// What asking needs; absent during recovery, which never asks.
struct Asker<'a> {
    info: LlmBackendInfo,
    planned: &'a Planned,
    ask: &'a Ask<'a>,
}

/// Asks the model and keeps its answer, or completes from a receipt.
struct DocumentAdapter<'a> {
    project: &'a CanonicalProject,
    asker: Option<Asker<'a>>,
}

impl PaidJobAdapter for DocumentAdapter<'_> {
    type Request = LlmRequest;
    type Output = (DocumentKind, RevisionRecord);
    type Error = DocumentJobError;

    fn estimate_cost(&self, request: &LlmRequest) -> Result<CostEstimate, DocumentJobError> {
        let asker = self
            .asker
            .as_ref()
            .ok_or_else(|| DocumentJobError::terminal("no model is configured"))?;
        let output = request.max_output_tokens.map_or(4_000, u64::from);
        kineto_llm::estimate_cost(&asker.info, request, output)
            .ok_or_else(|| DocumentJobError::terminal("the model's cost is not known"))
    }

    fn invoke(
        &mut self,
        request: &LlmRequest,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<Self::Output>, DocumentJobError> {
        let Some(asker) = &self.asker else {
            return Err(DocumentJobError::terminal("no model is configured"));
        };
        let planned = asker.planned;
        let answer = match (asker.ask)(request) {
            Ok(answer) => answer,
            Err(error) => {
                // A failure that left no answer can be asked again without
                // paying twice; without this record it counts as unresolved.
                if !error.answer_lost {
                    let _ = write_refusal(self.project, planned);
                }
                return Err(error);
            }
        };
        let bytes = answer.bytes;
        let next = revisions::revision_at(planned.kind, planned.revision)
            .map_err(|error| DocumentJobError::terminal(error.to_string()))?;
        let content_hash = revisions::write_document(self.project, &next, &bytes)
            .map_err(|error| DocumentJobError::retryable(error.to_string()))?;
        let record = RevisionRecord {
            revision: planned.revision,
            artifact_id: planned.artifact_id.as_str().to_owned(),
            path: planned.path.as_path().to_string_lossy().replace('\\', "/"),
            status: RevisionStatus::Candidate,
            origin: Origin::Generated,
            content_hash: content_hash.as_str().to_owned(),
            input_hash: Some(planned.input_hash.as_str().to_owned()),
            provider: Some(planned.provider.clone()),
            model: Some(if answer.model.trim().is_empty() {
                planned.model.clone()
            } else {
                answer.model
            }),
            provenance: Some(planned.provenance.clone()),
            based_on: planned.based_on,
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
            .map_err(|error| DocumentJobError::retryable(error.to_string()))?;
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

pub(crate) fn document_error(error: impl Error + Send + Sync + 'static) -> GenerationError {
    GenerationError::Provider {
        source: Box::new(error),
        class: ErrorClass::Terminal,
    }
}

fn request_for(spec: &DocumentSpec) -> LlmRequest {
    LlmRequest {
        messages: vec![
            Message::system(spec.instructions.clone()),
            Message::user(spec.input.clone()),
        ],
        schema: Some(OutputSchema {
            name: spec.schema_name.to_owned(),
            schema_json: spec.schema_json.to_owned(),
        }),
        max_output_tokens: spec.max_output_tokens,
    }
}

/// What asking for `spec` would cost with `info`, in USD micros, counting
/// its full output allowance; `None` when the cost is not known, as for an
/// agent on a subscription.
#[must_use]
pub fn estimate_micros(info: &LlmBackendInfo, spec: &DocumentSpec) -> Option<u64> {
    let output = spec.max_output_tokens.map_or(4_000, u64::from);
    kineto_llm::estimate_cost(info, &request_for(spec), output).map(|cost| cost.amount_micros)
}

/// Ask `backend` for the next revision of a document and keep it. The answer
/// is parsed as `T` and turned into the stored document by `accept`; an
/// answer either step refuses is asked for once more with the reason.
pub fn generate<B: LlmBackend, T: DeserializeOwned, D: Serialize>(
    project: &CanonicalProject,
    backend: &B,
    spec: DocumentSpec,
    accept: impl Fn(T) -> Result<D, String>,
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
        based_on: spec.based_on,
    };
    let request = request_for(&spec);

    let runtime = JobRuntime::new(project, provider, policy());
    let job_id = JobId::new(next.artifact_id.as_str())?;
    settle_earlier_attempt(project, &runtime, &job_id, &next.artifact_id, info.billing)?;
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
    let ask = |request: &LlmRequest| {
        let structured =
            complete_checked::<B, T, D>(backend, request, &accept).map_err(|error| {
                DocumentJobError {
                    class: error.class(),
                    answer_lost: match &error {
                        StructuredError::Backend { source, .. } => {
                            backend.may_have_lost_an_answer(source)
                        }
                        StructuredError::Schema { .. } | StructuredError::UnsupportedSchema(_) => {
                            false
                        }
                    },
                    message: error.to_string(),
                }
            })?;
        Ok(Answer {
            bytes: revisions::encode(&structured.value)
                .map_err(|error| DocumentJobError::terminal(error.to_string()))?,
            model: structured.response.model,
        })
    };
    let mut adapter = DocumentAdapter {
        project,
        asker: Some(Asker {
            info,
            planned: &planned,
            ask: &ask,
        }),
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

/// Make way for asking for `job_id` again. An earlier attempt that may have
/// been answered, and charged for, without its answer being kept is refused
/// as unresolved; one that failed is retired. An attempt made with another
/// provider, as when the director chose another model after it, is settled
/// the same way; whether that provider charges per call is not known here,
/// so it is taken to.
fn settle_earlier_attempt(
    project: &CanonicalProject,
    runtime: &JobRuntime<'_>,
    job_id: &JobId,
    artifact_id: &ArtifactId,
    billing: Billing,
) -> Result<(), GenerationError> {
    if let Some(record) = runtime.load_recorded(job_id)?
        && (billing.charged_per_call() || record.provider() != runtime.provider())
        && record.intent().state() == IntentState::Prepared
        && record.attempts() > 0
        && !refused(project, artifact_id, record.intent().input_hash())
    {
        return Err(GenerationError::UnresolvedSubmission);
    }
    // Asking again is what retries a request that failed.
    runtime.retire_failed(job_id)?;
    forget_refusal(project, artifact_id)
}

/// Give up on the last request for the next revision of `kind`, made with
/// `provider`, although the model may have answered it, and charged for it,
/// without the answer being kept: asking again may pay twice. Only on the
/// director's explicit decision. Returns whether there was such a request.
pub fn risk_paying_twice(
    project: &CanonicalProject,
    provider: &str,
    kind: DocumentKind,
) -> Result<bool, GenerationError> {
    let Ok(key) = ProviderKey::new(provider) else {
        return Ok(false);
    };
    let next = revisions::next(project, kind).map_err(document_error)?;
    let job_id = JobId::new(next.artifact_id.as_str())?;
    let retired = JobRuntime::new(project, key, policy()).retire_unresolved(&job_id)?;
    forget_refusal(project, &next.artifact_id)?;
    Ok(retired)
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
    let mut adapter = DocumentAdapter {
        project,
        asker: None,
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

/// Every provider key a document job may be recorded under: each API and
/// each agent CLI.
pub const DOCUMENT_PROVIDERS: [&str; 7] = [
    kineto_llm::openai::KIMI_PROVIDER,
    kineto_llm::openai::LOCAL_PROVIDER,
    AgentKind::ClaudeCode.name(),
    AgentKind::Codex.name(),
    AgentKind::KimiCode.name(),
    AgentKind::Agy.name(),
    AgentKind::MiniAgent.name(),
];
