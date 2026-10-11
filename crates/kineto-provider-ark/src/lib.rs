//! The video tasks of Volcengine's Ark and BytePlus's ModelArk, which serve
//! ByteDance's Seedance, as the gateways that resell it speak them too:
//! `POST <url>/contents/generations/tasks` starts a clip from a prompt and a
//! first frame, and a last frame to close on; `GET
//! <url>/contents/generations/tasks/<id>` follows it; the finished clip is
//! at `content.video_url` for 24 hours, fetched without the key.
//!
//! Only the protocol's common part is sent: the model, the prompt, the
//! frames as data URLs, and the clip's length, frame shape and height.
//! Fields the director adds in the settings file are sent too, and win.
//!
//! HTTPS everywhere, or plain HTTP to this computer; no redirects on the
//! API, so a key never follows one.

use std::{error::Error, fmt, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_jobs::ErrorClass;
use kineto_providers::{AspectRatio, ImageFormat, net::is_permitted, secret::SecretString};
use serde::Deserialize;
use serde_json::{Map, Value, json};

/// Largest clip taken.
pub const MAX_VIDEO_BYTES: u64 = 512 * 1024 * 1024;
/// Largest answer the API gives.
const MAX_ANSWER_BYTES: u64 = 8 * 1024 * 1024;
/// The frame shapes the protocol renders; any other is left to follow the
/// first frame (`adaptive`).
pub const ASPECT_RATIOS: [(u16, u16); 6] = [(16, 9), (9, 16), (4, 3), (3, 4), (1, 1), (21, 9)];
/// The clip lengths Seedance 2 renders, in seconds.
pub const CLIP_SECONDS: std::ops::RangeInclusive<u8> = 4..=15;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const FETCH_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_MESSAGE_CHARS: usize = 200;
const MAX_TASK_CHARS: usize = 128;

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
    /// POST the JSON `body` to `url` with the key as a bearer.
    fn post(
        &self,
        url: &str,
        key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure>;

    /// GET `url` with the key as a bearer.
    fn get(&self, url: &str, key: &SecretString) -> Result<HttpResponse, TransportFailure>;

    /// GET a clip an answer pointed at, never with the key.
    fn fetch(&self, url: &str) -> Result<HttpResponse, TransportFailure>;
}

/// A transport shared between clients and threads.
pub type SharedTransport = Arc<dyn Transport + Send + Sync>;

impl Transport for SharedTransport {
    fn post(
        &self,
        url: &str,
        key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().post(url, key, body)
    }

    fn get(&self, url: &str, key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().get(url, key)
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
            // A clip is fetched without the key, wherever it was moved.
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
        key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        if !is_permitted(url) {
            return Err(TransportFailure::Insecure);
        }
        let response = self
            .api
            .post(url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", key.expose()))
            .send(body)
            .map_err(failure)?;
        read(response, MAX_ANSWER_BYTES)
    }

    fn get(&self, url: &str, key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        if !is_permitted(url) {
            return Err(TransportFailure::Insecure);
        }
        let response = self
            .api
            .get(url)
            .header("Authorization", format!("Bearer {}", key.expose()))
            .call()
            .map_err(failure)?;
        read(response, MAX_ANSWER_BYTES)
    }

    fn fetch(&self, url: &str) -> Result<HttpResponse, TransportFailure> {
        if !is_permitted(url) {
            return Err(TransportFailure::Insecure);
        }
        read(
            self.files.get(url).call().map_err(failure)?,
            MAX_VIDEO_BYTES,
        )
    }
}

fn read(
    mut response: ureq::http::Response<ureq::Body>,
    limit: u64,
) -> Result<HttpResponse, TransportFailure> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(limit)
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
pub enum ArkError {
    /// The request did not complete; it may or may not have reached the
    /// service.
    Transport(TransportFailure),
    /// The service answered with an error status and, usually, its words.
    Http {
        status: u16,
        code: Option<String>,
        message: String,
    },
    /// The answer could not be read as the response it should be.
    Malformed(&'static str),
    /// The request was refused before it was sent.
    InvalidRequest(&'static str),
    /// The task ended without a clip.
    Failed { code: String, message: String },
    /// The task was cancelled, or ran past its time, without a clip.
    Ended(&'static str),
    /// The clip's file answered with an HTTP error status.
    Download { status: u16 },
}

impl ArkError {
    /// Whether asking the same again can succeed. A finished task answers
    /// the same way however often it is asked, so its failure is final.
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(TransportFailure::Insecure) => ErrorClass::Terminal,
            Self::Transport(_) => ErrorClass::Retryable,
            Self::Http {
                status: 429, code, ..
            } => {
                let account = code.as_deref().is_some_and(|code| {
                    ["Quota", "Overdue", "Balance", "Arrearage"]
                        .iter()
                        .any(|word| code.contains(word))
                });
                if account {
                    ErrorClass::Terminal
                } else {
                    ErrorClass::Retryable
                }
            }
            Self::Http { status, .. } if *status >= 500 => ErrorClass::Retryable,
            // The clip is kept for 24 hours; after that it is gone.
            Self::Download { status } => match status {
                429 | 500.. => ErrorClass::Retryable,
                _ => ErrorClass::Terminal,
            },
            Self::Http { .. }
            | Self::Malformed(_)
            | Self::InvalidRequest(_)
            | Self::Failed { .. }
            | Self::Ended(_) => ErrorClass::Terminal,
        }
    }

    /// How a task that ended with this is read once it is final:
    /// retryable where asking for the clip again may well work. A task
    /// that was too busy, ran out of time, or whose clip a filter withheld
    /// often succeeds the next time; one refused for what it was given,
    /// such as a real person's face, never does.
    #[must_use]
    pub fn ended_class(&self) -> ErrorClass {
        match self {
            Self::Failed { code, .. }
                if code.starts_with("Input") || code.starts_with("InvalidParameter") =>
            {
                ErrorClass::Terminal
            }
            Self::Failed { .. } | Self::Ended(_) => ErrorClass::Retryable,
            other => other.class(),
        }
    }

    /// Whether the service certainly did not accept a submission: it was
    /// never sent, no connection was made, or the service refused it. Any
    /// other failure may have started a task, and been charged for.
    #[must_use]
    pub fn certainly_not_accepted(&self) -> bool {
        match self {
            Self::InvalidRequest(_)
            | Self::Transport(TransportFailure::Connect | TransportFailure::Insecure) => true,
            Self::Http { status, .. } => (400..500).contains(status),
            Self::Transport(_)
            | Self::Malformed(_)
            | Self::Failed { .. }
            | Self::Ended(_)
            | Self::Download { .. } => false,
        }
    }
}

impl fmt::Display for ArkError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(failure) => {
                write!(formatter, "the video request failed: {failure:?}")
            }
            Self::Http {
                status,
                code,
                message,
            } => {
                write!(formatter, "the video service answered HTTP {status}")?;
                if let Some(code) = code {
                    write!(formatter, " ({code})")?;
                }
                if !message.is_empty() {
                    write!(formatter, ": {message}")?;
                }
                Ok(())
            }
            Self::Malformed(what) => {
                write!(formatter, "the video service's answer is malformed: {what}")
            }
            Self::InvalidRequest(what) => write!(formatter, "the video request is invalid: {what}"),
            Self::Failed { code, message } if message.is_empty() => {
                write!(formatter, "the video task failed ({code})")
            }
            Self::Failed { code, message } => {
                write!(formatter, "the video task failed ({code}): {message}")
            }
            Self::Ended(how) => write!(formatter, "the video task was {how} before its clip"),
            Self::Download { status } => {
                write!(formatter, "the clip's file answered HTTP {status}")
            }
        }
    }
}

