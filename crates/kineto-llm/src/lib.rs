//! The `llm.complete` capability.
//!
//! Workflow code asks an [`LlmBackend`] to complete a conversation and never
//! names a vendor. Backends come in two kinds: HTTP APIs that speak the
//! OpenAI chat-completions protocol (a cloud service with an API key, or a
//! local endpoint), and the agent CLIs the director already signed in to
//! (Claude Code, Codex, Kimi Code, agy, Mini-Agent), driven as local
//! processes without tools.
//!
//! Structured output is schema-first: the request carries the JSON Schema the
//! answer must satisfy, and [`complete_structured`] turns the answer into
//! typed state only after it deserializes. An answer that does not is asked
//! for once more with the error, then refused.

use std::{error::Error, fmt};

use kineto_jobs::{CostEstimate, CurrencyCode, ErrorClass};
use kineto_providers::LlmCapabilities;
use serde::de::DeserializeOwned;

pub mod acp;
pub mod agents;
#[cfg(any(test, feature = "testing"))]
pub mod fake;
pub mod http;
pub mod openai;
mod process;

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
    /// Metered, but this build knows no price for the model.
    Unpriced,
}

impl Billing {
    /// Whether each call is charged for, so that asking twice for one answer
    /// pays twice.
    #[must_use]
    pub const fn charged_per_call(self) -> bool {
        matches!(self, Self::PerToken { .. } | Self::Unpriced)
    }
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

    /// Whether a failed call may have produced an answer, and its charge,
    /// that never arrived: the request may have reached the model before the
    /// connection was lost. A call refused before the model ran, or one
    /// whose answer arrived and was refused, did not. Unknown failures may.
    fn may_have_lost_an_answer(&self, _error: &Self::Error) -> bool {
        true
    }
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
        Billing::Subscription | Billing::Unpriced => return None,
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
    /// Calls made: 1, or 2 when the first answer was not accepted.
    pub attempts: u8,
}

#[derive(Debug)]
pub enum StructuredError<E> {
    Backend {
        source: E,
        class: ErrorClass,
    },
    /// The answer did not fit the schema or failed its check, twice.
    /// `detail` says why the second answer was refused; the answer itself is
    /// not kept, since it may quote untrusted source text at length.
    Schema {
        detail: String,
    },
    /// The request's schema cannot be checked, so nothing was asked.
    UnsupportedSchema(String),
}

impl<E: fmt::Display> fmt::Display for StructuredError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend { source, .. } => source.fmt(formatter),
            Self::Schema { detail } => {
                write!(formatter, "the model's answer was not accepted: {detail}")
            }
            Self::UnsupportedSchema(detail) => {
                write!(
                    formatter,
                    "the requested schema cannot be checked: {detail}"
                )
            }
        }
    }
}

impl<E: Error + 'static> Error for StructuredError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Backend { source, .. } => Some(source),
            Self::Schema { .. } | Self::UnsupportedSchema(_) => None,
        }
    }
}

impl<E> StructuredError<E> {
    /// A schema failure is terminal: the same request already failed twice,
    /// or its schema cannot be checked at all.
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Backend { class, .. } => *class,
            Self::Schema { .. } | Self::UnsupportedSchema(_) => ErrorClass::Terminal,
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
    complete_checked(backend, request, Ok)
}

