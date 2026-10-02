//! Deterministic offline backends for tests: scripted answers, injected
//! latency, retryable-then-success and terminal failures, and answers that
//! do not fit a schema. No test needs a network or a paid call.

use std::{collections::VecDeque, error::Error, fmt, sync::Mutex, thread, time::Duration};

use kineto_jobs::ErrorClass;
use kineto_providers::LlmCapabilities;

use crate::{Billing, LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse, TokenUsage};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeLlmError {
    Retryable,
    Terminal,
}

impl fmt::Display for FakeLlmError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Retryable => "injected retryable LLM failure",
            Self::Terminal => "injected terminal LLM failure",
        })
    }
}

impl Error for FakeLlmError {}

/// Answers from a script, in order, recording every request.
#[derive(Debug)]
pub struct FakeLlm {
    script: Mutex<VecDeque<Result<String, FakeLlmError>>>,
    requests: Mutex<Vec<LlmRequest>>,
    latency: Duration,
    billing: Billing,
}

impl FakeLlm {
    /// Answer with each text in turn.
    pub fn answering<S: Into<String>>(answers: impl IntoIterator<Item = S>) -> Self {
        Self::scripted(answers.into_iter().map(|answer| Ok(answer.into())))
    }

    /// Fail every call.
    #[must_use]
    pub fn failing(error: FakeLlmError) -> Self {
        Self::scripted(std::iter::repeat_n(Err(error), 16))
    }

    /// Fail `failures` times retryably, then answer with `answer`.
    #[must_use]
    pub fn retryable_then(failures: usize, answer: impl Into<String>) -> Self {
        Self::scripted(
            std::iter::repeat_n(Err(FakeLlmError::Retryable), failures)
                .chain(std::iter::once(Ok(answer.into()))),
        )
    }

    pub fn scripted(script: impl IntoIterator<Item = Result<String, FakeLlmError>>) -> Self {
        Self {
            script: Mutex::new(script.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
            latency: Duration::ZERO,
            billing: Billing::Free,
        }
    }

    /// Bill every call, as a metered API does. Free otherwise.
    #[must_use]
    pub const fn metered(mut self) -> Self {
        self.billing = Billing::PerToken {
            input_micros_per_million: 1_000_000,
            output_micros_per_million: 2_000_000,
        };
        self
    }

    #[must_use]
    pub const fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// Every request so far, in order.
    #[must_use]
    pub fn requests(&self) -> Vec<LlmRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl LlmBackend for FakeLlm {
    type Error = FakeLlmError;

    fn info(&self) -> LlmBackendInfo {
        LlmBackendInfo {
            provider: "fake-llm".to_owned(),
            model: "scripted".to_owned(),
            capabilities: LlmCapabilities {
                context_window_tokens: Some(128_000),
                tools: false,
                structured_output: true,
                vision_input: false,
            },
            billing: self.billing,
        }
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, FakeLlmError> {
        self.requests.lock().unwrap().push(request.clone());
        if !self.latency.is_zero() {
            thread::sleep(self.latency);
        }
        let text = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(FakeLlmError::Terminal))?;
        Ok(LlmResponse {
            usage: Some(TokenUsage {
                input_tokens: request
                    .messages
                    .iter()
                    .map(|message| crate::estimate_tokens(&message.content))
                    .sum(),
                output_tokens: crate::estimate_tokens(&text),
            }),
            text,
            model: "scripted".to_owned(),
        })
    }

    fn classify(&self, error: &FakeLlmError) -> ErrorClass {
        match error {
            FakeLlmError::Retryable => ErrorClass::Retryable,
            FakeLlmError::Terminal => ErrorClass::Terminal,
        }
    }

    /// A retryable failure stands for a lost connection, which may have
    /// lost an answer; a terminal one for a refusal, which did not.
    fn may_have_lost_an_answer(&self, error: &FakeLlmError) -> bool {
        *error == FakeLlmError::Retryable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use std::time::Instant;

    fn request() -> LlmRequest {
        LlmRequest {
            messages: vec![Message::user("hello")],
            ..LlmRequest::default()
        }
    }

    #[test]
    fn the_script_plays_in_order_and_runs_out_terminally() {
        let fake = FakeLlm::retryable_then(2, "done");
        assert_eq!(fake.complete(&request()), Err(FakeLlmError::Retryable));
        assert_eq!(fake.complete(&request()), Err(FakeLlmError::Retryable));
        assert_eq!(fake.complete(&request()).unwrap().text, "done");
        assert_eq!(fake.complete(&request()), Err(FakeLlmError::Terminal));
        assert_eq!(fake.requests().len(), 4);
    }

    #[test]
    fn latency_is_injected() {
        let fake = FakeLlm::answering(["slow"]).with_latency(Duration::from_millis(30));
        let started = Instant::now();
        fake.complete(&request()).unwrap();
        assert!(started.elapsed() >= Duration::from_millis(30));
    }
}
