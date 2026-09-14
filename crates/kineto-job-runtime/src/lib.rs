use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    num::NonZeroU16,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard, OnceLock, Weak},
};

use kineto_jobs::{
    BackoffPolicy, CostEstimate, CurrencyCode, ErrorClass, FallbackPolicy, IdempotencyKey,
    IntentState, JobId, JobIntent, JobIntentError, JobValueError, ProviderExecutionPolicy,
    ProviderJobId,
};
use kineto_project::{
    ArtifactId, ArtifactIdError, HashValueError, InputHash,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::CanonicalProject,
};
use serde::{Deserialize, Serialize};

const RUNTIME_INTENT_DIR: &str = ".kineto/jobs/intents";
const RUNTIME_INTENT_SCHEMA_VERSION: u32 = 1;
const MAX_PROVIDER_KEY_LEN: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderKey(String);

impl ProviderKey {
    pub fn new(value: impl Into<String>) -> Result<Self, ProviderKeyError> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_PROVIDER_KEY_LEN
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            Err(ProviderKeyError)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderKeyError;

impl fmt::Display for ProviderKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("provider key must be bounded portable ASCII")
    }
}

impl Error for ProviderKeyError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeIntent {
    provider: ProviderKey,
    intent: JobIntent,
    attempts: u16,
    last_error_class: Option<ErrorClass>,
}

impl RuntimeIntent {
    #[must_use]
    pub fn provider(&self) -> &ProviderKey {
        &self.provider
    }

    #[must_use]
    pub fn intent(&self) -> &JobIntent {
        &self.intent
    }

    #[must_use]
    pub const fn attempts(&self) -> u16 {
        self.attempts
    }

