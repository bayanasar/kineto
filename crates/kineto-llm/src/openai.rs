//! An `llm.complete` backend for any API that speaks the OpenAI
//! chat-completions protocol: Kimi's open platform by default, or a local
//! model server on a loopback address.

use std::{error::Error, fmt, sync::Arc};

use kineto_jobs::ErrorClass;
use kineto_providers::{LlmCapabilities, secret::SecretString};
use serde::{Deserialize, Serialize};

use crate::{
    Billing, LlmBackend, LlmBackendInfo, LlmRequest, LlmResponse, TokenUsage,
    http::{HttpTransport, SharedTransport, Transport, TransportFailure},
};

/// The provider key Kimi API jobs are recorded under, and the name its key
/// is configured by (`KINETO_KIMI_API_KEY`).
pub const KIMI_PROVIDER: &str = "kimi";
/// Kimi's open platform.
pub const KIMI_BASE_URL: &str = "https://api.moonshot.ai/v1";
/// Kimi's flagship model.
pub const KIMI_DEFAULT_MODEL: &str = "kimi-k3";
/// The provider key local model servers are recorded under.
pub const LOCAL_PROVIDER: &str = "local-llm";
const MAX_ERROR_MESSAGE_CHARS: usize = 200;

/// Published Kimi prices, USD micros per million tokens (input on a cache
/// miss, output).
fn kimi_billing(model: &str) -> Billing {
    let (input, output) = match model {
        "kimi-k3" => (3_000_000, 15_000_000),
        "kimi-k2.6" | "kimi-k2.7-code" => (950_000, 4_000_000),
        "kimi-k2.7-code-highspeed" => (1_900_000, 8_000_000),
        _ => return Billing::Unpriced,
    };
    Billing::PerToken {
        input_micros_per_million: input,
        output_micros_per_million: output,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiError {
    Transport(TransportFailure),
    /// The API answered with an error status and, usually, an error body.
    Http {
        status: u16,
        kind: Option<String>,
        message: String,
    },
    /// The answer was cut off at the token limit.
    Truncated,
    /// The provider withheld the answer on content grounds.
    Filtered,
    Malformed(&'static str),
}

impl fmt::Display for ApiError {
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
            Self::Filtered => formatter.write_str("the provider withheld the answer"),
            Self::Malformed(what) => write!(formatter, "LLM API answer is malformed: {what}"),
        }
    }
}

impl Error for ApiError {}

impl ApiError {
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(failure) if *failure != TransportFailure::Insecure => {
                ErrorClass::Retryable
            }
            Self::Http { status, kind, .. } => {
                let quota = kind.as_deref().is_some_and(|kind| {
                    kind.contains("quota") || kind.contains("balance") || kind.contains("billing")
                });
                if (*status == 429 && !quota) || *status >= 500 {
                    ErrorClass::Retryable
                } else {
                    ErrorClass::Terminal
                }
            }
            Self::Transport(_) | Self::Truncated | Self::Filtered | Self::Malformed(_) => {
                ErrorClass::Terminal
            }
        }
    }
}

#[derive(Serialize)]
struct WireMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Serialize)]
struct WireResponseFormat {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<WireResponseFormat>,
    stream: bool,
}

#[derive(Deserialize)]
struct WireAnswer {
    #[serde(default)]
    model: Option<String>,
    choices: Vec<WireChoice>,
    #[serde(default)]
    usage: Option<WireUsage>,
}