impl Error for ArkError {}

/// One clip asked for.
#[derive(Clone, Copy)]
pub struct ClipRequest<'a> {
    pub prompt: &'a str,
    pub first_frame: (&'a [u8], ImageFormat),
    /// The frame the clip closes on.
    pub last_frame: Option<(&'a [u8], ImageFormat)>,
    pub seconds: u8,
    /// The clip's frame shape; one the protocol does not name follows the
    /// first frame.
    pub aspect_ratio: AspectRatio,
    /// The clip's height in lines.
    pub lines: u16,
}

/// Where a task is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskState {
    /// Queued or being made.
    Running,
    /// Finished; its clip is at `video_url`.
    Succeeded { video_url: String },
}

/// A video connection's client: one service, one model, one key.
pub struct ArkVideos<T = SharedTransport> {
    transport: T,
    base_url: String,
    model: String,
    key: SecretString,
    extra: Map<String, Value>,
}

impl ArkVideos {
    /// The service at `base_url` (`https://ark.ap-southeast.bytepluses.com/api/v3`)
    /// making clips with `model`, with `key`, sending `extra` with every
    /// request.
    #[must_use]
    pub fn new(base_url: &str, model: &str, key: SecretString, extra: Map<String, Value>) -> Self {
        Self::with_transport(Arc::new(HttpTransport::new()), base_url, model, key, extra)
    }
}

