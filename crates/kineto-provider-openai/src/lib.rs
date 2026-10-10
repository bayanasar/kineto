//! OpenAI's images protocol, which many services speak besides OpenAI:
//! `POST <url>/images/generations` with a prompt, and `POST
//! <url>/images/edits` with reference images as multipart files. The image
//! comes back inline (`b64_json`) or at an address, fetched without the key.
//!
//! Only the protocol's common part is sent: the model, the prompt, one image
//! and a size. The sizes are the three OpenAI's own models take; a service
//! that refuses the size is asked once more without it, which its refusal
//! made free. Fields the director adds in the settings file are sent too,
//! and win.
//!
//! HTTPS everywhere, or plain HTTP to this computer; no redirects on the
//! API, so a key never follows one.

use std::{error::Error, fmt, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_jobs::ErrorClass;
use kineto_providers::{ImageFormat, net::is_permitted, secret::SecretString};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Largest image taken, inline or fetched.
pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024;
/// An image can take minutes to make.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const FETCH_TIMEOUT: Duration = Duration::from_secs(120);
const FETCH_ATTEMPTS: usize = 3;
const MAX_MESSAGE_CHARS: usize = 200;

/// What a transport failed at. Library error text is deliberately not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    Connect,
    Timeout,
    Tls,
    Io,
    TooLarge,
    Protocol,
    /// The address is not HTTPS and not this computer.
    Insecure,
}

/// An HTTP response as the client needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Talks to the service. Production uses [`HttpTransport`]; tests replay
/// recorded answers.
pub trait Transport {
    /// POST `body`, of `content_type`, to `url`, with the key as a bearer
    /// when there is one.
    fn post(
        &self,
        url: &str,
        key: Option<&SecretString>,
        content_type: &str,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure>;

    /// GET an image an answer pointed at, never with the key.
    fn fetch(&self, url: &str) -> Result<HttpResponse, TransportFailure>;
}

/// A transport shared between clients and threads.
pub type SharedTransport = Arc<dyn Transport + Send + Sync>;

impl Transport for SharedTransport {
    fn post(
        &self,
        url: &str,
        key: Option<&SecretString>,
        content_type: &str,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().post(url, key, content_type, body)
    }

    fn fetch(&self, url: &str) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().fetch(url)
    }
}

/// Blocking HTTPS with rustls and the bundled Mozilla roots.
#[derive(Clone)]
pub struct HttpTransport {
    api: ureq::Agent,
    files: ureq::Agent,
}

impl HttpTransport {
    #[must_use]
    pub fn new() -> Self {
        let agent = |timeout, redirects| -> ureq::Agent {
            ureq::Agent::config_builder()
                // Checked per request instead: HTTP to this computer is allowed.
                .https_only(false)
                .max_redirects(redirects)
                .http_status_as_error(false)
                .timeout_connect(Some(CONNECT_TIMEOUT))
                .timeout_global(Some(timeout))
                .user_agent(concat!("kineto/", env!("CARGO_PKG_VERSION")))
                .build()
                .into()
        };
        Self {
            // A redirect would carry the key to wherever it points.
            api: agent(REQUEST_TIMEOUT, 0),
            // A file is fetched without the key, wherever it was moved.
            files: agent(FETCH_TIMEOUT, 3),
        }
    }
}

impl Default for HttpTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for HttpTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HttpTransport")
    }
}

impl Transport for HttpTransport {
    fn post(
        &self,
        url: &str,
        key: Option<&SecretString>,
        content_type: &str,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        if !is_permitted(url) {
            return Err(TransportFailure::Insecure);
        }
        let mut request = self.api.post(url).header("Content-Type", content_type);
        if let Some(key) = key {
            request = request.header("Authorization", format!("Bearer {}", key.expose()));
        }
        read(request.send(body).map_err(failure)?)
    }

    fn fetch(&self, url: &str) -> Result<HttpResponse, TransportFailure> {
        if !is_permitted(url) {
            return Err(TransportFailure::Insecure);
        }
        read(self.files.get(url).call().map_err(failure)?)
    }
}