#[derive(Deserialize)]
struct WireChoice {
    message: WireAnswerMessage,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireAnswerMessage {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct WireUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
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

/// A chat-completions backend.
pub struct OpenAiCompatible<T = SharedTransport> {
    transport: T,
    base_url: String,
    model: String,
    api_key: Option<SecretString>,
    provider: String,
    billing: Billing,
}

impl OpenAiCompatible {
    /// Kimi's open platform with `api_key`. `model` and `base_url` override
    /// the defaults.
    #[must_use]
    pub fn kimi(api_key: SecretString, model: Option<&str>, base_url: Option<&str>) -> Self {
        let model = model.unwrap_or(KIMI_DEFAULT_MODEL);
        Self::with_transport(
            Arc::new(HttpTransport::new()),
            base_url.unwrap_or(KIMI_BASE_URL),
            model,
            Some(api_key),
            KIMI_PROVIDER,
            kimi_billing(model),
        )
    }

    /// A local model server, for example `http://localhost:11434/v1`.
    #[must_use]
    pub fn local(base_url: &str, model: &str) -> Self {
        Self::with_transport(
            Arc::new(HttpTransport::new()),
            base_url,
            model,
            None,
            LOCAL_PROVIDER,
            Billing::Free,
        )
    }
}

impl<T: Transport> OpenAiCompatible<T> {
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
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            api_key,
            provider: provider.to_owned(),
            billing,
        }
    }
}

impl<T> fmt::Debug for OpenAiCompatible<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OpenAiCompatible")
            .field("base_url", &self.base_url)
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

impl<T: Transport> LlmBackend for OpenAiCompatible<T> {
    type Error = ApiError;

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