impl<T> fmt::Debug for ArkVideos<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ArkVideos")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct WireCreated {
    id: String,
}

#[derive(Deserialize)]
struct WireTask {
    status: String,
    #[serde(default)]
    content: Option<WireContent>,
    #[serde(default)]
    error: Option<WireError>,
}

#[derive(Deserialize)]
struct WireContent {
    #[serde(default)]
    video_url: Option<String>,
}

#[derive(Deserialize)]
struct WireError {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct WireErrorBody {
    error: WireError,
}

impl<T: Transport> ArkVideos<T> {
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
            model: model.to_owned(),
            key,
            extra,
        }
    }

    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    fn tasks(&self) -> String {
        format!("{}/contents/generations/tasks", self.base_url)
    }

    /// What `request` sends: the protocol's fields, then the director's.
    #[must_use]
    pub fn body(&self, request: &ClipRequest<'_>) -> Value {
        let image = |(bytes, format): (&[u8], ImageFormat), role: &str| {
            json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{};base64,{}", format.mime(), STANDARD.encode(bytes))},
                "role": role,
            })
        };
        let mut content = vec![
            json!({"type": "text", "text": request.prompt}),
            image(request.first_frame, "first_frame"),
        ];
        if let Some(last) = request.last_frame {
            content.push(image(last, "last_frame"));
        }
        let AspectRatio { width, height } = request.aspect_ratio;
        let ratio = if ASPECT_RATIOS.contains(&(width, height)) {
            format!("{width}:{height}")
        } else {
            "adaptive".to_owned()
        };
        let mut body = Map::new();
        body.insert("model".to_owned(), Value::from(self.model.as_str()));
        body.insert("content".to_owned(), Value::Array(content));
        body.insert("ratio".to_owned(), Value::from(ratio));
        body.insert("duration".to_owned(), Value::from(request.seconds));
        body.insert(
            "resolution".to_owned(),
            Value::from(format!("{}p", request.lines)),
        );
        for (field, value) in &self.extra {
            body.insert(field.clone(), value.clone());
        }
        Value::Object(body)
    }

    /// Start `request`: the paid call. Returns the task to follow.
    pub fn submit(&self, request: &ClipRequest<'_>) -> Result<String, ArkError> {
        if !CLIP_SECONDS.contains(&request.seconds) {
            return Err(ArkError::InvalidRequest("a clip is 4 to 15 seconds long"));
        }
        let body = serde_json::to_vec(&self.body(request))
            .map_err(|_| ArkError::Malformed("the request did not encode"))?;
        let response = self
            .transport
            .post(&self.tasks(), &self.key, &body)
            .map_err(ArkError::Transport)?;
        refused(&response)?;
        let created: WireCreated = serde_json::from_slice(&response.body)
            .map_err(|_| ArkError::Malformed("no task in the answer"))?;
        if !task_id(&created.id) {
            return Err(ArkError::Malformed("the task has an unusable ID"));
        }
        Ok(created.id)
    }

    /// Where the task `id` is: free.
    pub fn task(&self, id: &str) -> Result<TaskState, ArkError> {
        if !task_id(id) {
            return Err(ArkError::InvalidRequest("the task has an unusable ID"));
        }
        let response = self
            .transport
            .get(&format!("{}/{id}", self.tasks()), &self.key)
            .map_err(ArkError::Transport)?;
        refused(&response)?;
        let task: WireTask = serde_json::from_slice(&response.body)
            .map_err(|_| ArkError::Malformed("the task's answer is unreadable"))?;
        match task.status.as_str() {
            "queued" | "running" => Ok(TaskState::Running),
            "succeeded" => task
                .content
                .and_then(|content| content.video_url)
                .filter(|url| !url.is_empty())
                .map(|video_url| TaskState::Succeeded { video_url })
                .ok_or(ArkError::Malformed("a finished task gave no clip")),
            "failed" => {
                let error = task.error.unwrap_or(WireError {
                    code: None,
                    message: String::new(),
                });
                Err(ArkError::Failed {
                    code: error.code.unwrap_or_else(|| "unknown".to_owned()),
                    message: clean(&error.message),
                })
            }
            "cancelled" => Err(ArkError::Ended("cancelled")),
            "expired" => Err(ArkError::Ended("timed out")),
            _ => Err(ArkError::Malformed(
                "the task is in a state this build does not know",
            )),
        }
    }

    /// The finished clip at `url`, fetched without the key: free.
    pub fn download(&self, url: &str) -> Result<Vec<u8>, ArkError> {
        let response = self.transport.fetch(url).map_err(ArkError::Transport)?;
        if !(200..300).contains(&response.status) {
            return Err(ArkError::Download {
                status: response.status,
            });
        }
        // An MP4 or a QuickTime file names its brand in its first box.
        if response.body.get(4..8) != Some(b"ftyp") {
            return Err(ArkError::Malformed("the clip's file is not a video"));
        }
        Ok(response.body)
    }
}