/// Like [`complete_structured`], with a further `check` the parsed answer
/// must pass, such as references into another document. An answer must
/// satisfy the request's JSON Schema, then parse as `T`, then pass the
/// check; one that fails any of them is asked for once more with the
/// reason, then refused. A schema this build cannot check is refused before
/// anything is asked.
pub fn complete_checked<B: LlmBackend, T: DeserializeOwned, U>(
    backend: &B,
    request: &LlmRequest,
    check: impl Fn(T) -> Result<U, String>,
) -> Result<Structured<U>, StructuredError<B::Error>> {
    let schema = request
        .schema
        .as_ref()
        .map(|schema| {
            let value: serde_json::Value = serde_json::from_str(&schema.schema_json)
                .map_err(|error| format!("{:?} is not JSON ({error})", schema.name))?;
            kineto_json_schema::ensure_supported_schema(&value, "$")
                .map_err(|error| format!("{:?}: {error}", schema.name))?;
            Ok(value)
        })
        .transpose()
        .map_err(StructuredError::UnsupportedSchema)?;
    let unfitting =
        |error: String| format!("it does not satisfy the required JSON Schema ({error})");
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
        let checked = serde_json::from_str::<serde_json::Value>(json_body(&response.text))
            .map_err(|error| unfitting(error.to_string()))
            .and_then(|value| match &schema {
                Some(schema) => kineto_json_schema::validate_value(schema, &value, "$")
                    .map(|()| value)
                    .map_err(unfitting),
                None => Ok(value),
            })
            .and_then(|value| {
                serde_json::from_value::<T>(value).map_err(|error| unfitting(error.to_string()))
            })
            .and_then(|value| {
                check(value).map_err(|reason| format!("it is not consistent ({reason})"))
            });
        match checked {
            Ok(value) => {
                return Ok(Structured {
                    value,
                    response,
                    attempts,
                });
            }
            Err(reason) if attempts == 1 => {
                request.messages.push(Message::assistant(response.text));
                request.messages.push(Message::user(format!(
                    "That reply was not accepted: {reason}. Reply again with only the JSON \
                     value, no prose and no code fence."
                )));
            }
            Err(detail) => return Err(StructuredError::Schema { detail }),
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
        assert!(
            retry.messages[3]
                .content
                .contains("missing required property \"title\"")
        );

        let backend = FakeLlm::answering(["prose", "more prose"]);
        let error = complete_structured::<_, Title>(&backend, &request()).unwrap_err();
        assert!(matches!(error, StructuredError::Schema { .. }));
        assert_eq!(error.class(), ErrorClass::Terminal);
        assert!(!error.to_string().contains("more prose"));
    }

    #[test]
    fn an_answer_that_fails_its_check_is_asked_for_once_more_then_refused() {
        let check = |title: Title| {
            if title.title.starts_with("The") {
                Ok(title.title.len())
            } else {
                Err(format!("{:?} does not start with The", title.title))
            }
        };
        let backend = FakeLlm::answering([r#"{"title":"Wait"}"#, r#"{"title":"The Wait"}"#]);
        let structured = complete_checked(&backend, &request(), check).unwrap();
        assert_eq!(structured.value, 8);
        assert!(
            backend.requests()[1].messages[3]
                .content
                .contains("\"Wait\" does not start with The")
        );

        let backend = FakeLlm::answering([r#"{"title":"Wait"}"#, r#"{"title":"Still"}"#]);
        let error = complete_checked(&backend, &request(), check).unwrap_err();
        assert!(matches!(error, StructuredError::Schema { .. }));
        assert!(error.to_string().contains("\"Still\" does not start"));
    }

    #[test]
    fn an_answer_the_schema_refuses_is_asked_for_again_though_it_parses() {
        let mut request = request();
        request.schema.as_mut().unwrap().schema_json = r#"{
            "type": "object",
            "required": ["title"],
            "properties": {"title": {"type": "string", "minLength": 3}}
        }"#
        .to_owned();
        // `{"title":"ab"}` is a valid `Title`; only the schema refuses it.
        let backend = FakeLlm::answering([r#"{"title":"ab"}"#, r#"{"title":"abc"}"#]);
        let structured: Structured<Title> = complete_structured(&backend, &request).unwrap();
        assert_eq!(structured.value.title, "abc");
        assert!(
            backend.requests()[1].messages[3]
                .content
                .contains("$.title: string is shorter than minLength 3")
        );

        let backend = FakeLlm::answering([r#"{"title":"ab"}"#, r#"{"title":"a"}"#]);
        let error = complete_structured::<_, Title>(&backend, &request).unwrap_err();
        assert!(matches!(error, StructuredError::Schema { .. }));
        assert!(error.to_string().contains("minLength 3"));
    }

    #[test]
    fn a_schema_that_cannot_be_checked_asks_nothing() {
        let mut request = request();
        request.schema.as_mut().unwrap().schema_json =
            r#"{"type":"object","oneOf":[{"required":["title"]}]}"#.to_owned();
        let backend = FakeLlm::answering([r#"{"title":"The Last Line"}"#]);
        let error = complete_structured::<_, Title>(&backend, &request).unwrap_err();
        assert!(matches!(error, StructuredError::UnsupportedSchema(_)));
        assert_eq!(error.class(), ErrorClass::Terminal);
        assert!(backend.requests().is_empty());
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
