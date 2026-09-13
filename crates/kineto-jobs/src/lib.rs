use std::{error::Error, fmt, num::NonZeroU16};

use kineto_project::{ArtifactId, InputHash};

const MAX_LOCAL_ID_LEN: usize = 128;
const MAX_EXTERNAL_ID_LEN: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct JobId(String);

impl JobId {
    pub fn new(value: impl Into<String>) -> Result<Self, JobValueError> {
        portable_identifier(value.into(), MAX_LOCAL_ID_LEN, b"").map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn new(value: impl Into<String>) -> Result<Self, JobValueError> {
        portable_identifier(value.into(), MAX_EXTERNAL_ID_LEN, b":").map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderJobId(String);

impl ProviderJobId {
    pub fn new(value: impl Into<String>) -> Result<Self, JobValueError> {
        portable_identifier(value.into(), MAX_EXTERNAL_ID_LEN, b":/").map(Self)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn portable_identifier(
    value: String,
    max_len: usize,
    extra_allowed: &[u8],
) -> Result<String, JobValueError> {
    if value.is_empty()
        || value.len() > max_len
        || !value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'_' | b'-' | b'.')
                || extra_allowed.contains(&byte)
        })
    {
        Err(JobValueError)
    } else {
        Ok(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobValueError;

impl fmt::Display for JobValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("job identifier must be bounded portable ASCII without whitespace or control characters")
    }
}

impl Error for JobValueError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Queued,
    Running,
    WaitingForUser,
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntentState {
    Prepared,
    Invoked,
    Reconciled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobIntent {
    job_id: JobId,
    operation: String,
    idempotency_key: IdempotencyKey,
    input_hash: InputHash,
    artifact_id: Option<ArtifactId>,
    provider_job_id: Option<ProviderJobId>,
    state: IntentState,
}

impl JobIntent {
    pub fn new(
        job_id: JobId,
        operation: impl Into<String>,
        idempotency_key: IdempotencyKey,
        input_hash: InputHash,
        artifact_id: Option<ArtifactId>,
    ) -> Result<Self, JobIntentError> {
        let operation = operation.into();
        if operation.is_empty() {
            return Err(JobIntentError::MissingOperation);
        }
        Ok(Self {
            job_id,
            operation,
            idempotency_key,
            input_hash,
            artifact_id,
            provider_job_id: None,
            state: IntentState::Prepared,
        })
    }

    #[must_use]
    pub fn job_id(&self) -> &JobId {
        &self.job_id
    }

    #[must_use]
    pub fn operation(&self) -> &str {
        &self.operation
    }

    #[must_use]
    pub fn idempotency_key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    #[must_use]
    pub fn input_hash(&self) -> &InputHash {
        &self.input_hash
    }

    #[must_use]
    pub fn artifact_id(&self) -> Option<&ArtifactId> {
        self.artifact_id.as_ref()
    }

    #[must_use]
    pub fn provider_job_id(&self) -> Option<&ProviderJobId> {
        self.provider_job_id.as_ref()
    }

    #[must_use]
    pub const fn state(&self) -> IntentState {
        self.state
    }

    pub fn mark_invoked(
        &mut self,
        provider_job_id: Option<ProviderJobId>,
    ) -> Result<(), JobIntentError> {
        if self.state != IntentState::Prepared {
            return Err(JobIntentError::InvalidTransition);
        }
        self.provider_job_id = provider_job_id;
        self.state = IntentState::Invoked;
        Ok(())
    }

    pub fn mark_reconciled(&mut self) -> Result<(), JobIntentError> {
        if self.state != IntentState::Invoked {
            return Err(JobIntentError::InvalidTransition);
        }
        self.state = IntentState::Reconciled;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobIntentError {
    MissingOperation,
    InvalidTransition,
}

impl fmt::Display for JobIntentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::MissingOperation => "job operation must not be empty",
            Self::InvalidTransition => "invalid write-ahead intent transition",
        })
    }
}

impl Error for JobIntentError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyCode([u8; 3]);

impl CurrencyCode {
    pub fn new(code: [u8; 3]) -> Result<Self, CurrencyCodeError> {
        if code.iter().all(u8::is_ascii_uppercase) {
            Ok(Self(code))
        } else {
            Err(CurrencyCodeError)
        }
    }

    #[must_use]
    pub const fn bytes(self) -> [u8; 3] {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CurrencyCodeError;

impl fmt::Display for CurrencyCodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("currency code must be three uppercase ASCII letters")
    }
}

impl Error for CurrencyCodeError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostEstimate {
    pub call_count: u32,
    pub amount_micros: u64,
    pub currency: CurrencyCode,
    pub estimated_duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    Retryable,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicy {
    base_delay_ms: u64,
    max_delay_ms: u64,
    max_attempts: u16,
}

impl BackoffPolicy {
    pub fn new(
        base_delay_ms: u64,
        max_delay_ms: u64,
        max_attempts: u16,
    ) -> Result<Self, BackoffPolicyError> {
        if max_attempts == 0 || base_delay_ms > max_delay_ms {
            Err(BackoffPolicyError)
        } else {
            Ok(Self {
                base_delay_ms,
                max_delay_ms,
                max_attempts,
            })
        }
    }

    #[must_use]
    pub const fn base_delay_ms(self) -> u64 {
        self.base_delay_ms
    }

    #[must_use]
    pub const fn max_delay_ms(self) -> u64 {
        self.max_delay_ms
    }

    #[must_use]
    pub const fn max_attempts(self) -> u16 {
        self.max_attempts
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffPolicyError;

impl fmt::Display for BackoffPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("backoff policy requires attempts > 0 and base delay <= max delay")
    }
}

impl Error for BackoffPolicyError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderExecutionPolicy {
    max_concurrency: NonZeroU16,
    backoff: BackoffPolicy,
}

impl ProviderExecutionPolicy {
    #[must_use]
    pub const fn new(max_concurrency: NonZeroU16, backoff: BackoffPolicy) -> Self {
        Self {
            max_concurrency,
            backoff,
        }
    }

    #[must_use]
    pub const fn max_concurrency(self) -> NonZeroU16 {
        self.max_concurrency
    }

    #[must_use]
    pub const fn backoff(self) -> BackoffPolicy {
        self.backoff
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaidFallback {
    Forbidden,
    Allowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FallbackPolicy {
    pub paid_fallback: PaidFallback,
    pub max_paid_amount_micros: Option<u64>,
    pub require_user_approval: bool,
}

impl FallbackPolicy {
    #[must_use]
    pub const fn allows_paid_amount(self, amount_micros: u64) -> bool {
        if matches!(self.paid_fallback, PaidFallback::Forbidden) {
            return false;
        }
        match self.max_paid_amount_micros {
            Some(limit) => amount_micros <= limit,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_hash(fill: char) -> InputHash {
        InputHash::new(format!("sha256:{}", fill.to_string().repeat(64))).unwrap()
    }

    fn idempotency(operation: &str, fill: char) -> IdempotencyKey {
        IdempotencyKey::new(format!(
            "{operation}:sha256:{}",
            fill.to_string().repeat(64)
        ))
        .unwrap()
    }

    #[test]
    fn provider_handle_is_recorded_on_the_write_ahead_intent() {
        let mut intent = JobIntent::new(
            JobId::new("job_01J").unwrap(),
            "video.generate",
            idempotency("video.generate", 'a'),
            input_hash('a'),
            None,
        )
        .unwrap();

        let remote = ProviderJobId::new("remote-42").unwrap();
        intent.mark_invoked(Some(remote.clone())).unwrap();

        assert_eq!(intent.provider_job_id(), Some(&remote));
        assert_eq!(intent.state(), IntentState::Invoked);
        assert_eq!(intent.operation(), "video.generate");
        assert_eq!(intent.job_id().as_str(), "job_01J");
        assert!(intent.artifact_id().is_none());
        assert!(
            intent
                .idempotency_key()
                .as_str()
                .starts_with("video.generate:")
        );
        assert!(intent.input_hash().as_str().starts_with("sha256:"));
    }

    #[test]
    fn intent_cannot_skip_directly_from_prepared_to_reconciled() {
        let mut intent = JobIntent::new(
            JobId::new("job_02J").unwrap(),
            "image.generate",
            idempotency("image.generate", 'b'),
            input_hash('b'),
            None,
        )
        .unwrap();

        assert_eq!(
            intent.mark_reconciled().unwrap_err(),
            JobIntentError::InvalidTransition
        );
    }

    #[test]
    fn money_is_integer_and_paid_fallback_respects_the_ceiling() {
        let usd = CurrencyCode::new(*b"USD").unwrap();
        let estimate = CostEstimate {
            call_count: 42,
            amount_micros: 12_500_000,
            currency: usd,
            estimated_duration_ms: Some(18 * 60 * 1000),
        };
        let policy = FallbackPolicy {
            paid_fallback: PaidFallback::Allowed,
            max_paid_amount_micros: Some(10_000_000),
            require_user_approval: true,
        };

        assert_eq!(estimate.currency.bytes(), *b"USD");
        assert!(!policy.allows_paid_amount(estimate.amount_micros));
    }

    #[test]
    fn invalid_backoff_configuration_is_rejected_at_construction() {
        assert!(BackoffPolicy::new(5_000, 1_000, 3).is_err());
        assert!(BackoffPolicy::new(1_000, 5_000, 0).is_err());
    }

    #[test]
    fn provider_execution_policy_can_only_hold_valid_backoff() {
        let backoff = BackoffPolicy::new(500, 5_000, 4).unwrap();
        let concurrency = NonZeroU16::new(2).unwrap();
        let policy = ProviderExecutionPolicy::new(concurrency, backoff);

        assert_eq!(policy.max_concurrency(), concurrency);
        assert_eq!(policy.backoff().base_delay_ms(), 500);
        assert_eq!(policy.backoff().max_delay_ms(), 5_000);
        assert_eq!(policy.backoff().max_attempts(), 4);
    }

    #[test]
    fn job_identifiers_are_bounded_and_portable() {
        assert!(JobId::new("job_01J.test-2").is_ok());
        assert!(JobId::new("job with spaces").is_err());
        assert!(JobId::new("x".repeat(MAX_LOCAL_ID_LEN + 1)).is_err());
        assert!(IdempotencyKey::new("video.generate:sha256:abc").is_ok());
        assert!(ProviderJobId::new("provider/jobs:remote-42").is_ok());
        assert!(ProviderJobId::new("remote\n42").is_err());
    }
}
