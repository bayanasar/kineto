//! An `llm.complete` backend for any API that speaks Anthropic's messages
//! protocol: Claude's own API, and the Anthropic-compatible endpoints other
//! vendors serve beside their OpenAI-compatible ones.
//!
//! The key goes in `x-api-key`, as Anthropic's SDKs send it, with the
//! protocol version in `anthropic-version`. The address is the API's base,
//! with or without its `/v1`: `https://api.anthropic.com` and
//! `https://api.anthropic.com/v1` both reach `…/v1/messages`.

use std::{error::Error, fmt, sync::Arc};

use kineto_jobs::ErrorClass;
use kineto_providers::{LlmCapabilities, secret::SecretString};
use serde::{Deserialize, Serialize};

use crate::{
    Billing, LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse, Role, TokenUsage,
    http::{Auth, HttpTransport, SharedTransport, Transport, TransportFailure},
    openai::connection_provider,
};

/// The protocol version every request names.
pub const API_VERSION: &str = "2023-06-01";
/// What a request may write when the caller sets no limit: the protocol
/// requires one.
pub const DEFAULT_MAX_TOKENS: u32 = 16_384;
const MAX_ERROR_MESSAGE_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessagesError {
    Transport(TransportFailure),
    /// The API answered with an error status and, usually, an error body.
    Http {
        status: u16,
        kind: Option<String>,
        message: String,
    },
    /// The answer was cut off at the token limit.
    Truncated,
    /// The model declined to answer.
    Refused,
    Malformed(&'static str),
}

impl fmt::Display for MessagesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(failure) => write!(formatter, "LLM request failed: {failure:?}"),
            Self::Http {
                status,
                kind,
                message,
            } => write!(
                formatter,
                "LLM API answered HTTP {status} ({}): {message}",
                kind.as_deref().unwrap_or("no error type")
            ),
            Self::Truncated => formatter.write_str("the answer was cut off at its token limit"),
            Self::Refused => formatter.write_str("the model declined to answer"),
            Self::Malformed(what) => write!(formatter, "LLM API answer is malformed: {what}"),
        }
    }
}

impl Error for MessagesError {}

impl MessagesError {
    /// Whether the request may have reached the model and been answered,
    /// and charged for, without the answer arriving. As for the OpenAI
    /// protocol: a broken or timed-out connection after sending, or a
    /// server failure, may have; anything else did not.
    #[must_use]
    pub const fn may_have_lost_an_answer(&self) -> bool {
        match self {
            Self::Transport(failure) => matches!(
                failure,
                TransportFailure::Timeout | TransportFailure::Io | TransportFailure::Protocol
            ),
            Self::Http { status, .. } => *status >= 500,
            Self::Truncated | Self::Refused | Self::Malformed(_) => false,
        }
    }

    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(failure) if *failure != TransportFailure::Insecure => {
                ErrorClass::Retryable
            }
            // 429 is a rate limit, 529 an overloaded API: both pass. A
            // spent balance answers 400 with its own words.
            Self::Http { status, .. } if *status == 429 || *status >= 500 => ErrorClass::Retryable,
            Self::Transport(_)
            | Self::Http { .. }
            | Self::Truncated
            | Self::Refused
            | Self::Malformed(_) => ErrorClass::Terminal,
        }
    }
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<WireMessage<'a>>,
    stream: bool,
}

#[derive(Deserialize)]
struct WireAnswer {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    content: Vec<WireBlock>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireBlock {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

#[derive(Deserialize)]
struct WireErrorBody {
    error: WireError,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    message: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}

/// A messages backend.
pub struct AnthropicMessages<T = SharedTransport> {
    transport: T,
    url: String,
    model: String,
    api_key: Option<SecretString>,
    provider: String,
    billing: Billing,
}

impl AnthropicMessages {
    /// The connection `name` the director added (`connection.<name>` keeps
    /// its records), at `base_url` serving `model`, with its key if it takes
    /// one, at the price the director gave.
    #[must_use]
    pub fn connection(
        name: &str,
        base_url: &str,
        model: &str,
        api_key: Option<SecretString>,
        billing: Billing,
    ) -> Self {
        Self::with_transport(
            Arc::new(HttpTransport::new()),
            base_url,
            model,
            api_key,
            &connection_provider(name),
            billing,
        )
    }
}

/// Where the messages of the API at `base_url` are posted.
fn messages_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

impl<T: Transport> AnthropicMessages<T> {
    pub fn with_transport(
        transport: T,
        base_url: &str,
        model: &str,
        api_key: Option<SecretString>,
        provider: &str,
        billing: Billing,
    ) -> Self {
        Self {
            transport,
            url: messages_url(base_url),
            model: model.to_owned(),
            api_key,
            provider: provider.to_owned(),
            billing,
        }
    }
}

impl<T> fmt::Debug for AnthropicMessages<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AnthropicMessages")
            .field("url", &self.url)
            .field("model", &self.model)
            .field("api_key", &self.api_key)
            .finish_non_exhaustive()
    }
}

