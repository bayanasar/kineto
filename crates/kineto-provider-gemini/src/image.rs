//! Gemini's image models over `generateContent`, for a connection the
//! director adds (ADR-0009): the prompt and any reference images as parts
//! of one turn, an image asked for as the answer, the frame as its aspect
//! ratio. One call answers at once with the image inline.
//!
//! A refusal (a blocked prompt, an image withheld for safety) arrives as an
//! answer without an image: nothing was made, and asking again cannot pay
//! twice for one image. Fields the director adds in the settings file are
//! sent at the top of the request too, and win.

use std::{error::Error, fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_jobs::ErrorClass;
use kineto_providers::{ImageFormat, secret::SecretString};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::{HttpsTransport, SharedTransport, Transport, TransportFailure, clipped};

/// The frames Gemini's image models draw.
pub const ASPECT_RATIOS: [(u16, u16); 10] = [
    (1, 1),
    (2, 3),
    (3, 2),
    (3, 4),
    (4, 3),
    (4, 5),
    (5, 4),
    (9, 16),
    (16, 9),
    (21, 9),
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageError {
    Transport(TransportFailure),
    /// The API answered with an error status and, usually, its words.
    Http {
        status: u16,
        kind: Option<String>,
        message: String,
    },
    /// Answered without an image, and why.
    Refused(String),
    Malformed(&'static str),
}

impl fmt::Display for ImageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(failure) => write!(formatter, "the Gemini request failed: {failure:?}"),
            Self::Http {
                status,
                kind,
                message,
            } => write!(
                formatter,
                "the Gemini API answered HTTP {status} ({}): {message}",
                kind.as_deref().unwrap_or("no error status")
            ),
            Self::Refused(why) => write!(formatter, "Gemini made no image: {why}"),
            Self::Malformed(what) => write!(formatter, "the Gemini answer is malformed: {what}"),
        }
    }
}

impl Error for ImageError {}