fn read(mut response: ureq::http::Response<ureq::Body>) -> Result<HttpResponse, TransportFailure> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_IMAGE_BYTES * 2)
        .read_to_vec()
        .map_err(failure)?;
    Ok(HttpResponse { status, body })
}

fn failure(error: ureq::Error) -> TransportFailure {
    match error {
        ureq::Error::Timeout(_) => TransportFailure::Timeout,
        ureq::Error::HostNotFound | ureq::Error::ConnectionFailed => TransportFailure::Connect,
        ureq::Error::Io(_) => TransportFailure::Io,
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) | ureq::Error::Pem(_) => TransportFailure::Tls,
        ureq::Error::BodyExceedsLimit(_) => TransportFailure::TooLarge,
        _ => TransportFailure::Protocol,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImagesError {
    Transport(TransportFailure),
    /// The service answered with an error status and, usually, its words.
    Http {
        status: u16,
        kind: Option<String>,
        message: String,
    },
    /// The service answered without an image.
    NoImage,
    Malformed(&'static str),
    /// The image was made, but where the answer put it could not be
    /// fetched.
    Fetch(String),
}

impl fmt::Display for ImagesError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(failure) => write!(formatter, "the image request failed: {failure:?}"),
            Self::Http {
                status,
                kind,
                message,
            } => write!(
                formatter,
                "the image service answered HTTP {status} ({}): {message}",
                kind.as_deref().unwrap_or("no error type")
            ),
            Self::NoImage => formatter.write_str("the image service answered without an image"),
            Self::Malformed(what) => {
                write!(formatter, "the image service's answer is malformed: {what}")
            }
            Self::Fetch(why) => write!(
                formatter,
                "the image was made but could not be fetched: {why}"
            ),
        }
    }
}

impl Error for ImagesError {}

impl ImagesError {
    /// Whether the request certainly made nothing and cost nothing: it never
    /// reached the service, or the service refused it (4xx).
    #[must_use]
    pub const fn certainly_not_accepted(&self) -> bool {
        match self {
            Self::Transport(failure) => matches!(
                failure,
                TransportFailure::Connect | TransportFailure::Insecure | TransportFailure::Tls
            ),
            Self::Http { status, .. } => *status >= 400 && *status < 500,
            Self::NoImage | Self::Malformed(_) | Self::Fetch(_) => false,
        }
    }

    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(TransportFailure::Insecure) => ErrorClass::Terminal,
            Self::Transport(_) | Self::Fetch(_) => ErrorClass::Retryable,
            Self::Http {
                status,
                kind,
                message,
            } => {
                let words =
                    format!("{} {message}", kind.as_deref().unwrap_or_default()).to_lowercase();
                let spent = ["quota", "balance", "billing", "credit"]
                    .iter()
                    .any(|word| words.contains(word));
                if (*status == 429 && !spent) || *status >= 500 {
                    ErrorClass::Retryable
                } else {
                    ErrorClass::Terminal
                }
            }
            Self::NoImage | Self::Malformed(_) => ErrorClass::Terminal,
        }
    }
}

#[derive(Deserialize)]
struct WireAnswer {
    #[serde(default)]
    data: Vec<WireImage>,
}

#[derive(Deserialize)]
struct WireImage {
    #[serde(default)]
    b64_json: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

#[derive(Deserialize)]
struct WireErrorBody {
    error: WireError,
}

/// One error, or a list holding one. The list is tried first: serde reads
/// a struct from a list too, and would find no error in it.
#[derive(Deserialize)]
#[serde(untagged)]
enum WireErrorAnswer {
    List(Vec<WireErrorBody>),
    One(WireErrorBody),
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    message: String,
    #[serde(default, rename = "type")]
    kind: Option<String>,
    #[serde(default)]
    code: Option<Value>,
}

/// The size sent for a frame of `width:height`: the square, landscape or
/// portrait one OpenAI's models take, nearest in shape.
#[must_use]
pub fn size_for(width: u16, height: u16) -> &'static str {
    let (width, height) = (u32::from(width), u32::from(height));
    if width * 10 > height * 11 {
        "1536x1024"
    } else if height * 10 > width * 11 {
        "1024x1536"
    } else {
        "1024x1024"
    }
}