fn schema_instruction(name: &str, schema: &str) -> String {
    format!(
        "Answer with a single JSON value that satisfies the JSON Schema `{name}` below. \
         No prose, no code fence.\n\n{schema}"
    )
}

impl<T: Transport> LlmBackend for AnthropicMessages<T> {
    type Error = MessagesError;

    fn info(&self) -> LlmBackendInfo {
        LlmBackendInfo {
            provider: self.provider.clone(),
            model: self.model.clone(),
            capabilities: LlmCapabilities {
                context_window_tokens: None,
                tools: false,
                structured_output: true,
                vision_input: false,
            },
            billing: self.billing,
        }
    }

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, MessagesError> {
        // The protocol keeps the system prompt apart from the turns.
        let system = request
            .schema
            .as_ref()
            .map(|schema| schema_instruction(&schema.name, &schema.schema_json))
            .into_iter()
            .chain(
                request
                    .messages
                    .iter()
                    .filter(|message| message.role == Role::System)
                    .map(|message| message.content.clone()),
            )
            .collect::<Vec<_>>();
        let messages = request
            .messages
            .iter()
            .filter(|message| message.role != Role::System)
            .map(|message| WireMessage {
                role: message.role.as_str(),
                content: &message.content,
            })
            .collect();
        let body = serde_json::to_vec(&WireRequest {
            model: &self.model,
            max_tokens: request.max_output_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
            system: (!system.is_empty()).then(|| system.join("\n\n")),
            messages,
            stream: false,
        })
        .map_err(|_| MessagesError::Malformed("request did not encode"))?;
        let response = self
            .transport
            .post_json(
                &self.url,
                self.api_key.as_ref().map(Auth::ApiKey),
                &[("anthropic-version", API_VERSION)],
                &body,
            )
            .map_err(MessagesError::Transport)?;

        if !(200..300).contains(&response.status) {
            let error = serde_json::from_slice::<WireErrorBody>(&response.body).ok();
            return Err(MessagesError::Http {
                status: response.status,
                kind: error.as_ref().and_then(|body| body.error.kind.clone()),
                message: error.map_or_else(String::new, |body| {
                    body.error
                        .message
                        .chars()
                        .filter(|character| !character.is_control())
                        .take(MAX_ERROR_MESSAGE_CHARS)
                        .collect()
                }),
            });
        }
        let answer: WireAnswer = serde_json::from_slice(&response.body)
            .map_err(|_| MessagesError::Malformed("body is not a message"))?;
        match answer.stop_reason.as_deref() {
            Some("max_tokens") => return Err(MessagesError::Truncated),
            Some("refusal") => return Err(MessagesError::Refused),
            _ => {}
        }
        // Thinking and other blocks are not the answer.
        let text = answer
            .content
            .into_iter()
            .filter(|block| block.kind == "text")
            .filter_map(|block| block.text)
            .collect::<String>();
        if text.trim().is_empty() {
            return Err(MessagesError::Malformed("the answer is empty"));
        }
        Ok(LlmResponse {
            text,
            usage: answer.usage.map(|usage| TokenUsage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
            }),
            model: answer.model.unwrap_or_else(|| self.model.clone()),
        })
    }

    fn classify(&self, error: &MessagesError) -> ErrorClass {
        error.class()
    }

    fn may_have_lost_an_answer(&self, error: &MessagesError) -> bool {
        error.may_have_lost_an_answer()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, OutputSchema, http::HttpResponse};
    use std::sync::Mutex;

    const KEY: &str = "sk-ant-test-0123456789";

    /// URL, key, headers and body of one request.
    type Sent = (String, Option<String>, Vec<(String, String)>, Vec<u8>);

    struct Recorded {
        answer: Result<HttpResponse, TransportFailure>,
        sent: Mutex<Vec<Sent>>,
    }

    impl Transport for Recorded {
        fn post_json(
            &self,
            url: &str,
            auth: Option<Auth<'_>>,
            headers: &[(&str, &str)],
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            assert!(
                matches!(auth, None | Some(Auth::ApiKey(_))),
                "the messages protocol sends its key as x-api-key"
            );
            self.sent.lock().unwrap().push((
                url.to_owned(),
                auth.map(|auth| auth.key().expose().to_owned()),
                headers
                    .iter()
                    .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
                    .collect(),
                body.to_vec(),
            ));
            self.answer.clone()
        }
    }

    fn backend(answer: Result<HttpResponse, TransportFailure>) -> AnthropicMessages<Recorded> {
        AnthropicMessages::with_transport(
            Recorded {
                answer,
                sent: Mutex::new(Vec::new()),
            },
            "https://api.example.test/anthropic/",
            "claude-test",
            SecretString::new(KEY.to_owned()),
            "connection.claude",
            Billing::Unpriced,
        )
    }

    fn json(status: u16, body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    fn request() -> LlmRequest {
        LlmRequest {
            messages: vec![
                Message::system("You analyse stories."),
                Message::user("Text."),
                Message::assistant("{}"),
                Message::user("Again."),
            ],
            schema: Some(OutputSchema {
                name: "analysis".to_owned(),
                schema_json: r#"{"type":"object"}"#.to_owned(),
            }),
            max_output_tokens: None,
        }
    }

    #[test]
    fn the_address_reaches_its_messages_with_or_without_its_version() {
        assert_eq!(
            messages_url("https://api.anthropic.com"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://api.anthropic.com/v1/"),
            "https://api.anthropic.com/v1/messages"
        );
        assert_eq!(
            messages_url("https://api.minimax.io/anthropic"),
            "https://api.minimax.io/anthropic/v1/messages"
        );
    }

    #[test]
    fn a_message_is_sent_and_read() {
        let backend = backend(json(
            200,
            r#"{"id":"msg_1","type":"message","role":"assistant","model":"claude-test-20261001","content":[{"type":"thinking","thinking":"hm"},{"type":"text","text":"{\"ok\":"},{"type":"text","text":"true}"}],"stop_reason":"end_turn","usage":{"input_tokens":120,"output_tokens":8}}"#,
        ));
        let response = backend.complete(&request()).unwrap();
        assert_eq!(response.text, r#"{"ok":true}"#);
        assert_eq!(response.model, "claude-test-20261001");
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                input_tokens: 120,
                output_tokens: 8
            })
        );

        let sent = backend.transport.sent.lock().unwrap();
        let (url, key, headers, body) = &sent[0];
        assert_eq!(url, "https://api.example.test/anthropic/v1/messages");
        assert_eq!(key.as_deref(), Some(KEY));
        assert_eq!(
            headers,
            &[("anthropic-version".to_owned(), API_VERSION.to_owned())]
        );
        let body: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["model"], "claude-test");
        assert_eq!(body["max_tokens"], DEFAULT_MAX_TOKENS);
        assert_eq!(body["stream"], false);
        let system = body["system"].as_str().unwrap();
        assert!(
            system.starts_with("Answer with a single JSON value"),
            "{system}"
        );
        assert!(system.contains("`analysis`") && system.ends_with("You analyse stories."));
        let roles = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["role"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(roles, ["user", "assistant", "user"], "no system turn");
        assert!(!String::from_utf8_lossy(&sent[0].3).contains(KEY));
    }

    #[test]
    fn failures_are_classified() {
        let cases = [
            (
                json(
                    429,
                    r#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#,
                ),
                ErrorClass::Retryable,
            ),
            (
                json(
                    529,
                    r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
                ),
                ErrorClass::Retryable,
            ),
            (
                json(
                    400,
                    r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low"}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                json(
                    401,
                    r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (json(502, "<html>"), ErrorClass::Retryable),
            (Err(TransportFailure::Timeout), ErrorClass::Retryable),
            (Err(TransportFailure::Insecure), ErrorClass::Terminal),
            (
                json(
                    200,
                    r#"{"content":[{"type":"text","text":"half"}],"stop_reason":"max_tokens"}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                json(200, r#"{"content":[],"stop_reason":"refusal"}"#),
                ErrorClass::Terminal,
            ),
            (
                json(200, r#"{"content":[{"type":"text","text":" "}]}"#),
                ErrorClass::Terminal,
            ),
            (json(200, "not json"), ErrorClass::Terminal),
        ];
        for (answer, class) in cases {
            let backend = backend(answer);
            let error = backend.complete(&request()).unwrap_err();
            assert_eq!(backend.classify(&error), class, "{error}");
            assert!(!format!("{error} {error:?}").contains(KEY));
        }
        let message = backend(json(
            400,
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low"}}"#,
        ))
        .complete(&request())
        .unwrap_err()
        .to_string();
        assert!(message.contains("credit balance"), "{message}");
        assert!(!format!("{:?}", backend(Err(TransportFailure::Io))).contains(KEY));
    }

    #[test]
    fn only_a_call_that_may_have_reached_the_model_may_have_lost_an_answer() {
        let http = |status| MessagesError::Http {
            status,
            kind: None,
            message: String::new(),
        };
        for lost in [
            MessagesError::Transport(TransportFailure::Timeout),
            http(500),
            http(529),
        ] {
            assert!(lost.may_have_lost_an_answer(), "{lost}");
        }
        for kept in [
            MessagesError::Transport(TransportFailure::Connect),
            http(400),
            http(429),
            MessagesError::Truncated,
            MessagesError::Refused,
        ] {
            assert!(!kept.may_have_lost_an_answer(), "{kept}");
        }
    }

    #[test]
    fn a_connection_is_recorded_under_its_name() {
        let backend = AnthropicMessages::connection(
            "claude",
            "https://api.anthropic.com",
            "claude-sonnet-5-5",
            None,
            Billing::Unpriced,
        );
        assert_eq!(backend.info().provider, "connection.claude");
        assert_eq!(backend.info().model, "claude-sonnet-5-5");
    }
}