    fn complete(&self, request: &LlmRequest) -> Result<LlmResponse, ApiError> {
        let instruction = request
            .schema
            .as_ref()
            .map(|schema| schema_instruction(&schema.name, &schema.schema_json));
        let messages = instruction
            .iter()
            .map(|content| WireMessage {
                role: "system",
                content,
            })
            .chain(request.messages.iter().map(|message| WireMessage {
                role: message.role.as_str(),
                content: &message.content,
            }))
            .collect();
        let body = serde_json::to_vec(&WireRequest {
            model: &self.model,
            messages,
            max_tokens: request.max_output_tokens,
            response_format: request.schema.as_ref().map(|_| WireResponseFormat {
                kind: "json_object",
            }),
            stream: false,
        })
        .map_err(|_| ApiError::Malformed("request did not encode"))?;
        let response = self
            .transport
            .post_json(
                &format!("{}/chat/completions", self.base_url),
                self.api_key.as_ref(),
                &body,
            )
            .map_err(ApiError::Transport)?;

        if !(200..300).contains(&response.status) {
            let error = serde_json::from_slice::<WireErrorBody>(&response.body).ok();
            return Err(ApiError::Http {
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
            .map_err(|_| ApiError::Malformed("body is not a chat completion"))?;
        let choice = answer
            .choices
            .into_iter()
            .next()
            .ok_or(ApiError::Malformed("no choice"))?;
        match choice.finish_reason.as_deref() {
            Some("length") => return Err(ApiError::Truncated),
            Some("content_filter") => return Err(ApiError::Filtered),
            _ => {}
        }
        let text = choice
            .message
            .content
            .filter(|text| !text.trim().is_empty())
            .ok_or(ApiError::Malformed("the answer is empty"))?;
        Ok(LlmResponse {
            text,
            usage: answer.usage.map(|usage| TokenUsage {
                input_tokens: usage.prompt_tokens,
                output_tokens: usage.completion_tokens,
            }),
            model: answer.model.unwrap_or_else(|| self.model.clone()),
        })
    }

    fn classify(&self, error: &ApiError) -> ErrorClass {
        error.class()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Message, OutputSchema, http::HttpResponse};
    use std::sync::Mutex;

    const KEY: &str = "sk-kimi-test-0123456789";

    /// URL, bearer and body of one request.
    type Sent = (String, Option<String>, Vec<u8>);

    struct Recorded {
        answer: Result<HttpResponse, TransportFailure>,
        sent: Mutex<Vec<Sent>>,
    }

    impl Transport for Recorded {
        fn post_json(
            &self,
            url: &str,
            bearer: Option<&SecretString>,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            self.sent.lock().unwrap().push((
                url.to_owned(),
                bearer.map(|key| key.expose().to_owned()),
                body.to_vec(),
            ));
            self.answer.clone()
        }
    }

    fn backend(answer: Result<HttpResponse, TransportFailure>) -> OpenAiCompatible<Recorded> {
        OpenAiCompatible::with_transport(
            Recorded {
                answer,
                sent: Mutex::new(Vec::new()),
            },
            "https://api.example.test/v1/",
            "kimi-k3",
            SecretString::new(KEY.to_owned()),
            KIMI_PROVIDER,
            kimi_billing("kimi-k3"),
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
            ],
            schema: Some(OutputSchema {
                name: "analysis".to_owned(),
                schema_json: r#"{"type":"object"}"#.to_owned(),
            }),
            max_output_tokens: Some(4096),
        }
    }

    #[test]
    fn a_completion_is_sent_and_read() {
        let backend = backend(json(
            200,
            r#"{"id":"c1","model":"kimi-k3","choices":[{"index":0,"message":{"role":"assistant","content":"{\"ok\":true}"},"finish_reason":"stop"}],"usage":{"prompt_tokens":120,"completion_tokens":8,"total_tokens":128}}"#,
        ));
        let response = backend.complete(&request()).unwrap();
        assert_eq!(response.text, r#"{"ok":true}"#);
        assert_eq!(
            response.usage,
            Some(TokenUsage {
                input_tokens: 120,
                output_tokens: 8
            })
        );

        let sent = backend.transport.sent.lock().unwrap();
        let (url, key, body) = &sent[0];
        assert_eq!(url, "https://api.example.test/v1/chat/completions");
        assert_eq!(key.as_deref(), Some(KEY));
        let body: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["model"], "kimi-k3");
        assert_eq!(body["response_format"]["type"], "json_object");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["stream"], false);
        let roles = body["messages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|message| message["role"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(roles, ["system", "system", "user"]);
        assert!(
            body["messages"][0]["content"]
                .as_str()
                .unwrap()
                .contains("`analysis`")
        );
        assert!(!String::from_utf8_lossy(&sent[0].2).contains(KEY));
    }

    #[test]
    fn failures_are_classified() {
        let cases = [
            (
                json(
                    429,
                    r#"{"error":{"message":"slow down","type":"rate_limit_reached_error"}}"#,
                ),
                ErrorClass::Retryable,
            ),
            (
                json(
                    429,
                    r#"{"error":{"message":"no money","type":"exceeded_current_quota_error"}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                json(
                    401,
                    r#"{"error":{"message":"bad key","type":"invalid_authentication_error"}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                json(
                    400,
                    r#"{"error":{"message":"too long","type":"invalid_request_error"}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (json(503, "<html>"), ErrorClass::Retryable),
            (Err(TransportFailure::Timeout), ErrorClass::Retryable),
            (Err(TransportFailure::Insecure), ErrorClass::Terminal),
            (
                json(
                    200,
                    r#"{"choices":[{"message":{"content":"half"},"finish_reason":"length"}]}"#,
                ),
                ErrorClass::Terminal,
            ),
            (
                json(
                    200,
                    r#"{"choices":[{"message":{"content":""},"finish_reason":"content_filter"}]}"#,
                ),
                ErrorClass::Terminal,
            ),
            (json(200, r#"{"choices":[]}"#), ErrorClass::Terminal),
            (json(200, "not json"), ErrorClass::Terminal),
        ];
        for (answer, class) in cases {
            let backend = backend(answer);
            let error = backend.complete(&request()).unwrap_err();
            assert_eq!(backend.classify(&error), class, "{error}");
            assert!(!format!("{error} {error:?}").contains(KEY));
        }
        assert!(!format!("{:?}", backend(Err(TransportFailure::Io))).contains(KEY));
    }

    #[test]
    fn kimi_prices_known_models_and_leaves_others_unpriced() {
        assert_eq!(
            kimi_billing("kimi-k2.6"),
            Billing::PerToken {
                input_micros_per_million: 950_000,
                output_micros_per_million: 4_000_000
            }
        );
        assert_eq!(kimi_billing("kimi-k9-future"), Billing::Unpriced);
        let local = OpenAiCompatible::local("http://localhost:11434/v1", "qwen3");
        assert_eq!(local.info().billing, Billing::Free);
        assert_eq!(local.info().provider, LOCAL_PROVIDER);
    }
}