/// A client of one service that speaks the protocol.
pub struct ImagesClient<T = SharedTransport> {
    transport: T,
    base_url: String,
    model: String,
    key: Option<SecretString>,
    /// Fields the director added to every request.
    extra: Map<String, Value>,
}

impl ImagesClient {
    /// The service at `base_url` (for example `https://api.openai.com/v1`)
    /// making `model`, with its key if it takes one, and the fields the
    /// director added.
    #[must_use]
    pub fn new(
        base_url: &str,
        model: &str,
        key: Option<SecretString>,
        extra: Map<String, Value>,
    ) -> Self {
        Self::with_transport(Arc::new(HttpTransport::new()), base_url, model, key, extra)
    }
}

impl<T> fmt::Debug for ImagesClient<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ImagesClient")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl<T: Transport> ImagesClient<T> {
    pub fn with_transport(
        transport: T,
        base_url: &str,
        model: &str,
        key: Option<SecretString>,
        extra: Map<String, Value>,
    ) -> Self {
        Self {
            transport,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            key,
            extra,
        }
    }

    /// One image of `prompt`, framed `width:height` as nearly as the
    /// service's sizes allow, following `references` when there are any.
    pub fn generate(
        &self,
        prompt: &str,
        frame: (u16, u16),
        references: &[&[u8]],
    ) -> Result<Vec<u8>, ImagesError> {
        let size = (!self.extra.contains_key("size")).then(|| size_for(frame.0, frame.1));
        match self.ask(prompt, size, references) {
            // A refusal costs nothing; ask once more without the size it
            // did not take.
            Err(ImagesError::Http {
                status: 400,
                kind,
                message,
            }) if size.is_some()
                && format!("{} {message}", kind.as_deref().unwrap_or_default())
                    .to_lowercase()
                    .contains("size") =>
            {
                self.ask(prompt, None, references)
            }
            answered => answered,
        }
    }

    fn ask(
        &self,
        prompt: &str,
        size: Option<&str>,
        references: &[&[u8]],
    ) -> Result<Vec<u8>, ImagesError> {
        let mut fields = Map::new();
        fields.insert("model".to_owned(), json!(self.model));
        fields.insert("prompt".to_owned(), json!(prompt));
        fields.insert("n".to_owned(), json!(1));
        if let Some(size) = size {
            fields.insert("size".to_owned(), json!(size));
        }
        fields.extend(self.extra.clone());
        let response = if references.is_empty() {
            let body = serde_json::to_vec(&Value::Object(fields))
                .map_err(|_| ImagesError::Malformed("the request did not encode"))?;
            self.transport.post(
                &format!("{}/images/generations", self.base_url),
                self.key.as_ref(),
                "application/json",
                &body,
            )
        } else {
            let (content_type, body) = multipart(&fields, references);
            self.transport.post(
                &format!("{}/images/edits", self.base_url),
                self.key.as_ref(),
                &content_type,
                &body,
            )
        }
        .map_err(ImagesError::Transport)?;
        if !(200..300).contains(&response.status) {
            let error = match serde_json::from_slice::<WireErrorAnswer>(&response.body) {
                Ok(WireErrorAnswer::One(body)) => Some(body.error),
                Ok(WireErrorAnswer::List(bodies)) => {
                    bodies.into_iter().next().map(|body| body.error)
                }
                Err(_) => None,
            };
            return Err(ImagesError::Http {
                status: response.status,
                kind: error.as_ref().and_then(|error| {
                    error.kind.clone().or_else(|| match &error.code {
                        Some(Value::String(code)) => Some(code.clone()),
                        _ => None,
                    })
                }),
                message: error.map_or_else(String::new, |error| clipped(&error.message)),
            });
        }
        let answer: WireAnswer = serde_json::from_slice(&response.body)
            .map_err(|_| ImagesError::Malformed("the body is not an images answer"))?;
        let image = answer.data.into_iter().next().ok_or(ImagesError::NoImage)?;
        let bytes = match (image.b64_json, image.url) {
            (Some(inline), _) => STANDARD
                .decode(inline.trim())
                .map_err(|_| ImagesError::Malformed("the image is not base64"))?,
            (None, Some(url)) => self.fetch(&url)?,
            (None, None) => return Err(ImagesError::NoImage),
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > MAX_IMAGE_BYTES {
            return Err(ImagesError::Malformed("the image is too large"));
        }
        ImageFormat::sniff(&bytes).ok_or(ImagesError::Malformed("the answer is not an image"))?;
        Ok(bytes)
    }

    /// The image at `url`, asked for again when fetching it fails: it was
    /// made and paid for, and the address may not last.
    fn fetch(&self, url: &str) -> Result<Vec<u8>, ImagesError> {
        let mut why = String::new();
        for _ in 0..FETCH_ATTEMPTS {
            match self.transport.fetch(url) {
                Ok(response) if (200..300).contains(&response.status) => return Ok(response.body),
                Ok(response) => why = format!("HTTP {}", response.status),
                Err(failure) => why = format!("{failure:?}"),
            }
        }
        Err(ImagesError::Fetch(why))
    }
}

