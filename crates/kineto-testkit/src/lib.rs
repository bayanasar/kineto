use std::{collections::BTreeMap, error::Error, fmt, thread, time::Duration};

use kineto_job_runtime::{Invocation, PaidJobAdapter, Reconciliation};
use kineto_jobs::{CostEstimate, CurrencyCode, ErrorClass, IdempotencyKey, ProviderJobId};
use kineto_providers::{
    ImageCapabilities, LlmCapabilities, TtsCapabilities, VideoCapabilities,
    fake as capability_fixtures,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeFailurePlan {
    None,
    RetryableThenSuccess(u16),
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FakeCompletion {
    #[default]
    Immediate,
    Pending,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FakeBehavior {
    latency: Duration,
    failure: FakeFailurePlan,
    completion: FakeCompletion,
}

impl FakeBehavior {
    #[must_use]
    pub const fn success() -> Self {
        Self {
            latency: Duration::ZERO,
            failure: FakeFailurePlan::None,
            completion: FakeCompletion::Immediate,
        }
    }

    #[must_use]
    pub const fn retryable_then_success(failures: u16) -> Self {
        Self {
            latency: Duration::ZERO,
            failure: FakeFailurePlan::RetryableThenSuccess(failures),
            completion: FakeCompletion::Immediate,
        }
    }

    #[must_use]
    pub const fn terminal_failure() -> Self {
        Self {
            latency: Duration::ZERO,
            failure: FakeFailurePlan::Terminal,
            completion: FakeCompletion::Immediate,
        }
    }

    #[must_use]
    pub const fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    #[must_use]
    pub const fn pending(mut self) -> Self {
        self.completion = FakeCompletion::Pending;
        self
    }
}

impl Default for FakeBehavior {
    fn default() -> Self {
        Self::success()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeRequest {
    pub payload: String,
    pub amount_micros: u64,
    pub estimated_duration_ms: Option<u64>,
}

impl FakeRequest {
    #[must_use]
    pub fn new(
        payload: impl Into<String>,
        amount_micros: u64,
        estimated_duration_ms: Option<u64>,
    ) -> Self {
        Self {
            payload: payload.into(),
            amount_micros,
            estimated_duration_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeOutput {
    pub modality: &'static str,
    pub payload: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeProviderError {
    Retryable,
    Terminal,
}

impl fmt::Display for FakeProviderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Retryable => formatter.write_str("injected retryable provider failure"),
            Self::Terminal => formatter.write_str("injected terminal provider failure"),
        }
    }
}

impl Error for FakeProviderError {}

#[derive(Debug, Clone)]
enum RemoteState {
    Pending(FakeOutput),
    Completed(FakeOutput),
}

#[derive(Debug, Clone)]
struct RemoteRecord {
    provider_job_id: Option<ProviderJobId>,
    state: RemoteState,
}

#[derive(Debug)]
pub struct FakePaidAdapter<C> {
    modality: &'static str,
    capabilities: C,
    behavior: FakeBehavior,
    retryable_failures_remaining: u16,
    invocation_count: u32,
    records_by_key: BTreeMap<String, RemoteRecord>,
    key_by_job_id: BTreeMap<String, String>,
}

impl<C> FakePaidAdapter<C> {
    #[must_use]
    fn new(modality: &'static str, capabilities: C, behavior: FakeBehavior) -> Self {
        let retryable_failures_remaining = match behavior.failure {
            FakeFailurePlan::RetryableThenSuccess(failures) => failures,
            FakeFailurePlan::None | FakeFailurePlan::Terminal => 0,
        };
        Self {
            modality,
            capabilities,
            behavior,
            retryable_failures_remaining,
            invocation_count: 0,
            records_by_key: BTreeMap::new(),
            key_by_job_id: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn capabilities(&self) -> &C {
        &self.capabilities
    }

    #[must_use]
    pub const fn invocation_count(&self) -> u32 {
        self.invocation_count
    }

    pub fn complete_pending(&mut self, idempotency_key: &IdempotencyKey) -> bool {
        let Some(record) = self.records_by_key.get_mut(idempotency_key.as_str()) else {
            return false;
        };
        let output = match &record.state {
            RemoteState::Pending(output) => output.clone(),
            RemoteState::Completed(_) => return false,
        };
        record.state = RemoteState::Completed(output);
        true
    }

    fn maybe_sleep(&self) {
        if !self.behavior.latency.is_zero() {
            thread::sleep(self.behavior.latency);
        }
    }

    fn output_for(&self, request: &FakeRequest, idempotency_key: &IdempotencyKey) -> FakeOutput {
        FakeOutput {
            modality: self.modality,
            payload: request.payload.clone(),
            idempotency_key: idempotency_key.as_str().to_owned(),
        }
    }

    fn provider_job_id_for(&self, idempotency_key: &IdempotencyKey) -> ProviderJobId {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for byte in self
            .modality
            .bytes()
            .chain([0xff])
            .chain(idempotency_key.as_str().bytes())
        {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        ProviderJobId::new(format!("fake/{}/{hash:016x}", self.modality))
            .expect("fake provider job ids are portable")
    }

    fn reconciliation(record: &RemoteRecord) -> Reconciliation<FakeOutput> {
        match &record.state {
            RemoteState::Pending(_) => Reconciliation::Pending {
                provider_job_id: record.provider_job_id.clone(),
            },
            RemoteState::Completed(output) => Reconciliation::Completed {
                output: output.clone(),
                provider_job_id: record.provider_job_id.clone(),
            },
        }
    }
}

impl<C> PaidJobAdapter for FakePaidAdapter<C> {
    type Request = FakeRequest;
    type Output = FakeOutput;
    type Error = FakeProviderError;

    fn estimate_cost(&self, request: &Self::Request) -> Result<CostEstimate, Self::Error> {
        Ok(CostEstimate {
            call_count: 1,
            amount_micros: request.amount_micros,
            currency: CurrencyCode::new(*b"USD").expect("USD is a valid currency code"),
            estimated_duration_ms: request.estimated_duration_ms,
        })
    }

    fn invoke(
        &mut self,
        request: &Self::Request,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<Self::Output>, Self::Error> {
        self.maybe_sleep();
        self.invocation_count = self.invocation_count.saturating_add(1);

        match self.behavior.failure {
            FakeFailurePlan::Terminal => return Err(FakeProviderError::Terminal),
            FakeFailurePlan::RetryableThenSuccess(_) if self.retryable_failures_remaining > 0 => {
                self.retryable_failures_remaining -= 1;
                return Err(FakeProviderError::Retryable);
            }
            FakeFailurePlan::None | FakeFailurePlan::RetryableThenSuccess(_) => {}
        }

        if let Some(record) = self.records_by_key.get(idempotency_key.as_str()) {
            return Ok(match Self::reconciliation(record) {
                Reconciliation::Completed { output, .. } => Invocation::Completed(output),
                Reconciliation::Pending {
                    provider_job_id: Some(provider_job_id),
                } => Invocation::Pending(provider_job_id),
                Reconciliation::Pending {
                    provider_job_id: None,
                }
                | Reconciliation::NotFound => {
                    unreachable!("fake invocation records are always addressable")
                }
            });
        }

        let output = self.output_for(request, idempotency_key);
        match self.behavior.completion {
            FakeCompletion::Immediate => {
                self.records_by_key.insert(
                    idempotency_key.as_str().to_owned(),
                    RemoteRecord {
                        provider_job_id: None,
                        state: RemoteState::Completed(output.clone()),
                    },
                );
                Ok(Invocation::Completed(output))
            }
            FakeCompletion::Pending => {
                let provider_job_id = self.provider_job_id_for(idempotency_key);
                self.key_by_job_id.insert(
                    provider_job_id.as_str().to_owned(),
                    idempotency_key.as_str().to_owned(),
                );
                self.records_by_key.insert(
                    idempotency_key.as_str().to_owned(),
                    RemoteRecord {
                        provider_job_id: Some(provider_job_id.clone()),
                        state: RemoteState::Pending(output),
                    },
                );
                Ok(Invocation::Pending(provider_job_id))
            }
        }
    }

    fn reconcile(
        &mut self,
        provider_job_id: &ProviderJobId,
    ) -> Result<Reconciliation<Self::Output>, Self::Error> {
        self.maybe_sleep();
        let Some(key) = self.key_by_job_id.get(provider_job_id.as_str()) else {
            return Ok(Reconciliation::NotFound);
        };
        Ok(self
            .records_by_key
            .get(key)
            .map(Self::reconciliation)
            .unwrap_or(Reconciliation::NotFound))
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Reconciliation<Self::Output>, Self::Error> {
        self.maybe_sleep();
        Ok(self
            .records_by_key
            .get(idempotency_key.as_str())
            .map(Self::reconciliation)
            .unwrap_or(Reconciliation::NotFound))
    }

    fn classify_error(&self, error: &Self::Error) -> ErrorClass {
        match error {
            FakeProviderError::Retryable => ErrorClass::Retryable,
            FakeProviderError::Terminal => ErrorClass::Terminal,
        }
    }
}

pub type FakeLlmAdapter = FakePaidAdapter<LlmCapabilities>;
pub type FakeImageAdapter = FakePaidAdapter<ImageCapabilities>;
pub type FakeVideoAdapter = FakePaidAdapter<VideoCapabilities>;
pub type FakeTtsAdapter = FakePaidAdapter<TtsCapabilities>;

#[must_use]
pub fn llm(behavior: FakeBehavior) -> FakeLlmAdapter {
    FakePaidAdapter::new("llm", capability_fixtures::llm_structured(), behavior)
}

#[must_use]
pub fn image_full(behavior: FakeBehavior) -> FakeImageAdapter {
    FakePaidAdapter::new("image", capability_fixtures::image_full(), behavior)
}

#[must_use]
pub fn image_text_only(behavior: FakeBehavior) -> FakeImageAdapter {
    FakePaidAdapter::new("image", capability_fixtures::image_text_only(), behavior)
}

#[must_use]
pub fn video_i2v_resumable(behavior: FakeBehavior) -> FakeVideoAdapter {
    FakePaidAdapter::new(
        "video",
        capability_fixtures::video_i2v_resumable(),
        behavior,
    )
}

#[must_use]
pub fn video_t2v_only(behavior: FakeBehavior) -> FakeVideoAdapter {
    FakePaidAdapter::new("video", capability_fixtures::video_t2v_only(), behavior)
}

#[must_use]
pub fn tts_basic(behavior: FakeBehavior) -> FakeTtsAdapter {
    FakePaidAdapter::new("tts", capability_fixtures::tts_basic(), behavior)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_job_runtime::{
        DispatchOutcome, InvokePreparedError, JobRuntime, JobRuntimePolicy, PrepareOutcome,
        ProviderKey, RetryDisposition,
    };
    use kineto_jobs::{
        BackoffPolicy, FallbackPolicy, JobId, PaidFallback, ProviderExecutionPolicy,
    };
    use kineto_project::{
        ArtifactId, InputHash,
        manifest::{
            CanonicalProject, PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest,
            ProjectSource, ProjectWorkflow,
        },
    };
    use kineto_providers::{ImageRequirements, ReferenceRole, SeedSupport, evaluate_image};
    use std::{
        collections::BTreeMap,
        fs,
        num::NonZeroU16,
        path::PathBuf,
        time::{Instant, SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock should be after Unix epoch")
                .as_nanos();
            Self(
                std::env::temp_dir().join(format!("kineto-testkit-{}-{nonce}", std::process::id())),
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
            project_id: "provider_testkit".to_owned(),
            title: "Provider Testkit".to_owned(),
            created_at: "2026-09-20T00:00:00Z".to_owned(),
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
        CanonicalProject::create(&temp.0, manifest, b"story\n")
            .expect("test project should be creatable")
    }

    fn policy(max_attempts: u16) -> JobRuntimePolicy {
        JobRuntimePolicy {
            execution: ProviderExecutionPolicy::new(
                NonZeroU16::new(1).expect("one is non-zero"),
                BackoffPolicy::new(100, 1_000, max_attempts).expect("test backoff should be valid"),
            ),
            fallback_policy: FallbackPolicy {
                paid_fallback: PaidFallback::Allowed,
                max_paid_amount_micros: Some(5_000_000),
                require_user_approval: true,
            },
        }
    }

    fn request() -> FakeRequest {
        FakeRequest::new("fixture-input", 250_000, Some(2_000))
    }

    fn key() -> IdempotencyKey {
        IdempotencyKey::new("image.generate:fixture-key")
            .expect("test idempotency key should be valid")
    }

    fn assert_immediate_success<C>(mut adapter: FakePaidAdapter<C>) {
        let request = request();
        let estimate = adapter
            .estimate_cost(&request)
            .expect("fake cost estimate should succeed");
        assert_eq!(estimate.call_count, 1);
        assert_eq!(estimate.amount_micros, 250_000);
        assert_eq!(estimate.currency.bytes(), *b"USD");

        let output = match adapter
            .invoke(&request, &key())
            .expect("fake invocation should succeed")
        {
            Invocation::Completed(output) => output,
            Invocation::Pending(_) => panic!("expected immediate fake completion"),
        };
        assert_eq!(output.payload, "fixture-input");
        assert_eq!(adapter.invocation_count(), 1);
    }

    #[test]
    fn all_modalities_use_the_real_paid_job_adapter_contract() {
        assert_immediate_success(llm(FakeBehavior::success()));
        assert_immediate_success(image_full(FakeBehavior::success()));
        assert_immediate_success(video_i2v_resumable(FakeBehavior::success()));
        assert_immediate_success(tts_basic(FakeBehavior::success()));
    }

    #[test]
    fn deterministic_success_is_stable_across_fresh_adapters() {
        let request = request();
        let key = key();
        let mut first = image_full(FakeBehavior::success());
        let mut second = image_full(FakeBehavior::success());

        let Invocation::Completed(first_output) = first
            .invoke(&request, &key)
            .expect("first fake call should succeed")
        else {
            panic!("expected immediate completion");
        };
        let Invocation::Completed(second_output) = second
            .invoke(&request, &key)
            .expect("second fake call should succeed")
        else {
            panic!("expected immediate completion");
        };

        assert_eq!(first_output, second_output);
    }

    #[test]
    fn injected_latency_is_applied_to_provider_io() {
        let latency = Duration::from_millis(5);
        let mut adapter = llm(FakeBehavior::success().with_latency(latency));
        let started = Instant::now();

        adapter
            .invoke(&request(), &key())
            .expect("fake invocation should succeed");

        assert!(started.elapsed() >= latency);
    }

    #[test]
    fn transient_failure_then_success_exercises_job_runtime_retry_path() {
        let temp = TempDir::new();
        let project = project(&temp);
        let runtime = JobRuntime::new(
            &project,
            ProviderKey::new("fake-image-testkit").expect("provider key should be valid"),
            policy(3),
        );
        let job_id = JobId::new("job_testkit_retry").expect("job id should be valid");
        let prepared = runtime
            .prepare(
                job_id.clone(),
                "image.generate",
                InputHash::new(format!("sha256:{}", "a".repeat(64)))
                    .expect("input hash should be valid"),
                ArtifactId::new("image_candidate_testkit").expect("artifact id should be valid"),
            )
            .expect("prepare should succeed");
        assert!(matches!(prepared, PrepareOutcome::Created(_)));

        let mut adapter = image_full(FakeBehavior::retryable_then_success(1));
        match runtime.invoke_prepared(&job_id, &mut adapter, &request()) {
            Err(InvokePreparedError::Provider {
                class: ErrorClass::Retryable,
                retry: RetryDisposition::AfterMs(100),
                ..
            }) => {}
            other => panic!("unexpected first invocation result: {other:?}"),
        }

        let completed = runtime
            .invoke_prepared(&job_id, &mut adapter, &request())
            .expect("second invocation should succeed");
        assert!(matches!(completed, DispatchOutcome::Completed { .. }));
        assert_eq!(adapter.invocation_count(), 2);
    }

    #[test]
    fn terminal_failure_is_classified_without_provider_spend() {
        let mut adapter = video_i2v_resumable(FakeBehavior::terminal_failure());
        let error = adapter
            .invoke(&request(), &key())
            .expect_err("terminal fake must fail");

        assert_eq!(adapter.classify_error(&error), ErrorClass::Terminal);
    }

    #[test]
    fn pending_fake_can_be_reconciled_by_handle_and_key() {
        let request = request();
        let key = key();
        let mut adapter = video_i2v_resumable(FakeBehavior::success().pending());
        let Invocation::Pending(provider_job_id) = adapter
            .invoke(&request, &key)
            .expect("pending fake invocation should succeed")
        else {
            panic!("expected pending fake invocation");
        };

        assert!(matches!(
            adapter
                .reconcile(&provider_job_id)
                .expect("handle reconciliation should succeed"),
            Reconciliation::Pending { .. }
        ));
        assert!(adapter.complete_pending(&key));
        assert!(matches!(
            adapter
                .reconcile_by_idempotency_key(&key)
                .expect("key reconciliation should succeed"),
            Reconciliation::Completed { .. }
        ));
    }

    #[test]
    fn capability_limited_fake_exposes_real_compatibility_gaps() {
        let adapter = image_text_only(FakeBehavior::success());
        let report = evaluate_image(
            ImageRequirements {
                required_reference_role: Some(ReferenceRole::Identity),
                minimum_seed_support: SeedSupport::Deterministic,
                requires_negative_prompt: true,
                requires_mask: false,
            },
            adapter.capabilities(),
        );

        assert!(!report.is_compatible());
        assert!(!report.is_lossless());
    }
}