/// The service's refusal, when `response` is one.
fn refused(response: &HttpResponse) -> Result<(), ArkError> {
    if (200..300).contains(&response.status) {
        return Ok(());
    }
    let error = serde_json::from_slice::<WireErrorBody>(&response.body)
        .ok()
        .map(|body| body.error);
    Err(ArkError::Http {
        status: response.status,
        code: error.as_ref().and_then(|error| error.code.clone()),
        message: error.map_or_else(String::new, |error| clean(&error.message)),
    })
}

/// Whether `id` can name a task in an address.
fn task_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_TASK_CHARS
        && !id.contains("..")
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn clean(message: &str) -> String {
    message
        .chars()
        .filter(|character| !character.is_control())
        .take(MAX_MESSAGE_CHARS)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const KEY: &str = "ark-test-key-0123456789";

    /// Method, URL, key and body of one request.
    type Sent = (&'static str, String, Option<String>, Vec<u8>);

    struct Recorded {
        /// Answers in turn; the last is given again to any further request.
        answers: Mutex<Vec<Result<HttpResponse, TransportFailure>>>,
        sent: Mutex<Vec<Sent>>,
    }

    impl Recorded {
        fn answer(&self, sent: Sent) -> Result<HttpResponse, TransportFailure> {
            self.sent.lock().unwrap().push(sent);
            let mut answers = self.answers.lock().unwrap();
            if answers.len() > 1 {
                answers.remove(0)
            } else {
                answers[0].clone()
            }
        }
    }

    impl Transport for Recorded {
        fn post(
            &self,
            url: &str,
            key: &SecretString,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            self.answer((
                "POST",
                url.to_owned(),
                Some(key.expose().to_owned()),
                body.to_vec(),
            ))
        }

        fn get(&self, url: &str, key: &SecretString) -> Result<HttpResponse, TransportFailure> {
            self.answer((
                "GET",
                url.to_owned(),
                Some(key.expose().to_owned()),
                Vec::new(),
            ))
        }

        fn fetch(&self, url: &str) -> Result<HttpResponse, TransportFailure> {
            self.answer(("FETCH", url.to_owned(), None, Vec::new()))
        }
    }

    fn client(
        answers: Vec<Result<HttpResponse, TransportFailure>>,
        extra: Map<String, Value>,
    ) -> ArkVideos<Recorded> {
        ArkVideos::with_transport(
            Recorded {
                answers: Mutex::new(answers),
                sent: Mutex::new(Vec::new()),
            },
            "https://ark.example.test/api/v3/",
            "dreamina-seedance-2-0-260128",
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

    fn request<'a>(first: &'a [u8], last: Option<&'a [u8]>) -> ClipRequest<'a> {
        ClipRequest {
            prompt: "The door opens.",
            first_frame: (first, ImageFormat::Png),
            last_frame: last.map(|bytes| (bytes, ImageFormat::Jpeg)),
            seconds: 5,
            aspect_ratio: AspectRatio::new(16, 9).unwrap(),
            lines: 720,
        }
    }

    #[test]
    fn a_clip_is_asked_for_with_its_frames_length_shape_and_height() {
        let mut extra = Map::new();
        extra.insert("generate_audio".to_owned(), Value::Bool(false));
        extra.insert("resolution".to_owned(), Value::from("1080p"));
        let client = client(vec![json(200, r#"{"id":"cgt-20261010-abc"}"#)], extra);
        let task = client.submit(&request(b"first", Some(b"last"))).unwrap();
        assert_eq!(task, "cgt-20261010-abc");

        let sent = client.transport.sent.lock().unwrap();
        let (method, url, key, body) = &sent[0];
        assert_eq!(*method, "POST");
        assert_eq!(
            url,
            "https://ark.example.test/api/v3/contents/generations/tasks"
        );
        assert_eq!(key.as_deref(), Some(KEY));
        assert!(!String::from_utf8_lossy(body).contains(KEY));
        let body: Value = serde_json::from_slice(body).unwrap();
        assert_eq!(body["model"], "dreamina-seedance-2-0-260128");
        assert_eq!(body["ratio"], "16:9");
        assert_eq!(body["duration"], 5);
        // The director's fields win.
        assert_eq!(body["resolution"], "1080p");
        assert_eq!(body["generate_audio"], false);
        let content = body["content"].as_array().unwrap();
        assert_eq!(
            content[0],
            json!({"type": "text", "text": "The door opens."})
        );
        assert_eq!(content[1]["role"], "first_frame");
        assert_eq!(
            content[1]["image_url"]["url"],
            format!("data:image/png;base64,{}", STANDARD.encode(b"first"))
        );
        assert_eq!(content[2]["role"], "last_frame");
        assert!(
            content[2]["image_url"]["url"]
                .as_str()
                .unwrap()
                .starts_with("data:image/jpeg;base64,")
        );
    }

    #[test]
    fn a_shape_it_does_not_name_follows_the_first_frame() {
        let client = client(vec![json(200, r#"{"id":"t1"}"#)], Map::new());
        let mut wide = request(b"first", None);
        wide.aspect_ratio = AspectRatio::new(239, 100).unwrap();
        let body = client.body(&wide);
        assert_eq!(body["ratio"], "adaptive");
        assert_eq!(body["content"].as_array().unwrap().len(), 2);
        assert_eq!(body["resolution"], "720p");

        let mut long = request(b"first", None);
        long.seconds = 20;
        assert_eq!(
            client.submit(&long),
            Err(ArkError::InvalidRequest("a clip is 4 to 15 seconds long"))
        );
        assert!(client.transport.sent.lock().unwrap().is_empty());
    }

    #[test]
    fn a_task_is_followed_to_its_clip() {
        let mut video = b"\0\0\0\x18ftypmp42".to_vec();
        video.extend_from_slice(&[0; 32]);
        let client = client(
            vec![
                json(200, r#"{"id":"t1","status":"queued"}"#),
                json(200, r#"{"id":"t1","status":"running"}"#),
                json(
                    200,
                    r#"{"id":"t1","status":"succeeded","content":{"video_url":"https://files.example.test/t1.mp4?sig=x"},"usage":{"completion_tokens":108900}}"#,
                ),
                Ok(HttpResponse {
                    status: 200,
                    body: video.clone(),
                }),
            ],
            Map::new(),
        );
        assert_eq!(client.task("t1").unwrap(), TaskState::Running);
        assert_eq!(client.task("t1").unwrap(), TaskState::Running);
        let TaskState::Succeeded { video_url } = client.task("t1").unwrap() else {
            panic!("finished");
        };
        assert_eq!(client.download(&video_url).unwrap(), video);

        let sent = client.transport.sent.lock().unwrap();
        assert_eq!(
            sent[0].1,
            "https://ark.example.test/api/v3/contents/generations/tasks/t1"
        );
        assert_eq!(sent[0].0, "GET");
        let (method, url, key, _) = &sent[3];
        assert_eq!(
            (*method, url.as_str(), key),
            ("FETCH", "https://files.example.test/t1.mp4?sig=x", &None),
            "a clip is fetched without the key"
        );
    }

    #[test]
    fn a_task_that_ended_without_a_clip_says_why() {
        let failed = client(
            vec![json(
                200,
                r#"{"id":"t1","status":"failed","error":{"code":"InputImageSensitiveContentDetected.PrivacyInformation","message":"The request failed because the input image may contain a real person."}}"#,
            )],
            Map::new(),
        );
        let error = failed.task("t1").unwrap_err();
        assert_eq!(error.ended_class(), ErrorClass::Terminal, "{error}");
        assert!(error.to_string().contains("real person"));

        let busy = client(
            vec![json(
                200,
                r#"{"status":"failed","error":{"code":"ServerOverloaded","message":"busy"}}"#,
            )],
            Map::new(),
        );
        assert_eq!(
            busy.task("t1").unwrap_err().ended_class(),
            ErrorClass::Retryable
        );
        for (status, ended) in [("cancelled", "cancelled"), ("expired", "timed out")] {
            let task = client(
                vec![json(200, &format!(r#"{{"status":"{status}"}}"#))],
                Map::new(),
            );
            assert_eq!(task.task("t1"), Err(ArkError::Ended(ended)));
        }
        let unknown = client(vec![json(200, r#"{"status":"paused"}"#)], Map::new());
        assert!(matches!(unknown.task("t1"), Err(ArkError::Malformed(_))));
        let empty = client(vec![json(200, r#"{"status":"succeeded"}"#)], Map::new());
        assert!(matches!(empty.task("t1"), Err(ArkError::Malformed(_))));
        assert!(matches!(
            failed.task("../keys"),
            Err(ArkError::InvalidRequest(_))
        ));
    }

    #[test]
    fn only_a_refusal_or_no_connection_proves_nothing_was_started() {
        let refusal = client(
            vec![json(
                400,
                r#"{"error":{"code":"InvalidParameter","message":"duration is invalid","param":"duration","type":"BadRequest"}}"#,
            )],
            Map::new(),
        )
        .submit(&request(b"first", None))
        .unwrap_err();
        assert!(refusal.certainly_not_accepted());
        assert_eq!(refusal.class(), ErrorClass::Terminal);
        assert_eq!(
            refusal.to_string(),
            "the video service answered HTTP 400 (InvalidParameter): duration is invalid"
        );

        let cases = [
            (Err(TransportFailure::Connect), true, ErrorClass::Retryable),
            (Err(TransportFailure::Insecure), true, ErrorClass::Terminal),
            (Err(TransportFailure::Timeout), false, ErrorClass::Retryable),
            (json(500, "<html>"), false, ErrorClass::Retryable),
            (
                json(
                    429,
                    r#"{"error":{"code":"RateLimitExceeded.EndpointRPMExceeded","message":"slow"}}"#,
                ),
                true,
                ErrorClass::Retryable,
            ),
            (
                json(
                    429,
                    r#"{"error":{"code":"QuotaExceeded","message":"no more"}}"#,
                ),
                true,
                ErrorClass::Terminal,
            ),
            (json(200, r#"{"id":"../x"}"#), false, ErrorClass::Terminal),
            (json(200, "not json"), false, ErrorClass::Terminal),
        ];
        for (answer, not_accepted, class) in cases {
            let error = client(vec![answer], Map::new())
                .submit(&request(b"first", None))
                .unwrap_err();
            assert_eq!(error.certainly_not_accepted(), not_accepted, "{error}");
            assert_eq!(error.class(), class, "{error}");
            assert!(!format!("{error} {error:?}").contains(KEY));
        }
        assert!(!format!("{:?}", client(vec![json(200, "{}")], Map::new())).contains(KEY));
    }

    #[test]
    fn a_file_that_is_not_a_clip_is_not_kept() {
        let gone = client(vec![json(403, "expired")], Map::new());
        let error = gone
            .download("https://files.example.test/t1.mp4")
            .unwrap_err();
        assert_eq!(error, ArkError::Download { status: 403 });
        assert_eq!(error.class(), ErrorClass::Terminal);
        let page = client(vec![json(200, "<html>sign in</html>")], Map::new());
        assert!(matches!(
            page.download("https://files.example.test/t1.mp4"),
            Err(ArkError::Malformed(_))
        ));
    }
}