    #[must_use]
    pub const fn last_error_class(&self) -> Option<ErrorClass> {
        self.last_error_class
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareOutcome {
    Created(RuntimeIntent),
    Existing(RuntimeIntent),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation<T> {
    Completed(T),
    Pending(ProviderJobId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reconciliation<T> {
    Completed {
        output: T,
        provider_job_id: Option<ProviderJobId>,
    },
    Pending {
        provider_job_id: Option<ProviderJobId>,
    },
    NotFound,
}

pub trait PaidJobAdapter {
    type Request;
    type Output;
    type Error: Error + Send + Sync + 'static;

    fn estimate_cost(&self, request: &Self::Request) -> Result<CostEstimate, Self::Error>;

    fn invoke(
        &mut self,
        request: &Self::Request,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<Self::Output>, Self::Error>;

    fn reconcile(
        &mut self,
        provider_job_id: &ProviderJobId,
    ) -> Result<Reconciliation<Self::Output>, Self::Error>;

    fn reconcile_by_idempotency_key(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Reconciliation<Self::Output>, Self::Error>;

    fn classify_error(&self, error: &Self::Error) -> ErrorClass;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobRuntimePolicy {
    pub execution: ProviderExecutionPolicy,
    pub fallback_policy: FallbackPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDisposition {
    AfterMs(u64),
    Exhausted,
    Never,
}

#[derive(Debug)]
pub enum InvokePreparedError<E> {
    Runtime(JobRuntimeError),
    Provider {
        source: E,
        class: ErrorClass,
        retry: RetryDisposition,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchOutcome<T> {
    Completed {
        job_id: JobId,
        output: T,
    },
    Pending {
        job_id: JobId,
        provider_job_id: ProviderJobId,
    },
}

#[derive(Debug)]
pub enum StartupOutcome<T, E> {
    ResultReady {
        job_id: JobId,
        output: T,
    },
    StillPending {
        job_id: JobId,
    },
    RetryPrepared {
        job_id: JobId,
        retry: RetryDisposition,
    },
    NeedsAttention {
        job_id: JobId,
    },
    ProviderError {
        job_id: JobId,
        source: E,
        class: ErrorClass,
        retry: RetryDisposition,
    },
}

pub type StartupOutcomes<A> = Result<
    Vec<StartupOutcome<<A as PaidJobAdapter>::Output, <A as PaidJobAdapter>::Error>>,
    JobRuntimeError,
>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DryRunSummary {
    pub request_count: u32,
    pub call_count: u32,
    pub amount_micros: u64,
    pub currency: Option<CurrencyCode>,
    pub estimated_duration_ms: Option<u64>,
}

#[derive(Debug)]
pub enum DryRunError<E> {
    Provider(E),
    CurrencyMismatch,
    Overflow,
}

pub struct JobRuntime<'project> {
    project: &'project CanonicalProject,
    provider: ProviderKey,
    policy: JobRuntimePolicy,
    limiter: ProviderLimiter,
}

impl<'project> JobRuntime<'project> {
    #[must_use]
    pub fn new(
        project: &'project CanonicalProject,
        provider: ProviderKey,
        policy: JobRuntimePolicy,
    ) -> Self {
        let limiter = ProviderLimiter::new(&provider, policy.execution.max_concurrency());
        Self {
            project,
            provider,
            policy,
            limiter,
        }
    }

    #[must_use]
    pub fn provider(&self) -> &ProviderKey {
        &self.provider
    }

    #[must_use]
    pub const fn policy(&self) -> JobRuntimePolicy {
        self.policy
    }

    /// Returns the strictest concurrency ceiling registered by any live
    /// runtime for this provider in the current process.
    #[must_use]
    pub fn effective_max_concurrency(&self) -> NonZeroU16 {
        self.limiter.effective_max()
    }

    pub fn prepare(
        &self,
        job_id: JobId,
        operation: impl Into<String>,
        input_hash: InputHash,
        artifact_id: Option<ArtifactId>,
    ) -> Result<PrepareOutcome, JobRuntimeError> {
        self.ensure_writable()?;
        let operation = operation.into();
        let idempotency_key = derive_idempotency_key(&operation, &input_hash)?;
        let store = IntentStore::new(self.project);

        if let Some(existing) = store.find_by_idempotency(&self.provider, &idempotency_key)? {
            return Ok(PrepareOutcome::Existing(existing));
        }

        let intent = JobIntent::new(job_id, operation, idempotency_key, input_hash, artifact_id)?;
        let record = RuntimeIntent {
            provider: self.provider.clone(),
            intent,
            attempts: 0,
            last_error_class: None,
        };
        store.save(&record)?;
        Ok(PrepareOutcome::Created(record))
    }

    pub fn invoke_prepared<A: PaidJobAdapter>(
        &self,
        job_id: &JobId,
        adapter: &mut A,
        request: &A::Request,
    ) -> Result<DispatchOutcome<A::Output>, InvokePreparedError<A::Error>> {
        self.ensure_writable()
            .map_err(InvokePreparedError::Runtime)?;
        let _permit = self
            .limiter
            .try_acquire()
            .ok_or(InvokePreparedError::Runtime(
                JobRuntimeError::ConcurrencyLimitReached,
            ))?;
        let store = IntentStore::new(self.project);
        let mut record = store.load(job_id).map_err(InvokePreparedError::Runtime)?;
        self.ensure_provider(&record)
            .map_err(InvokePreparedError::Runtime)?;

        if record.intent.state() != IntentState::Prepared {
            return Err(InvokePreparedError::Runtime(
                JobRuntimeError::IntentNotPrepared,
            ));
        }
        if record.last_error_class == Some(ErrorClass::Terminal) {
            return Err(InvokePreparedError::Runtime(
                JobRuntimeError::PreviousTerminalFailure,
            ));
        }

        let max_attempts = self.policy.execution.backoff().max_attempts();
        if record.attempts >= max_attempts {
            return Err(InvokePreparedError::Runtime(
                JobRuntimeError::AttemptsExhausted,
            ));
        }

        record.attempts = record
            .attempts
            .checked_add(1)
            .ok_or(InvokePreparedError::Runtime(JobRuntimeError::Overflow))?;
        record.last_error_class = None;
        store.save(&record).map_err(InvokePreparedError::Runtime)?;

        let invocation = match adapter.invoke(request, record.intent.idempotency_key()) {
            Ok(invocation) => invocation,
            Err(source) => {
                let class = adapter.classify_error(&source);
                record.last_error_class = Some(class);
                store.save(&record).map_err(InvokePreparedError::Runtime)?;
                let retry =
                    retry_disposition(class, record.attempts, self.policy.execution.backoff());
                return Err(InvokePreparedError::Provider {
                    source,
                    class,
                    retry,
                });
            }
        };

        record.last_error_class = None;
        match invocation {
            Invocation::Completed(output) => {
                record.intent.mark_invoked(None)?;
                store.save(&record).map_err(InvokePreparedError::Runtime)?;
                Ok(DispatchOutcome::Completed {
                    job_id: record.intent.job_id().clone(),
                    output,
                })
            }
            Invocation::Pending(provider_job_id) => {
                record.intent.mark_invoked(Some(provider_job_id.clone()))?;
                store.save(&record).map_err(InvokePreparedError::Runtime)?;
                Ok(DispatchOutcome::Pending {
                    job_id: record.intent.job_id().clone(),
                    provider_job_id,
                })
            }
        }
    }

    /// Mark an invoked intent reconciled only after the caller has durably
    /// attached the provider result to canonical project state.
    pub fn acknowledge_result(&self, job_id: &JobId) -> Result<RuntimeIntent, JobRuntimeError> {
        self.ensure_writable()?;
        let store = IntentStore::new(self.project);
        let mut record = store.load(job_id)?;
        self.ensure_provider(&record)?;
        record.intent.mark_reconciled()?;
        store.save(&record)?;
        Ok(record)
    }

    pub fn reconcile_startup<A: PaidJobAdapter>(&self, adapter: &mut A) -> StartupOutcomes<A> {
        self.ensure_writable()?;
        let store = IntentStore::new(self.project);
        let records = store.load_all()?;
        let mut outcomes = Vec::new();

        for mut record in records.into_iter().filter(|record| {
            record.provider == self.provider && record.intent.state() != IntentState::Reconciled
        }) {
            let job_id = record.intent.job_id().clone();
            let reconciliation = match record.intent.state() {
                IntentState::Prepared => {
                    adapter.reconcile_by_idempotency_key(record.intent.idempotency_key())
                }
                IntentState::Invoked => match record.intent.provider_job_id() {
                    Some(provider_job_id) => adapter.reconcile(provider_job_id),
                    None => adapter.reconcile_by_idempotency_key(record.intent.idempotency_key()),
                },
                IntentState::Reconciled => continue,
            };

            let reconciliation = match reconciliation {
                Ok(reconciliation) => reconciliation,
                Err(source) => {
                    let class = adapter.classify_error(&source);
                    let retry =
                        retry_disposition(class, record.attempts, self.policy.execution.backoff());
                    outcomes.push(StartupOutcome::ProviderError {
                        job_id,
                        source,
                        class,
                        retry,
                    });
                    continue;
                }
            };

            match reconciliation {
                Reconciliation::Completed {
                    output,
                    provider_job_id,
                } => {
                    if record.intent.state() == IntentState::Prepared {
                        record.intent.mark_invoked(provider_job_id)?;
                        record.last_error_class = None;
                        store.save(&record)?;
                    }
                    outcomes.push(StartupOutcome::ResultReady { job_id, output });
                }
                Reconciliation::Pending { provider_job_id } => {
                    if record.intent.state() == IntentState::Prepared {
                        record.intent.mark_invoked(provider_job_id)?;
                        record.last_error_class = None;
                        store.save(&record)?;
                    }
                    outcomes.push(StartupOutcome::StillPending { job_id });
                }
                Reconciliation::NotFound => {
                    if record.intent.state() == IntentState::Prepared {
                        let retry = match record.last_error_class {
                            Some(ErrorClass::Terminal) => RetryDisposition::Never,
                            _ if record.attempts == 0 => RetryDisposition::AfterMs(0),
                            _ => retry_disposition(
                                ErrorClass::Retryable,
                                record.attempts,
                                self.policy.execution.backoff(),
                            ),
                        };
                        outcomes.push(StartupOutcome::RetryPrepared { job_id, retry });
                    } else {
                        outcomes.push(StartupOutcome::NeedsAttention { job_id });
                    }
                }
            }
        }

        Ok(outcomes)
    }

    pub fn dry_run<A: PaidJobAdapter>(
        &self,
        adapter: &A,
        requests: &[A::Request],
    ) -> Result<DryRunSummary, DryRunError<A::Error>> {
        let mut request_count = 0_u32;
        let mut call_count = 0_u32;
        let mut amount_micros = 0_u64;
        let mut currency = None;
        let mut duration = Some(0_u64);

        for request in requests {
            let estimate = adapter
                .estimate_cost(request)
                .map_err(DryRunError::Provider)?;
            request_count = request_count.checked_add(1).ok_or(DryRunError::Overflow)?;
            call_count = call_count
                .checked_add(estimate.call_count)
                .ok_or(DryRunError::Overflow)?;
            amount_micros = amount_micros
                .checked_add(estimate.amount_micros)
                .ok_or(DryRunError::Overflow)?;

            match currency {
                None => currency = Some(estimate.currency),
                Some(current) if current == estimate.currency => {}
                Some(_) => return Err(DryRunError::CurrencyMismatch),
            }

            duration = match (duration, estimate.estimated_duration_ms) {
                (Some(total), Some(next)) => {
                    Some(total.checked_add(next).ok_or(DryRunError::Overflow)?)
                }
                _ => None,
            };
        }

        Ok(DryRunSummary {
            request_count,
            call_count,
            amount_micros,
            currency,
            estimated_duration_ms: if requests.is_empty() { None } else { duration },
        })
    }

    #[must_use]
    pub fn allows_paid_fallback(&self, estimate: &CostEstimate, user_approved: bool) -> bool {
        self.policy
            .fallback_policy
            .allows_paid_amount(estimate.amount_micros)
            && (!self.policy.fallback_policy.require_user_approval || user_approved)
    }

    fn ensure_writable(&self) -> Result<(), JobRuntimeError> {
        if self.project.is_read_only() {
            Err(JobRuntimeError::ReadOnlyProject)
        } else {
            Ok(())
        }
    }

    fn ensure_provider(&self, record: &RuntimeIntent) -> Result<(), JobRuntimeError> {
        if record.provider == self.provider {
            Ok(())
        } else {
            Err(JobRuntimeError::ProviderMismatch)
        }
    }
}

fn derive_idempotency_key(
    operation: &str,
    input_hash: &InputHash,
) -> Result<IdempotencyKey, JobRuntimeError> {
    IdempotencyKey::new(format!("{operation}:{}", input_hash.as_str()))
        .map_err(JobRuntimeError::JobValue)
}

fn retry_disposition(class: ErrorClass, attempts: u16, policy: BackoffPolicy) -> RetryDisposition {
    if class == ErrorClass::Terminal {
        return RetryDisposition::Never;
    }
    if attempts >= policy.max_attempts() {
        return RetryDisposition::Exhausted;
    }

    let exponent = u32::from(attempts.saturating_sub(1)).min(63);
    let multiplier = 1_u64.checked_shl(exponent).unwrap_or(u64::MAX);
    RetryDisposition::AfterMs(
        policy
            .base_delay_ms()
            .saturating_mul(multiplier)
            .min(policy.max_delay_ms()),
    )
}

static PROVIDER_LIMITERS: OnceLock<Mutex<BTreeMap<ProviderKey, Weak<ProviderLimiterState>>>> =
    OnceLock::new();

fn provider_limiter_registry() -> &'static Mutex<BTreeMap<ProviderKey, Weak<ProviderLimiterState>>>
{
    PROVIDER_LIMITERS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct ProviderLimiter {
    state: Arc<ProviderLimiterState>,
    registered_max: NonZeroU16,
}

impl ProviderLimiter {
    fn new(provider: &ProviderKey, max: NonZeroU16) -> Self {
        let state = {
            let mut registry = lock_unpoisoned(provider_limiter_registry());
            registry.retain(|_, state| state.strong_count() > 0);
            let state = match registry.get(provider).and_then(Weak::upgrade) {
                Some(state) => state,
                None => {
                    let state = Arc::new(ProviderLimiterState::new());
                    registry.insert(provider.clone(), Arc::downgrade(&state));
                    state
                }
            };
            state.register(max);
            state
        };
        Self {
            state,
            registered_max: max,
        }
    }

    fn effective_max(&self) -> NonZeroU16 {
        self.state.effective_max()
    }

    fn try_acquire(&self) -> Option<ProviderPermit> {
        self.state.try_acquire()
    }
}

impl Drop for ProviderLimiter {
    fn drop(&mut self) {
        self.state.unregister(self.registered_max);
    }
}

struct ProviderLimiterState {
    inner: Mutex<ProviderLimiterStateInner>,
}

impl ProviderLimiterState {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ProviderLimiterStateInner {
                registrations: BTreeMap::new(),
                in_flight: 0,
            }),
        }
    }

    fn register(&self, max: NonZeroU16) {
        let mut inner = lock_unpoisoned(&self.inner);
        let count = inner.registrations.entry(max).or_insert(0);
        *count = count.saturating_add(1);
    }

    fn unregister(&self, max: NonZeroU16) {
        let mut inner = lock_unpoisoned(&self.inner);
        let remove = match inner.registrations.get_mut(&max) {
            Some(count) if *count > 1 => {
                *count -= 1;
                false
            }
            Some(_) => true,
            None => false,
        };
        if remove {
            inner.registrations.remove(&max);
        }
    }

    fn effective_max(&self) -> NonZeroU16 {
        let inner = lock_unpoisoned(&self.inner);
        // A wider runtime policy must never bypass a stricter live policy.
        inner
            .registrations
            .first_key_value()
            .map(|(max, _)| *max)
            .expect("live provider limiter must have a registered ceiling")
    }

    fn try_acquire(self: &Arc<Self>) -> Option<ProviderPermit> {
        let mut inner = lock_unpoisoned(&self.inner);
        let max = inner
            .registrations
            .first_key_value()
            .map(|(max, _)| max.get())
            .expect("live provider limiter must have a registered ceiling");
        if inner.in_flight >= max {
            return None;
        }
        inner.in_flight += 1;
        Some(ProviderPermit {
            state: Arc::clone(self),
        })
    }
}

struct ProviderLimiterStateInner {
    registrations: BTreeMap<NonZeroU16, usize>,
    in_flight: u16,
}

struct ProviderPermit {
    state: Arc<ProviderLimiterState>,
}

impl Drop for ProviderPermit {
    fn drop(&mut self) {
        let mut inner = lock_unpoisoned(&self.state.inner);
        inner.in_flight = inner.in_flight.saturating_sub(1);
    }
}

struct IntentStore<'project> {
    project: &'project CanonicalProject,
}

impl<'project> IntentStore<'project> {
    fn new(project: &'project CanonicalProject) -> Self {
        Self { project }
    }

    fn save(&self, record: &RuntimeIntent) -> Result<(), JobRuntimeError> {
        let path = intent_path(record.intent.job_id())?;
        let bytes = serde_json::to_vec_pretty(&StoredIntent::from_runtime(record))?;
        self.project.root().write_atomic(&path, &bytes)?;
        Ok(())
    }

    fn load(&self, job_id: &JobId) -> Result<RuntimeIntent, JobRuntimeError> {
        let path = intent_path(job_id)?;
        let bytes = self.project.root().read(&path)?;
        let stored: StoredIntent = serde_json::from_slice(&bytes)?;
        let record = RuntimeIntent::try_from(stored)?;
        if record.intent.job_id() != job_id {
            return Err(JobRuntimeError::InvalidRecord(
                "intent path does not match stored job_id",
            ));
        }
        Ok(record)
    }

    fn find_by_idempotency(
        &self,
        provider: &ProviderKey,
        key: &IdempotencyKey,
    ) -> Result<Option<RuntimeIntent>, JobRuntimeError> {
        Ok(self
            .load_all()?
            .into_iter()
            .find(|record| record.provider == *provider && record.intent.idempotency_key() == key))
    }

    fn load_all(&self) -> Result<Vec<RuntimeIntent>, JobRuntimeError> {
        let Some(directory) = checked_runtime_directory(self.project, RUNTIME_INTENT_DIR)? else {
            return Ok(Vec::new());
        };
        let mut records = Vec::new();

        for entry in fs::read_dir(directory).map_err(ProjectFsError::Io)? {
            let entry = entry.map_err(ProjectFsError::Io)?;
            let file_type = entry.file_type().map_err(ProjectFsError::Io)?;
            if file_type.is_symlink() {
                return Err(JobRuntimeError::Fs(ProjectFsError::Symlink(entry.path())));
            }
            if !file_type.is_file() {
                continue;
            }

            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| JobRuntimeError::InvalidRecord("intent filename must be UTF-8"))?;
            if name.starts_with(".kineto-write-") || !name.ends_with(".json") {
                continue;
            }

            let path = ProjectRelativePath::new(format!("{RUNTIME_INTENT_DIR}/{name}"))?;
            let bytes = self.project.root().read(&path)?;
            let stored: StoredIntent = serde_json::from_slice(&bytes)?;
            let record = RuntimeIntent::try_from(stored)?;
            if name != format!("{}.json", record.intent.job_id().as_str()) {
                return Err(JobRuntimeError::InvalidRecord(
                    "intent filename does not match stored job_id",
                ));
            }
            records.push(record);
        }

        records.sort_by(|left, right| left.intent.job_id().cmp(right.intent.job_id()));
        Ok(records)
    }
}

fn intent_path(job_id: &JobId) -> Result<ProjectRelativePath, ProjectPathError> {
    ProjectRelativePath::new(format!("{RUNTIME_INTENT_DIR}/{}.json", job_id.as_str()))
}

fn checked_runtime_directory(
    project: &CanonicalProject,
    relative: &str,
) -> Result<Option<PathBuf>, JobRuntimeError> {
    let mut current = project.root().root().to_path_buf();
    for segment in relative.split('/') {
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(JobRuntimeError::Fs(ProjectFsError::Symlink(current)));
                }
                if !metadata.is_dir() {
                    return Err(JobRuntimeError::Fs(ProjectFsError::NotDirectory(current)));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(JobRuntimeError::Fs(ProjectFsError::Io(error))),
        }
    }
    Ok(Some(current))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredIntentState {
    Prepared,
    Invoked,
    Reconciled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredErrorClass {
    Retryable,
    Terminal,
}

impl From<ErrorClass> for StoredErrorClass {
    fn from(value: ErrorClass) -> Self {
        match value {
            ErrorClass::Retryable => Self::Retryable,
            ErrorClass::Terminal => Self::Terminal,
        }
    }
}

impl From<StoredErrorClass> for ErrorClass {
    fn from(value: StoredErrorClass) -> Self {
        match value {
            StoredErrorClass::Retryable => Self::Retryable,
            StoredErrorClass::Terminal => Self::Terminal,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct StoredIntent {
    schema_version: u32,
    provider: String,
    job_id: String,
    operation: String,
    idempotency_key: String,
    input_hash: String,
    artifact_id: Option<String>,
    provider_job_id: Option<String>,
    state: StoredIntentState,
    attempts: u16,
    #[serde(default)]
    last_error_class: Option<StoredErrorClass>,
}

impl StoredIntent {
    fn from_runtime(record: &RuntimeIntent) -> Self {
        Self {
            schema_version: RUNTIME_INTENT_SCHEMA_VERSION,
            provider: record.provider.as_str().to_owned(),
            job_id: record.intent.job_id().as_str().to_owned(),
            operation: record.intent.operation().to_owned(),
            idempotency_key: record.intent.idempotency_key().as_str().to_owned(),
            input_hash: record.intent.input_hash().as_str().to_owned(),
            artifact_id: record
                .intent
                .artifact_id()
                .map(|artifact_id| artifact_id.as_str().to_owned()),
            provider_job_id: record
                .intent
                .provider_job_id()
                .map(|provider_job_id| provider_job_id.as_str().to_owned()),
            state: match record.intent.state() {
                IntentState::Prepared => StoredIntentState::Prepared,
                IntentState::Invoked => StoredIntentState::Invoked,
                IntentState::Reconciled => StoredIntentState::Reconciled,
            },
            attempts: record.attempts,
            last_error_class: record.last_error_class.map(StoredErrorClass::from),
        }
    }
}

impl TryFrom<StoredIntent> for RuntimeIntent {
    type Error = JobRuntimeError;

    fn try_from(stored: StoredIntent) -> Result<Self, Self::Error> {
        if stored.schema_version != RUNTIME_INTENT_SCHEMA_VERSION {
            return Err(JobRuntimeError::UnsupportedIntentSchema(
                stored.schema_version,
            ));
        }

        let provider = ProviderKey::new(stored.provider)?;
        let provider_job_id = stored.provider_job_id.map(ProviderJobId::new).transpose()?;
        if matches!(stored.state, StoredIntentState::Prepared) && provider_job_id.is_some() {
            return Err(JobRuntimeError::InvalidRecord(
                "prepared intent cannot contain provider_job_id",
            ));
        }
        if !matches!(stored.state, StoredIntentState::Prepared) && stored.last_error_class.is_some()
        {
            return Err(JobRuntimeError::InvalidRecord(
                "only prepared intent may contain last_error_class",
            ));
        }

        let mut intent = JobIntent::new(
            JobId::new(stored.job_id)?,
            stored.operation,
            IdempotencyKey::new(stored.idempotency_key)?,
            InputHash::new(stored.input_hash)?,
            stored.artifact_id.map(ArtifactId::new).transpose()?,
        )?;

        match stored.state {
            StoredIntentState::Prepared => {}
            StoredIntentState::Invoked => intent.mark_invoked(provider_job_id)?,
            StoredIntentState::Reconciled => {
                intent.mark_invoked(provider_job_id)?;
                intent.mark_reconciled()?;
            }
        }

        Ok(Self {
            provider,
            intent,
            attempts: stored.attempts,
            last_error_class: stored.last_error_class.map(ErrorClass::from),
        })
    }
}

#[derive(Debug)]
pub enum JobRuntimeError {
    ReadOnlyProject,
    ConcurrencyLimitReached,
    AttemptsExhausted,
    PreviousTerminalFailure,
    IntentNotPrepared,
    ProviderMismatch,
    Overflow,
    UnsupportedIntentSchema(u32),
    InvalidRecord(&'static str),
    ProviderKey(ProviderKeyError),
    JobValue(JobValueError),
    JobIntent(JobIntentError),
    ArtifactId(ArtifactIdError),
    Hash(HashValueError),
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Json(serde_json::Error),
}

impl fmt::Display for JobRuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReadOnlyProject => {
                formatter.write_str("read-only project cannot invoke paid jobs")
            }
            Self::ConcurrencyLimitReached => {
                formatter.write_str("provider concurrency limit reached")
            }
            Self::AttemptsExhausted => formatter.write_str("provider retry attempts exhausted"),
            Self::PreviousTerminalFailure => {
                formatter.write_str("job intent records a terminal provider failure")
            }
            Self::IntentNotPrepared => {
                formatter.write_str("job intent is not prepared for invocation")
            }
            Self::ProviderMismatch => {
                formatter.write_str("job intent belongs to a different provider")
            }
            Self::Overflow => formatter.write_str("job runtime counter overflow"),
            Self::UnsupportedIntentSchema(version) => {
                write!(
                    formatter,
                    "unsupported runtime intent schema version {version}"
                )
            }
            Self::InvalidRecord(message) => {
                write!(formatter, "invalid runtime intent record: {message}")
            }
            Self::ProviderKey(error) => error.fmt(formatter),
            Self::JobValue(error) => error.fmt(formatter),
            Self::JobIntent(error) => error.fmt(formatter),
            Self::ArtifactId(error) => error.fmt(formatter),
            Self::Hash(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid runtime intent JSON: {error}"),
        }
    }
}

impl Error for JobRuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ProviderKey(error) => Some(error),
            Self::JobValue(error) => Some(error),
            Self::JobIntent(error) => Some(error),
            Self::ArtifactId(error) => Some(error),
            Self::Hash(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Fs(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::ReadOnlyProject
            | Self::ConcurrencyLimitReached
            | Self::AttemptsExhausted
            | Self::PreviousTerminalFailure
            | Self::IntentNotPrepared
            | Self::ProviderMismatch
            | Self::Overflow
            | Self::UnsupportedIntentSchema(_)
            | Self::InvalidRecord(_) => None,
        }
    }
}

impl From<ProviderKeyError> for JobRuntimeError {
    fn from(error: ProviderKeyError) -> Self {
        Self::ProviderKey(error)
    }
}

impl From<JobValueError> for JobRuntimeError {
    fn from(error: JobValueError) -> Self {
        Self::JobValue(error)
    }
}

impl From<JobIntentError> for JobRuntimeError {
    fn from(error: JobIntentError) -> Self {
        Self::JobIntent(error)
    }
}

impl From<ArtifactIdError> for JobRuntimeError {
    fn from(error: ArtifactIdError) -> Self {
        Self::ArtifactId(error)
    }
}

impl From<HashValueError> for JobRuntimeError {
    fn from(error: HashValueError) -> Self {
        Self::Hash(error)
    }
}

impl From<ProjectPathError> for JobRuntimeError {
    fn from(error: ProjectPathError) -> Self {
        Self::Path(error)
    }
}

impl From<ProjectFsError> for JobRuntimeError {
    fn from(error: ProjectFsError) -> Self {
        Self::Fs(error)
    }
}

impl From<serde_json::Error> for JobRuntimeError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}

impl<E> From<JobRuntimeError> for InvokePreparedError<E> {
    fn from(error: JobRuntimeError) -> Self {
        Self::Runtime(error)
    }
}

impl<E> From<JobIntentError> for InvokePreparedError<E> {
    fn from(error: JobIntentError) -> Self {
        Self::Runtime(JobRuntimeError::JobIntent(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_jobs::{BackoffPolicy, PaidFallback};
    use kineto_project::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use std::{
        collections::BTreeMap,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(
                std::env::temp_dir()
                    .join(format!("kineto-job-runtime-{}-{nonce}", std::process::id())),
            )
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(temp: &TempDir) -> CanonicalProject {
        let manifest = ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: "job_runtime_test".to_owned(),
            title: "Job Runtime Test".to_owned(),
            created_at: "2026-09-13T00:00:00Z".to_owned(),
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
        };
        CanonicalProject::create(&temp.0, manifest, b"story\n").unwrap()
    }

    fn policy(max_concurrency: u16, max_attempts: u16) -> JobRuntimePolicy {
        JobRuntimePolicy {
            execution: ProviderExecutionPolicy::new(
                NonZeroU16::new(max_concurrency).unwrap(),
                BackoffPolicy::new(100, 1_000, max_attempts).unwrap(),
            ),
            fallback_policy: FallbackPolicy {
                paid_fallback: PaidFallback::Allowed,
                max_paid_amount_micros: Some(5_000_000),
                require_user_approval: true,
            },
        }
    }

    fn input_hash(fill: char) -> InputHash {
        InputHash::new(format!("sha256:{}", fill.to_string().repeat(64))).unwrap()
    }

    fn created(outcome: PrepareOutcome) -> RuntimeIntent {
        match outcome {
            PrepareOutcome::Created(record) => record,
            PrepareOutcome::Existing(_) => panic!("expected a new intent"),
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        Retryable,
        Terminal,
    }

    impl fmt::Display for FakeError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "{self:?}")
        }
    }

    impl Error for FakeError {}

    struct FakeAdapter {
        root: PathBuf,
        expected_job: Option<JobId>,
        invocation: Result<Invocation<&'static str>, FakeError>,
        by_handle: Reconciliation<&'static str>,
        by_key: Reconciliation<&'static str>,
        invoke_count: usize,
        reconcile_count: usize,
    }

    impl FakeAdapter {
        fn completed(root: PathBuf) -> Self {
            Self {
                root,
                expected_job: None,
                invocation: Ok(Invocation::Completed("result")),
                by_handle: Reconciliation::NotFound,
                by_key: Reconciliation::NotFound,
                invoke_count: 0,
                reconcile_count: 0,
            }
        }
    }

    impl PaidJobAdapter for FakeAdapter {
        type Request = u64;
        type Output = &'static str;
        type Error = FakeError;

        fn estimate_cost(&self, request: &Self::Request) -> Result<CostEstimate, Self::Error> {
            Ok(CostEstimate {
                call_count: 1,
                amount_micros: *request,
                currency: CurrencyCode::new(*b"USD").unwrap(),
                estimated_duration_ms: Some(1_000),
            })
        }

        fn invoke(
            &mut self,
            _request: &Self::Request,
            _idempotency_key: &IdempotencyKey,
        ) -> Result<Invocation<Self::Output>, Self::Error> {
            self.invoke_count += 1;
            if let Some(job_id) = &self.expected_job {
                let path = self
                    .root
                    .join(format!("{RUNTIME_INTENT_DIR}/{}.json", job_id.as_str()));
                let bytes = fs::read(path).expect("Prepared intent must exist before invoke");
                let stored: StoredIntent = serde_json::from_slice(&bytes).unwrap();
                assert!(matches!(stored.state, StoredIntentState::Prepared));
                assert_eq!(stored.attempts, 1);
            }
            self.invocation.clone()
        }

        fn reconcile(
            &mut self,
            _provider_job_id: &ProviderJobId,
        ) -> Result<Reconciliation<Self::Output>, Self::Error> {
            self.reconcile_count += 1;
            Ok(self.by_handle.clone())
        }

        fn reconcile_by_idempotency_key(
            &mut self,
            _idempotency_key: &IdempotencyKey,
        ) -> Result<Reconciliation<Self::Output>, Self::Error> {
            self.reconcile_count += 1;
            Ok(self.by_key.clone())
        }

        fn classify_error(&self, error: &Self::Error) -> ErrorClass {
            match error {
                FakeError::Retryable => ErrorClass::Retryable,
                FakeError::Terminal => ErrorClass::Terminal,
            }
        }
    }

    #[test]
    fn prepared_intent_is_persisted_before_provider_invocation() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-video-persist").unwrap(),
            policy(1, 3),
        );
        let job_id = JobId::new("job_001").unwrap();
        let record = created(
            runtime
                .prepare(job_id.clone(), "video.generate", input_hash('a'), None)
                .unwrap(),
        );
        assert_eq!(record.intent().state(), IntentState::Prepared);

        let mut adapter = FakeAdapter::completed(temp.0.clone());
        adapter.expected_job = Some(job_id.clone());
        let outcome = runtime
            .invoke_prepared(&job_id, &mut adapter, &1_000_000)
            .unwrap();
        assert!(matches!(outcome, DispatchOutcome::Completed { .. }));

        let persisted = IntentStore::new(&project).load(&job_id).unwrap();
        assert_eq!(persisted.intent().state(), IntentState::Invoked);
        assert_eq!(persisted.attempts(), 1);
    }

    #[test]
    fn startup_reattaches_remote_job_without_reinvoking() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-video-reconcile").unwrap(),
            policy(1, 3),
        );
        let job_id = JobId::new("job_002").unwrap();
        created(
            runtime
                .prepare(job_id.clone(), "video.generate", input_hash('b'), None)
                .unwrap(),
        );
        let remote = ProviderJobId::new("provider/jobs:42").unwrap();
        let mut adapter = FakeAdapter::completed(temp.0.clone());
        adapter.invocation = Ok(Invocation::Pending(remote.clone()));
        runtime
            .invoke_prepared(&job_id, &mut adapter, &1_000_000)
            .unwrap();

        let reopened = CanonicalProject::open(&temp.0).unwrap();
        let runtime = JobRuntime::new(
            &reopened,
            ProviderKey::new("fake-video-reconcile").unwrap(),
            policy(1, 3),
        );
        let mut recovery = FakeAdapter::completed(temp.0.clone());
        recovery.by_handle = Reconciliation::Completed {
            output: "reattached",
            provider_job_id: Some(remote),
        };
        let outcomes = runtime.reconcile_startup(&mut recovery).unwrap();

        assert_eq!(recovery.invoke_count, 0);
        assert_eq!(recovery.reconcile_count, 1);
        assert!(matches!(
            outcomes.as_slice(),
            [StartupOutcome::ResultReady {
                output: "reattached",
                ..
            }]
        ));
    }

    #[test]
    fn prepared_crash_window_reconciles_by_idempotency_key_before_retry() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-image-crash").unwrap(),
            policy(1, 3),
        );
        let job_id = JobId::new("job_003").unwrap();
        created(
            runtime
                .prepare(job_id.clone(), "image.generate", input_hash('c'), None)
                .unwrap(),
        );

        let mut recovery = FakeAdapter::completed(temp.0.clone());
        recovery.by_key = Reconciliation::Completed {
            output: "found-by-key",
            provider_job_id: Some(ProviderJobId::new("remote-003").unwrap()),
        };
        let outcomes = runtime.reconcile_startup(&mut recovery).unwrap();

        assert_eq!(recovery.invoke_count, 0);
        assert_eq!(recovery.reconcile_count, 1);
        assert!(matches!(
            outcomes.as_slice(),
            [StartupOutcome::ResultReady {
                output: "found-by-key",
                ..
            }]
        ));
        let persisted = IntentStore::new(&project).load(&job_id).unwrap();
        assert_eq!(persisted.intent().state(), IntentState::Invoked);
    }

    #[test]
    fn result_is_reconciled_only_after_canonical_acknowledgement() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-image-ack").unwrap(),
            policy(1, 3),
        );
        let job_id = JobId::new("job_004").unwrap();
        created(
            runtime
                .prepare(job_id.clone(), "image.generate", input_hash('d'), None)
                .unwrap(),
        );
        let mut adapter = FakeAdapter::completed(temp.0.clone());
        runtime
            .invoke_prepared(&job_id, &mut adapter, &500_000)
            .unwrap();
        assert_eq!(
            IntentStore::new(&project)
                .load(&job_id)
                .unwrap()
                .intent()
                .state(),
            IntentState::Invoked
        );

        let acknowledged = runtime.acknowledge_result(&job_id).unwrap();
        assert_eq!(acknowledged.intent().state(), IntentState::Reconciled);
    }