/// `message` without control characters, cut to a length an error carries.
fn clipped(message: &str) -> String {
    message
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect()
}

/// `fields` and `references` as a multipart form: each field as text, each
/// reference as an `image[]` file. Returns the content type, with its
/// boundary, and the body.
fn multipart(fields: &Map<String, Value>, references: &[&[u8]]) -> (String, Vec<u8>) {
    let text = |value: &Value| match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    // A boundary that none of the parts contains.
    let boundary = (0_u32..)
        .map(|salt| format!("kineto-form-{salt:08x}-7f3a9c2e4b1d6a50"))
        .find(|boundary| {
            let mark = boundary.as_bytes();
            !references.iter().any(|bytes| contains(bytes, mark))
                && !fields
                    .values()
                    .any(|value| text(value).contains(boundary.as_str()))
        })
        .expect("some boundary is free");
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{}\r\n",
                text(value)
            )
            .as_bytes(),
        );
    }
    for (index, bytes) in references.iter().enumerate() {
        let format = ImageFormat::sniff(bytes).unwrap_or(ImageFormat::Png);
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"image[]\"; \
                 filename=\"reference-{index}.{}\"\r\nContent-Type: {}\r\n\r\n",
                format.extension(),
                format.mime()
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    const KEY: &str = "sk-images-test-0123456789";
    const PNG: &[u8] = b"\x89PNG\r\n\x1a\nimage";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0jpeg";

    /// A request as sent: URL, key, content type and body.
    type Sent = (String, Option<String>, String, Vec<u8>);

    struct Recorded {
        answers: Mutex<VecDeque<Result<HttpResponse, TransportFailure>>>,
        files: Mutex<VecDeque<Result<HttpResponse, TransportFailure>>>,
        sent: Mutex<Vec<Sent>>,
        fetched: Mutex<Vec<String>>,
    }

    impl Transport for Recorded {
        fn post(
            &self,
            url: &str,
            key: Option<&SecretString>,
            content_type: &str,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            self.sent.lock().unwrap().push((
                url.to_owned(),
                key.map(|key| key.expose().to_owned()),
                content_type.to_owned(),
                body.to_vec(),
            ));
            self.answers.lock().unwrap().pop_front().expect("an answer")
        }

        fn fetch(&self, url: &str) -> Result<HttpResponse, TransportFailure> {
            self.fetched.lock().unwrap().push(url.to_owned());
            self.files.lock().unwrap().pop_front().expect("a file")
        }
    }

    fn client(
        answers: Vec<Result<HttpResponse, TransportFailure>>,
        files: Vec<Result<HttpResponse, TransportFailure>>,
        extra: Map<String, Value>,
    ) -> ImagesClient<Recorded> {
        ImagesClient::with_transport(
            Recorded {
                answers: Mutex::new(answers.into()),
                files: Mutex::new(files.into()),
                sent: Mutex::new(Vec::new()),
                fetched: Mutex::new(Vec::new()),
            },
            "https://images.example.test/v1/",
            "image-model",
            SecretString::new(KEY.to_owned()),
            extra,
        )
    }

    fn json(status: u16, body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    fn inline(bytes: &[u8]) -> Result<HttpResponse, TransportFailure> {
        json(
            200,
            &format!(
                r#"{{"created":1,"data":[{{"b64_json":"{}"}}]}}"#,
                STANDARD.encode(bytes)
            ),
        )
    }

    #[test]
    fn a_frame_asks_for_the_size_nearest_in_shape() {
        assert_eq!(size_for(16, 9), "1536x1024");
        assert_eq!(size_for(21, 9), "1536x1024");
        assert_eq!(size_for(9, 16), "1024x1536");
        assert_eq!(size_for(1, 1), "1024x1024");
        assert_eq!(size_for(5, 4), "1536x1024");
        assert_eq!(size_for(21, 20), "1024x1024");
    }

    #[test]
    fn an_image_is_asked_for_and_read_inline() {
        let mut extra = Map::new();
        extra.insert("quality".to_owned(), json!("high"));
        let client = client(vec![inline(PNG)], vec![], extra);
        assert_eq!(client.generate("a red door", (16, 9), &[]).unwrap(), PNG);

        let sent = client.transport.sent.lock().unwrap();
        let (url, key, content_type, body) = &sent[0];
        assert_eq!(url, "https://images.example.test/v1/images/generations");
        assert_eq!(key.as_deref(), Some(KEY));
        assert_eq!(content_type, "application/json");
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            body,
            json!({"model": "image-model", "prompt": "a red door", "n": 1, "size": "1536x1024", "quality": "high"})
        );
        assert!(!String::from_utf8_lossy(&sent[0].3).contains(KEY));
    }

    #[test]
    fn an_image_at_an_address_is_fetched_without_the_key_and_again_when_that_fails() {
        let fetched = client(
            vec![json(
                200,
                r#"{"data":[{"url":"https://cdn.example.test/a.jpeg"}]}"#,
            )],
            vec![
                Err(TransportFailure::Timeout),
                json(403, "expired"),
                Ok(HttpResponse {
                    status: 200,
                    body: JPEG.to_vec(),
                }),
            ],
            Map::new(),
        );
        assert_eq!(fetched.generate("a red door", (1, 1), &[]).unwrap(), JPEG);
        assert_eq!(fetched.transport.fetched.lock().unwrap().len(), 3);

        let gone = client(
            vec![json(
                200,
                r#"{"data":[{"url":"https://cdn.example.test/a.jpeg"}]}"#,
            )],
            vec![json(404, ""), json(404, ""), json(404, "")],
            Map::new(),
        );
        let error = gone.generate("a red door", (1, 1), &[]).unwrap_err();
        assert_eq!(error, ImagesError::Fetch("HTTP 404".to_owned()));
        assert!(!error.certainly_not_accepted(), "it was made, and paid for");
    }

    #[test]
    fn references_go_as_files_of_an_edit() {
        let client = client(vec![inline(PNG)], vec![], Map::new());
        client
            .generate("the same woman, older", (9, 16), &[JPEG, PNG])
            .unwrap();
        let sent = client.transport.sent.lock().unwrap();
        let (url, _, content_type, body) = &sent[0];
        assert_eq!(url, "https://images.example.test/v1/images/edits");
        let boundary = content_type
            .strip_prefix("multipart/form-data; boundary=")
            .unwrap();
        let body = String::from_utf8_lossy(body);
        assert!(body.starts_with(&format!("--{boundary}\r\n")));
        assert!(body.ends_with(&format!("--{boundary}--\r\n")));
        for part in [
            "name=\"model\"\r\n\r\nimage-model\r\n",
            "name=\"prompt\"\r\n\r\nthe same woman, older\r\n",
            "name=\"n\"\r\n\r\n1\r\n",
            "name=\"size\"\r\n\r\n1024x1536\r\n",
            "name=\"image[]\"; filename=\"reference-0.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n",
            "name=\"image[]\"; filename=\"reference-1.png\"\r\nContent-Type: image/png\r\n\r\n",
        ] {
            assert!(body.contains(part), "{part:?} in {body}");
        }
    }

    #[test]
    fn a_size_the_service_refuses_is_left_out_once() {
        let resized = client(
            vec![
                json(
                    400,
                    r#"{"error":{"message":"Invalid size 1536x1024; supported: 1024x1024, 1344x768","type":"invalid_request_error"}}"#,
                ),
                inline(PNG),
            ],
            vec![],
            Map::new(),
        );
        assert_eq!(resized.generate("a red door", (16, 9), &[]).unwrap(), PNG);
        let sent = resized.transport.sent.lock().unwrap();
        assert_eq!(sent.len(), 2);
        let second: Value = serde_json::from_slice(&sent[1].3).unwrap();
        assert!(second.get("size").is_none());

        // Any other refusal is the answer.
        let refused = client_refusing(
            r#"{"error":{"message":"Your request was rejected by the safety system","type":"image_generation_user_error","code":"content_policy_violation"}}"#,
        );
        let error = refused.generate("a red door", (16, 9), &[]).unwrap_err();
        assert!(error.certainly_not_accepted());
        assert_eq!(error.class(), ErrorClass::Terminal);
        assert!(error.to_string().contains("safety system"), "{error}");
        assert_eq!(refused.transport.sent.lock().unwrap().len(), 1);

        // A size the director chose is never left out.
        let mut extra = Map::new();
        extra.insert("size".to_owned(), json!("2K"));
        let chosen = client(
            vec![json(400, r#"{"error":{"message":"bad size"}}"#)],
            vec![],
            extra,
        );
        assert!(chosen.generate("a red door", (16, 9), &[]).is_err());
        assert_eq!(chosen.transport.sent.lock().unwrap().len(), 1);
    }

    fn client_refusing(body: &str) -> ImagesClient<Recorded> {
        client(vec![json(400, body)], vec![], Map::new())
    }

    #[test]
    fn failures_say_whether_anything_was_made() {
        let cases = [
            (Err(TransportFailure::Connect), true, ErrorClass::Retryable),
            (Err(TransportFailure::Insecure), true, ErrorClass::Terminal),
            (Err(TransportFailure::Timeout), false, ErrorClass::Retryable),
            (
                json(401, r#"{"error":{"message":"bad key"}}"#),
                true,
                ErrorClass::Terminal,
            ),
            (
                json(429, r#"{"error":{"message":"slow down"}}"#),
                true,
                ErrorClass::Retryable,
            ),
            (
                json(
                    429,
                    r#"{"error":{"message":"You exceeded your current quota","type":"insufficient_quota"}}"#,
                ),
                true,
                ErrorClass::Terminal,
            ),
            (json(500, "<html>"), false, ErrorClass::Retryable),
            (json(200, r#"{"data":[]}"#), false, ErrorClass::Terminal),
            (
                json(200, r#"{"data":[{"b64_json":"%%%"}]}"#),
                false,
                ErrorClass::Terminal,
            ),
            (inline(b"not an image"), false, ErrorClass::Terminal),
            (json(200, "not json"), false, ErrorClass::Terminal),
        ];
        for (answer, refused, class) in cases {
            let client = client(vec![answer], vec![], Map::new());
            let error = client.generate("a red door", (16, 9), &[]).unwrap_err();
            assert_eq!(error.certainly_not_accepted(), refused, "{error}");
            assert_eq!(error.class(), class, "{error}");
            assert!(!format!("{error} {error:?} {client:?}").contains(KEY));
        }
    }

    #[test]
    fn a_plain_address_elsewhere_is_never_asked() {
        let transport = HttpTransport::new();
        assert_eq!(
            transport.post(
                "http://images.example.test/v1",
                None,
                "application/json",
                b"{}"
            ),
            Err(TransportFailure::Insecure)
        );
        assert_eq!(
            transport.fetch("http://cdn.example.test/a.png"),
            Err(TransportFailure::Insecure)
        );
    }
}