impl ImageError {
    /// Whether the call certainly left no image behind to pay for twice:
    /// it never reached the API, the API refused it (4xx), or it answered
    /// without an image.
    #[must_use]
    pub const fn certainly_not_accepted(&self) -> bool {
        match self {
            Self::Transport(failure) => {
                matches!(failure, TransportFailure::Connect | TransportFailure::Tls)
            }
            Self::Http { status, .. } => *status >= 400 && *status < 500,
            Self::Refused(_) => true,
            Self::Malformed(_) => false,
        }
    }

    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(_) => ErrorClass::Retryable,
            Self::Http {
                status, message, ..
            } => {
                let spent = ["quota", "billing", "credit"]
                    .iter()
                    .any(|word| message.to_lowercase().contains(word));
                if (*status == 429 && !spent) || *status >= 500 {
                    ErrorClass::Retryable
                } else {
                    ErrorClass::Terminal
                }
            }
            Self::Refused(_) | Self::Malformed(_) => ErrorClass::Terminal,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireAnswer {
    #[serde(default)]
    candidates: Vec<WireCandidate>,
    #[serde(default)]
    prompt_feedback: Option<WireFeedback>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireCandidate {
    #[serde(default)]
    content: Option<WireContent>,
    #[serde(default)]
    finish_reason: Option<String>,
    #[serde(default)]
    finish_message: Option<String>,
}

#[derive(Deserialize)]
struct WireContent {
    #[serde(default)]
    parts: Vec<WirePart>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WirePart {
    #[serde(default)]
    inline_data: Option<WireData>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireData {
    #[serde(default)]
    mime_type: String,
    data: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireFeedback {
    #[serde(default)]
    block_reason: Option<String>,
}

#[derive(Deserialize)]
struct WireErrorBody {
    error: WireError,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    status: Option<String>,
}

/// One image model on the Gemini API, or on a service that speaks its
/// protocol.
pub struct GeminiImages<T = SharedTransport> {
    transport: T,
    base_url: String,
    model: String,
    key: SecretString,
    /// Fields the director added to every request.
    extra: Map<String, Value>,
}

impl GeminiImages {
    /// `model` at `base_url` (for example
    /// `https://generativelanguage.googleapis.com/v1beta`) with `key`, and
    /// the fields the director added.
    #[must_use]
    pub fn new(base_url: &str, model: &str, key: SecretString, extra: Map<String, Value>) -> Self {
        Self::with_transport(Arc::new(HttpsTransport::new()), base_url, model, key, extra)
    }
}

impl<T> fmt::Debug for GeminiImages<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeminiImages")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl<T: Transport> GeminiImages<T> {
    pub fn with_transport(
        transport: T,
        base_url: &str,
        model: &str,
        key: SecretString,
        extra: Map<String, Value>,
    ) -> Self {
        Self {
            transport,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.trim_start_matches("models/").to_owned(),
            key,
            extra,
        }
    }

    /// One image of `prompt`, framed `width:height` when the model draws
    /// that frame, following `references` when there are any.
    pub fn generate(
        &self,
        prompt: &str,
        frame: (u16, u16),
        references: &[&[u8]],
    ) -> Result<Vec<u8>, ImageError> {
        let mut parts: Vec<Value> = references
            .iter()
            .map(|bytes| {
                json!({"inlineData": {
                    "mimeType": ImageFormat::sniff(bytes).unwrap_or(ImageFormat::Png).mime(),
                    "data": STANDARD.encode(bytes),
                }})
            })
            .collect();
        parts.push(json!({ "text": prompt }));
        let mut config = json!({ "responseModalities": ["IMAGE"] });
        if ASPECT_RATIOS.contains(&frame) {
            config["imageConfig"] = json!({ "aspectRatio": format!("{}:{}", frame.0, frame.1) });
        }
        let mut body = Map::new();
        body.insert(
            "contents".to_owned(),
            json!([{ "role": "user", "parts": parts }]),
        );
        body.insert("generationConfig".to_owned(), config);
        body.extend(self.extra.clone());
        let body = serde_json::to_vec(&Value::Object(body))
            .map_err(|_| ImageError::Malformed("the request did not encode"))?;
        let response = self
            .transport
            .post_json(
                &format!("{}/models/{}:generateContent", self.base_url, self.model),
                &self.key,
                &body,
            )
            .map_err(ImageError::Transport)?;
        if !(200..300).contains(&response.status) {
            let error = serde_json::from_slice::<WireErrorBody>(&response.body).ok();
            return Err(ImageError::Http {
                status: response.status,
                kind: error.as_ref().and_then(|body| body.error.status.clone()),
                message: error.map_or_else(String::new, |body| clipped(&body.error.message)),
            });
        }
        let answer: WireAnswer = serde_json::from_slice(&response.body)
            .map_err(|_| ImageError::Malformed("the body is not a generateContent answer"))?;
        if let Some(reason) = answer
            .prompt_feedback
            .and_then(|feedback| feedback.block_reason)
        {
            return Err(ImageError::Refused(format!(
                "the prompt was blocked ({reason})"
            )));
        }
        let candidate = answer
            .candidates
            .into_iter()
            .next()
            .ok_or(ImageError::Malformed("no candidate"))?;
        let parts = candidate
            .content
            .map(|content| content.parts)
            .unwrap_or_default();
        if let Some(data) = parts
            .iter()
            .filter_map(|part| part.inline_data.as_ref())
            .find(|data| data.mime_type.starts_with("image/"))
        {
            let bytes = STANDARD
                .decode(data.data.trim())
                .map_err(|_| ImageError::Malformed("the image is not base64"))?;
            ImageFormat::sniff(&bytes)
                .ok_or(ImageError::Malformed("the answer is not an image"))?;
            return Ok(bytes);
        }
        // Words instead of an image: say what it said, and why it stopped.
        let said = parts
            .iter()
            .filter_map(|part| part.text.as_deref())
            .collect::<Vec<_>>()
            .join(" ");
        let reason = candidate
            .finish_reason
            .unwrap_or_else(|| "no reason".to_owned());
        let detail = candidate.finish_message.unwrap_or(said);
        Err(ImageError::Refused(if detail.trim().is_empty() {
            reason
        } else {
            format!("{reason}: {}", clipped(&detail))
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HttpResponse;
    use std::sync::Mutex;

    const KEY: &str = "AIza-test-0123456789";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0jpeg";

    struct Recorded {
        answer: Result<HttpResponse, TransportFailure>,
        sent: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl Transport for Recorded {
        fn post_json(
            &self,
            url: &str,
            api_key: &SecretString,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            self.sent.lock().unwrap().push((
                url.to_owned(),
                api_key.expose().to_owned(),
                body.to_vec(),
            ));
            self.answer.clone()
        }

        fn get(&self, _: &str, _: &SecretString, _: u64) -> Result<HttpResponse, TransportFailure> {
            unreachable!("an image answers at once")
        }

        fn download(
            &self,
            _: &str,
            _: &SecretString,
            _: u64,
        ) -> Result<HttpResponse, TransportFailure> {
            unreachable!("an image answers inline")
        }
    }

    fn client(
        answer: Result<HttpResponse, TransportFailure>,
        extra: Map<String, Value>,
    ) -> GeminiImages<Recorded> {
        GeminiImages::with_transport(
            Recorded {
                answer,
                sent: Mutex::new(Vec::new()),
            },
            "https://generativelanguage.example.test/v1beta/",
            "models/gemini-image-test",
            SecretString::new(KEY.to_owned()).unwrap(),
            extra,
        )
    }

    fn json(status: u16, body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    fn image(bytes: &[u8]) -> Result<HttpResponse, TransportFailure> {
        json(
            200,
            &format!(
                r#"{{"candidates":[{{"content":{{"parts":[{{"text":"Here it is."}},{{"inlineData":{{"mimeType":"image/png","data":"{}"}}}}]}},"finishReason":"STOP"}}]}}"#,
                STANDARD.encode(bytes)
            ),
        )
    }

    #[test]
    fn an_image_is_asked_for_in_its_frame_with_its_references() {
        let mut extra = Map::new();
        extra.insert("safetySettings".to_owned(), json!([]));
        let client = client(image(PNG), extra);
        assert_eq!(
            client
                .generate("the same woman, older", (16, 9), &[JPEG])
                .unwrap(),
            PNG
        );
        let sent = client.transport.sent.lock().unwrap();
        let (url, key, body) = &sent[0];
        assert_eq!(
            url,
            "https://generativelanguage.example.test/v1beta/models/gemini-image-test:generateContent"
        );
        assert_eq!(key, KEY);
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            body["generationConfig"]["responseModalities"],
            json!(["IMAGE"])
        );
        assert_eq!(
            body["generationConfig"]["imageConfig"]["aspectRatio"],
            "16:9"
        );
        let parts = body["contents"][0]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["inlineData"]["mimeType"], "image/jpeg");
        assert_eq!(parts[0]["inlineData"]["data"], STANDARD.encode(JPEG));
        assert_eq!(parts[1]["text"], "the same woman, older");
        assert_eq!(body["safetySettings"], json!([]));
        assert!(!String::from_utf8_lossy(&sent[0].2).contains(KEY));
    }

    #[test]
    fn a_frame_it_does_not_draw_is_left_to_the_prompt() {
        let client = client(image(PNG), Map::new());
        client.generate("a door", (7, 3), &[]).unwrap();
        let body: Value =
            serde_json::from_slice(&client.transport.sent.lock().unwrap()[0].2).unwrap();
        assert!(body["generationConfig"].get("imageConfig").is_none());
    }

    #[test]
    fn an_answer_without_an_image_is_a_refusal_that_cost_nothing_twice() {
        for (answer, says) in [
            (
                r#"{"promptFeedback":{"blockReason":"PROHIBITED_CONTENT"}}"#,
                "the prompt was blocked (PROHIBITED_CONTENT)",
            ),
            (
                r#"{"candidates":[{"content":{"parts":[{"text":"I cannot draw that."}]},"finishReason":"STOP"}]}"#,
                "STOP: I cannot draw that.",
            ),
            (
                r#"{"candidates":[{"finishReason":"IMAGE_SAFETY","finishMessage":"Unable to show the generated image."}]}"#,
                "IMAGE_SAFETY: Unable to show the generated image.",
            ),
        ] {
            let error = client(json(200, answer), Map::new())
                .generate("a door", (16, 9), &[])
                .unwrap_err();
            assert_eq!(error, ImageError::Refused(says.to_owned()));
            assert!(error.certainly_not_accepted());
            assert_eq!(error.class(), ErrorClass::Terminal);
        }
    }

    #[test]
    fn failures_say_whether_anything_was_made() {
        let cases = [
            (Err(TransportFailure::Connect), true, ErrorClass::Retryable),
            (Err(TransportFailure::Timeout), false, ErrorClass::Retryable),
            (
                json(
                    400,
                    r#"{"error":{"code":400,"message":"API key not valid","status":"INVALID_ARGUMENT"}}"#,
                ),
                true,
                ErrorClass::Terminal,
            ),
            (
                json(
                    429,
                    r#"{"error":{"code":429,"message":"Resource has been exhausted","status":"RESOURCE_EXHAUSTED"}}"#,
                ),
                true,
                ErrorClass::Retryable,
            ),
            (
                json(
                    429,
                    r#"{"error":{"code":429,"message":"You exceeded your current quota","status":"RESOURCE_EXHAUSTED"}}"#,
                ),
                true,
                ErrorClass::Terminal,
            ),
            (json(503, "<html>"), false, ErrorClass::Retryable),
            (json(200, "{}"), false, ErrorClass::Terminal),
            (json(200, "not json"), false, ErrorClass::Terminal),
        ];
        for (answer, refused, class) in cases {
            let client = client(answer, Map::new());
            let error = client.generate("a door", (16, 9), &[]).unwrap_err();
            assert_eq!(error.certainly_not_accepted(), refused, "{error}");
            assert_eq!(error.class(), class, "{error}");
            assert!(!format!("{error} {error:?} {client:?}").contains(KEY));
        }
        let error = client(
            json(
                404,
                r#"{"error":{"code":404,"message":"models/x is not found","status":"NOT_FOUND"}}"#,
            ),
            Map::new(),
        )
        .generate("a door", (16, 9), &[])
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the Gemini API answered HTTP 404 (NOT_FOUND): models/x is not found"
        );
    }
}