    #[test]
    fn dry_run_aggregates_cost_without_invoking_or_persisting() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-image-dry-run").unwrap(),
            policy(1, 3),
        );
        let adapter = FakeAdapter::completed(temp.0.clone());
        let summary = runtime.dry_run(&adapter, &[1_000_000, 2_000_000]).unwrap();

        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.call_count, 2);
        assert_eq!(summary.amount_micros, 3_000_000);
        assert_eq!(summary.currency.unwrap().bytes(), *b"USD");
        assert_eq!(summary.estimated_duration_ms, Some(2_000));
        assert!(!temp.0.join(RUNTIME_INTENT_DIR).exists());
        assert_eq!(adapter.invoke_count, 0);
    }

    #[test]
    fn retryable_and_terminal_failures_follow_backoff_policy() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-image-retry").unwrap(),
            policy(1, 2),
        );
        let retry_job = JobId::new("job_005").unwrap();
        created(
            runtime
                .prepare(retry_job.clone(), "image.generate", input_hash('e'), None)
                .unwrap(),
        );
        let mut retryable = FakeAdapter::completed(temp.0.clone());
        retryable.invocation = Err(FakeError::Retryable);
        match runtime.invoke_prepared(&retry_job, &mut retryable, &1) {
            Err(InvokePreparedError::Provider {
                class: ErrorClass::Retryable,
                retry: RetryDisposition::AfterMs(100),
                ..
            }) => {}
            other => panic!("unexpected retry result: {other:?}"),
        }
        match runtime.invoke_prepared(&retry_job, &mut retryable, &1) {
            Err(InvokePreparedError::Provider {
                retry: RetryDisposition::Exhausted,
                ..
            }) => {}
            other => panic!("unexpected exhausted result: {other:?}"),
        }

        let terminal_job = JobId::new("job_006").unwrap();
        created(
            runtime
                .prepare(
                    terminal_job.clone(),
                    "image.generate",
                    input_hash('f'),
                    None,
                )
                .unwrap(),
        );
        let mut terminal = FakeAdapter::completed(temp.0.clone());
        terminal.invocation = Err(FakeError::Terminal);
        match runtime.invoke_prepared(&terminal_job, &mut terminal, &1) {
            Err(InvokePreparedError::Provider {
                class: ErrorClass::Terminal,
                retry: RetryDisposition::Never,
                ..
            }) => {}
            other => panic!("unexpected terminal result: {other:?}"),
        }
        assert_eq!(
            IntentStore::new(&project)
                .load(&terminal_job)
                .unwrap()
                .last_error_class(),
            Some(ErrorClass::Terminal)
        );
        assert!(matches!(
            runtime.invoke_prepared(&terminal_job, &mut terminal, &1),
            Err(InvokePreparedError::Runtime(
                JobRuntimeError::PreviousTerminalFailure
            ))
        ));
    }

    #[test]
    fn terminal_failure_remains_non_retryable_after_restart() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-image-terminal-restart").unwrap(),
            policy(1, 3),
        );
        let job_id = JobId::new("job_terminal_restart").unwrap();
        created(
            runtime
                .prepare(job_id.clone(), "image.generate", input_hash('9'), None)
                .unwrap(),
        );
        let mut adapter = FakeAdapter::completed(temp.0.clone());
        adapter.invocation = Err(FakeError::Terminal);
        let _ = runtime.invoke_prepared(&job_id, &mut adapter, &1);
        drop(runtime);
        drop(project);

        let reopened = CanonicalProject::open(&temp.0).unwrap();
        let runtime = JobRuntime::new(
            &reopened,
            ProviderKey::new("fake-image-terminal-restart").unwrap(),
            policy(1, 3),
        );
        let mut recovery = FakeAdapter::completed(temp.0.clone());
        let outcomes = runtime.reconcile_startup(&mut recovery).unwrap();
        assert!(matches!(
            outcomes.as_slice(),
            [StartupOutcome::RetryPrepared {
                retry: RetryDisposition::Never,
                ..
            }]
        ));
    }

    #[test]
    fn duplicate_semantic_work_reuses_existing_intent() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-video-duplicate").unwrap(),
            policy(1, 3),
        );
        let first = created(
            runtime
                .prepare(
                    JobId::new("job_007").unwrap(),
                    "video.generate",
                    input_hash('7'),
                    None,
                )
                .unwrap(),
        );
        let second = runtime
            .prepare(
                JobId::new("job_008").unwrap(),
                "video.generate",
                input_hash('7'),
                None,
            )
            .unwrap();

        match second {
            PrepareOutcome::Existing(existing) => {
                assert_eq!(existing.intent().job_id(), first.intent().job_id());
                assert_eq!(
                    existing.intent().idempotency_key(),
                    first.intent().idempotency_key()
                );
            }
            PrepareOutcome::Created(_) => panic!("duplicate work must not create another intent"),
        }
    }

    #[test]
    fn paid_fallback_requires_both_budget_and_explicit_approval() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-video-fallback").unwrap(),
            policy(1, 3),
        );
        let usd = CurrencyCode::new(*b"USD").unwrap();
        let cheap = CostEstimate {
            call_count: 1,
            amount_micros: 4_000_000,
            currency: usd,
            estimated_duration_ms: None,
        };
        let expensive = CostEstimate {
            amount_micros: 6_000_000,
            ..cheap
        };

        assert!(!runtime.allows_paid_fallback(&cheap, false));
        assert!(runtime.allows_paid_fallback(&cheap, true));
        assert!(!runtime.allows_paid_fallback(&expensive, true));
    }

    #[test]
    fn provider_limiter_is_process_wide_across_job_runtimes() {
        let first_temp = TempDir::new();
        let second_temp = TempDir::new();
        let first_project = project(&first_temp);
        let second_project = project(&second_temp);
        let provider = ProviderKey::new("shared-limit-provider").unwrap();
        let first = JobRuntime::new(&first_project, provider.clone(), policy(1, 3));
        let second = JobRuntime::new(&second_project, provider, policy(1, 3));

        let permit = first.limiter.try_acquire().unwrap();
        assert!(second.limiter.try_acquire().is_none());
        drop(permit);
        assert!(second.limiter.try_acquire().is_some());
    }

    #[test]
    fn different_provider_keys_do_not_share_concurrency_ceiling() {
        let temp = TempDir::new();
        let project = project(&temp);
        let first = JobRuntime::new(
            &project,
            ProviderKey::new("independent-provider-a").unwrap(),
            policy(1, 3),
        );
        let second = JobRuntime::new(
            &project,
            ProviderKey::new("independent-provider-b").unwrap(),
            policy(1, 3),
        );

        let first_permit = first.limiter.try_acquire().unwrap();
        assert!(second.limiter.try_acquire().is_some());
        drop(first_permit);
    }

    #[test]
    fn provider_permit_releases_after_success_error_and_drop() {
        let temp = TempDir::new();
        let project = project(&temp);
        let provider = ProviderKey::new("permit-release-provider").unwrap();
        let first = JobRuntime::new(&project, provider.clone(), policy(1, 3));
        let second = JobRuntime::new(&project, provider, policy(1, 3));

        let success_job = JobId::new("job_permit_success").unwrap();
        created(
            first
                .prepare(success_job.clone(), "image.generate", input_hash('1'), None)
                .unwrap(),
        );
        let mut success = FakeAdapter::completed(temp.0.clone());
        first
            .invoke_prepared(&success_job, &mut success, &1)
            .unwrap();
        let permit = second.limiter.try_acquire().unwrap();
        drop(permit);

        let error_job = JobId::new("job_permit_error").unwrap();
        created(
            first
                .prepare(error_job.clone(), "image.generate", input_hash('2'), None)
                .unwrap(),
        );
        let mut error = FakeAdapter::completed(temp.0.clone());
        error.invocation = Err(FakeError::Retryable);
        assert!(matches!(
            first.invoke_prepared(&error_job, &mut error, &1),
            Err(InvokePreparedError::Provider { .. })
        ));
        let permit = second.limiter.try_acquire().unwrap();
        drop(permit);

        let permit = first.limiter.try_acquire().unwrap();
        assert!(second.limiter.try_acquire().is_none());
        drop(permit);
        assert!(second.limiter.try_acquire().is_some());
    }

    #[test]
    fn conflicting_provider_ceilings_use_strictest_live_policy() {
        let temp = TempDir::new();
        let project = project(&temp);
        let provider = ProviderKey::new("conflicting-limit-provider").unwrap();
        let wide = JobRuntime::new(&project, provider.clone(), policy(4, 3));
        let strict = JobRuntime::new(&project, provider, policy(1, 3));

        assert_eq!(wide.effective_max_concurrency().get(), 1);
        assert_eq!(strict.effective_max_concurrency().get(), 1);
        let first = wide.limiter.try_acquire().unwrap();
        assert!(wide.limiter.try_acquire().is_none());
        drop(first);

        drop(strict);
        assert_eq!(wide.effective_max_concurrency().get(), 4);
        let first = wide.limiter.try_acquire().unwrap();
        let second = wide.limiter.try_acquire().unwrap();
        let third = wide.limiter.try_acquire().unwrap();
        let fourth = wide.limiter.try_acquire().unwrap();
        assert!(wide.limiter.try_acquire().is_none());
        drop((first, second, third, fourth));
    }
}
