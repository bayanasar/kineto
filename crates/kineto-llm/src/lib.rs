//! The `llm.complete` capability.
//!
//! Workflow code asks an [`LlmBackend`] to complete a conversation and never
//! names a vendor. Backends come in two kinds: HTTP APIs that speak the
//! OpenAI chat-completions protocol (a cloud service with an API key, or a
//! local endpoint), and agent CLIs such as Kimi Code, driven as configured
//! local processes that manage their own sign-in.
//!
//! Structured output is schema-first: the request carries the JSON Schema the
//! answer must satisfy, and [`complete_structured`] turns the answer into
//! typed state only after it deserializes. An answer that does not is asked
//! for once more with the error, then refused.

use std::{error::Error, fmt};

use kineto_jobs::{CostEstimate, CurrencyCode, ErrorClass};
use kineto_providers::LlmCapabilities;
use serde::de::DeserializeOwned;

#[cfg(any(test, feature = "testing"))]
pub mod fake;

/// Who a message is from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    #[must_use]
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
    }
}

/// The JSON Schema an answer must satisfy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputSchema {
    /// A short identifier, for example `story_analysis`.
    pub name: String,
    /// The schema as JSON text, exactly as committed under `schemas/`.
    pub schema_json: String,
}

/// One completion.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmRequest {
    pub messages: Vec<Message>,
    /// Ask for JSON that satisfies this schema.
    pub schema: Option<OutputSchema>,
    pub max_output_tokens: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

/// What a backend answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmResponse {
    pub text: String,
    /// Token counts, when the backend reports them.
    pub usage: Option<TokenUsage>,
    /// The model that answered, as the backend names it.
    pub model: String,
}

/// How a backend is paid for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Billing {
    /// Metered per token, in USD micros per million tokens.
    PerToken {
        input_micros_per_million: u64,
        output_micros_per_million: u64,
    },
    /// Covered by a subscription the agent CLI signs in to; the cost of one
    /// call is not known.
    Subscription,
    /// A local model.
    Free,
}

/// What the director needs to know about a backend before asking it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmBackendInfo {
    /// The provider key its jobs are recorded under, for example `kimi`.
    pub provider: String,
    pub model: String,
    pub capabilities: LlmCapabilities,
    pub billing: Billing,
}

/// A backend for `llm.complete`.
///
/// Calls are blocking; long work runs on a session worker or its own thread.
pub trait LlmBackend {
    type Error: Error + Send + Sync + 'static;

    fn info(&self) -> LlmBackendInfo;

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, Self::Error>;

    /// Whether asking again unchanged can succeed.
    fn classify(&self, error: &Self::Error) -> ErrorClass;
}

/// A rough token count: about four characters per token for the scripts the
/// models are trained on most. Used only to estimate cost before a call.
#[must_use]
pub fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.chars().count().div_ceil(4)).unwrap_or(u64::MAX)
}

/// Estimated cost of `request` when the answer is about
/// `expected_output_tokens` long. `None` when the cost cannot be known, as
/// with a subscription.
#[must_use]
pub fn estimate_cost(
    info: &LlmBackendInfo,
    request: &LlmRequest,
    expected_output_tokens: u64,
) -> Option<CostEstimate> {
    let amount_micros = match info.billing {
        Billing::Subscription => return None,
        Billing::Free => 0,
        Billing::PerToken {
            input_micros_per_million,
            output_micros_per_million,
        } => {
            let input = request
                .messages
                .iter()
                .map(|message| estimate_tokens(&message.content))
                .sum::<u64>()
                + request
                    .schema
                    .as_ref()
                    .map_or(0, |schema| estimate_tokens(&schema.schema_json));
            let micros = u128::from(input) * u128::from(input_micros_per_million)
                + u128::from(expected_output_tokens) * u128::from(output_micros_per_million);
            u64::try_from(micros.div_ceil(1_000_000)).unwrap_or(u64::MAX)
        }
    };
    Some(CostEstimate {
        call_count: 1,
        amount_micros,
        currency: CurrencyCode::new(*b"USD").expect("static currency code is valid"),
        estimated_duration_ms: None,
    })
}

/// A typed answer and the response it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Structured<T> {
    pub value: T,
    pub response: LlmResponse,
    /// Calls made: 1, or 2 when the first answer did not fit the schema.
    pub attempts: u8,
}

#[derive(Debug)]
pub enum StructuredError<E> {
    Backend {
        source: E,
        class: ErrorClass,
    },
    /// The answer did not fit the schema, twice. `detail` is the parser's
    /// account of the second answer; the answer itself is not kept, since it
    /// may quote untrusted source text at length.
    Schema {
        detail: String,
    },
}

impl<E: fmt::Display> fmt::Display for StructuredError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend { source, .. } => source.fmt(formatter),
            Self::Schema { detail } => {
                write!(
                    formatter,
                    "the model's answer did not fit the schema: {detail}"
                )
            }
        }
    }
}

impl<E: Error + 'static> Error for StructuredError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Backend { source, .. } => Some(source),
            Self::Schema { .. } => None,
        }
    }
}

impl<E> StructuredError<E> {
    /// A schema failure is terminal: the same request already failed twice.
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Backend { class, .. } => *class,
            Self::Schema { .. } => ErrorClass::Terminal,
        }
    }
}

/// The JSON in an answer: the whole text, or the body of a fenced code block
/// when a model wraps it in one.
#[must_use]
pub fn json_body(text: &str) -> &str {
    let trimmed = text.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let rest = rest.split_once('\n').map_or("", |(_, body)| body);
    rest.trim_end().strip_suffix("```").unwrap_or(rest).trim()
}

/// Complete `request`, which must carry a schema, and parse the answer as
/// `T`. An answer that does not parse is asked for once more, with the
/// parser's error.
pub fn complete_structured<B: LlmBackend, T: DeserializeOwned>(
    backend: &B,
    request: &LlmRequest,
) -> Result<Structured<T>, StructuredError<B::Error>> {
    let mut request = request.clone();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let response = backend
            .complete(&request)
            .map_err(|source| StructuredError::Backend {
                class: backend.classify(&source),
                source,
            })?;
        match serde_json::from_str::<T>(json_body(&response.text)) {
            Ok(value) => {
                return Ok(Structured {
                    value,
                    response,
                    attempts,
                });
            }
            Err(error) if attempts == 1 => {
                request.messages.push(Message::assistant(response.text));
                request.messages.push(Message::user(format!(
                    "That reply does not satisfy the required JSON Schema ({error}). Reply \
                     again with only the JSON value, no prose and no code fence."
                )));
            }
            Err(error) => {
                return Err(StructuredError::Schema {
                    detail: error.to_string(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeLlm, FakeLlmError};
    use serde::Deserialize;

    #[derive(Debug, Deserialize, PartialEq, Eq)]
    #[serde(deny_unknown_fields)]
    struct Title {
        title: String,
    }

    fn request() -> LlmRequest {
        LlmRequest {
            messages: vec![
                Message::system("You title stories."),
                Message::user("A filmmaker waits."),
            ],
            schema: Some(OutputSchema {
                name: "title".to_owned(),
                schema_json: r#"{"type":"object","required":["title"]}"#.to_owned(),
            }),
            max_output_tokens: Some(100),
        }
    }

    #[test]
    fn a_fitting_answer_becomes_typed_state() {
        let backend = FakeLlm::answering([r#"{"title":"The Last Line"}"#]);
        let structured: Structured<Title> = complete_structured(&backend, &request()).unwrap();
        assert_eq!(structured.value.title, "The Last Line");
        assert_eq!(structured.attempts, 1);
        assert_eq!(backend.requests().len(), 1);
    }

    #[test]
    fn a_fenced_answer_is_read_from_its_fence() {
        let backend = FakeLlm::answering(["```json\n{\"title\":\"Fenced\"}\n```"]);
        let structured: Structured<Title> = complete_structured(&backend, &request()).unwrap();
        assert_eq!(structured.value.title, "Fenced");
        assert_eq!(json_body("```\n[1]\n```"), "[1]");
        assert_eq!(json_body("  {}  "), "{}");
    }

    #[test]
    fn an_unfitting_answer_is_asked_for_once_more_then_refused() {
        let backend = FakeLlm::answering([r#"{"name":"wrong"}"#, r#"{"title":"Second"}"#]);
        let structured: Structured<Title> = complete_structured(&backend, &request()).unwrap();
        assert_eq!(structured.value.title, "Second");
        assert_eq!(structured.attempts, 2);
        let retry = &backend.requests()[1];
        assert_eq!(retry.messages.len(), 4);
        assert_eq!(retry.messages[2], Message::assistant(r#"{"name":"wrong"}"#));
        assert!(retry.messages[3].content.contains("unknown field `name`"));

        let backend = FakeLlm::answering(["prose", "more prose"]);
        let error = complete_structured::<_, Title>(&backend, &request()).unwrap_err();
        assert!(matches!(error, StructuredError::Schema { .. }));
        assert_eq!(error.class(), ErrorClass::Terminal);
        assert!(!error.to_string().contains("more prose"));
    }

    #[test]
    fn backend_failures_keep_their_class() {
        for (failure, class) in [
            (FakeLlmError::Retryable, ErrorClass::Retryable),
            (FakeLlmError::Terminal, ErrorClass::Terminal),
        ] {
            let backend = FakeLlm::failing(failure);
            let error = complete_structured::<_, Title>(&backend, &request()).unwrap_err();
            assert_eq!(error.class(), class);
            assert_eq!(
                backend.requests().len(),
                1,
                "a backend failure is not retried here"
            );
        }
    }

    #[test]
    fn cost_follows_billing() {
        let mut info = FakeLlm::answering(["{}"]).info();
        assert_eq!(
            estimate_cost(&info, &request(), 1000)
                .unwrap()
                .amount_micros,
            0
        );
        info.billing = Billing::Subscription;
        assert_eq!(estimate_cost(&info, &request(), 1000), None);
        info.billing = Billing::PerToken {
            input_micros_per_million: 600_000,
            output_micros_per_million: 2_500_000,
        };
        let tokens = request()
            .messages
            .iter()
            .map(|message| estimate_tokens(&message.content))
            .sum::<u64>()
            + estimate_tokens(&request().schema.unwrap().schema_json);
        let expected = (tokens * 600_000 + 1000 * 2_500_000).div_ceil(1_000_000);
        assert_eq!(
            estimate_cost(&info, &request(), 1000)
                .unwrap()
                .amount_micros,
            expected
        );
        assert_eq!(estimate_tokens("abcde"), 2);
    }
}
