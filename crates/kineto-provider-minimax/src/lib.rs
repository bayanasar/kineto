//! MiniMax image generation (`image-01`) over blocking HTTPS.
//!
//! This crate is the vendor edge: it knows MiniMax's endpoint, request and
//! response shapes and status codes, and nothing about projects, artifacts or
//! jobs. It returns decoded image bytes and classifies every failure as
//! retryable or terminal for the job runtime.
//!
//! The API key is held as a [`SecretString`] and exposed only to build the
//! `Authorization` header. No error value carries request headers, and
//! transport failures are reduced to a kind so that no library error text can
//! carry them either.
//!
//! MiniMax's image endpoint has no idempotency key and no way to look a past
//! request up. A call that fails after the request was sent may still have
//! been billed, so retrying it can pay twice; the job runtime's attempt limit
//! bounds that.

use std::{error::Error, fmt, sync::Arc, time::Duration};

use base64::Engine as _;
use kineto_jobs::{CostEstimate, CurrencyCode, ErrorClass};
use kineto_providers::{
    ImageCapabilities, ImageFormat, Price, ReferenceRole, secret::SecretString,
};
use serde::{Deserialize, Serialize};

mod video;

pub use video::{
    MAX_VIDEO_BYTES, VIDEO_MODEL, VideoDuration, VideoRequest, VideoResolution, VideoTask,
    VideoTaskState, camera_command, video_capabilities,
};

/// The provider key MiniMax jobs are recorded under.
pub const PROVIDER: &str = "minimax";
/// The image model this adapter calls.
pub const IMAGE_MODEL: &str = "image-01";
/// MiniMax's international API.
pub const DEFAULT_BASE_URL: &str = "https://api.minimax.io";
/// The longest prompt `image-01` accepts, in characters.
pub const MAX_PROMPT_CHARS: usize = 1500;
/// Published pay-as-you-go price of one `image-01` image, in USD micros.
pub const IMAGE_PRICE_MICROS: u64 = 3_500;

/// One base64 image of a few megapixels is a few MiB; anything far larger is
/// not an `image-01` answer.
const MAX_RESPONSE_BYTES: u64 = 32 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(180);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_STATUS_MESSAGE_CHARS: usize = 200;

/// What `image-01` can preserve, as MiniMax documents it. A subject reference
/// (`subject_reference`, type `character`) is the one reference kind: an
/// identity portrait, JPG or PNG, under 10 MB, best a single front-facing
/// face. The client sends none yet; the declaration is what prompt
/// compilation and routing read instead of the vendor's name.
#[must_use]
pub fn image_capabilities() -> ImageCapabilities {
    ImageCapabilities {
        aspect_ratios: [(1, 1), (16, 9), (9, 16)]
            .into_iter()
            .filter_map(|(width, height)| kineto_providers::AspectRatio::new(width, height))
            .collect(),
        max_reference_images: 1,
        reference_roles: [ReferenceRole::Identity].into_iter().collect(),
        reference_formats: [ImageFormat::Jpeg, ImageFormat::Png].into_iter().collect(),
        max_reference_bytes: Some(MAX_REFERENCE_BYTES as u64),
        output_formats: [ImageFormat::Jpeg].into_iter().collect(),
        max_prompt_chars: Some(MAX_PROMPT_CHARS),
        // The image comes back base64 in the answer to the request.
        async_jobs: false,
        price_per_image: Some(Price::usd(IMAGE_PRICE_MICROS)),
        ..ImageCapabilities::default()
    }
}

/// Aspect ratios `image-01` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectRatio {
    Square,
    Landscape16x9,
    Portrait9x16,
}

impl AspectRatio {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Square => "1:1",
            Self::Landscape16x9 => "16:9",
            Self::Portrait9x16 => "9:16",
        }
    }
}

/// A picture of the person an image must show, sent as `subject_reference`
/// of type `character`: a JPEG or PNG under 10 MB, best a single
/// front-facing portrait.
#[derive(Clone, PartialEq, Eq)]
pub struct SubjectReference(pub Vec<u8>);

impl fmt::Debug for SubjectReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "SubjectReference({} bytes)", self.0.len())
    }
}

/// Largest subject reference MiniMax accepts.
pub const MAX_REFERENCE_BYTES: usize = 10 * 1024 * 1024 - 1;

/// One image to generate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageRequest {
    pub prompt: String,
    pub aspect_ratio: AspectRatio,
    pub subject_reference: Option<SubjectReference>,
}

/// The image MiniMax returned.
#[derive(Clone, PartialEq, Eq)]
pub struct GeneratedImage {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
    /// MiniMax's ID for the request, kept for support enquiries.
    pub request_id: String,
}

impl fmt::Debug for GeneratedImage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("GeneratedImage")
            .field("bytes", &self.bytes.len())
            .field("format", &self.format)
            .field("request_id", &self.request_id)
            .finish()
    }
}

/// What a transport failed at. Library error text is deliberately not kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportFailure {
    Connect,
    Timeout,
    Tls,
    Io,
    TooLarge,
    Protocol,
}

/// An HTTP response as the client needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Sends one JSON POST. Production uses [`HttpsTransport`]; tests replay
/// recorded responses.
pub trait Transport {
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure>;

    /// An authenticated GET to the MiniMax API.
    fn get(&self, url: &str, api_key: &SecretString) -> Result<HttpResponse, TransportFailure>;

    /// An unauthenticated GET of a file the API pointed to, at most
    /// `max_bytes` long. It never carries the API key: the file lives on a
    /// content host, not on the API.
    fn download(&self, url: &str, max_bytes: u64) -> Result<HttpResponse, TransportFailure>;
}

/// A transport shared between clients and threads. The workflow holds this
/// so tests can stand a recorded MiniMax in for the network; dispatch through
/// it costs nothing next to an HTTPS round trip.
pub type SharedTransport = Arc<dyn Transport + Send + Sync>;

impl Transport for SharedTransport {
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().post_json(url, api_key, body)
    }

    fn get(&self, url: &str, api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().get(url, api_key)
    }

    fn download(&self, url: &str, max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().download(url, max_bytes)
    }
}

/// Blocking HTTPS with rustls and the bundled Mozilla roots. Cloning shares
/// the connection pools.
#[derive(Clone)]
pub struct HttpsTransport {
    agent: ureq::Agent,
    downloads: ureq::Agent,
}

impl HttpsTransport {
    #[must_use]
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .https_only(true)
            // A redirect would carry the key to wherever it points.
            .max_redirects(0)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(concat!("kineto/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        // Downloads carry no credentials, so a content host may redirect.
        let downloads = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(3)
            .http_status_as_error(false)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(DOWNLOAD_TIMEOUT))
            .user_agent(concat!("kineto/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();
        Self { agent, downloads }
    }
}

impl Default for HttpsTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for HttpsTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HttpsTransport")
    }
}

impl Transport for HttpsTransport {
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure> {
        let response = self
            .agent
            .post(url)
            .header("Authorization", format!("Bearer {}", api_key.expose()))
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(failure)?;
        read(response, MAX_RESPONSE_BYTES)
    }

    fn get(&self, url: &str, api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        let response = self
            .agent
            .get(url)
            .header("Authorization", format!("Bearer {}", api_key.expose()))
            .call()
            .map_err(failure)?;
        read(response, MAX_RESPONSE_BYTES)
    }

    fn download(&self, url: &str, max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
        let response = self.downloads.get(url).call().map_err(failure)?;
        read(response, max_bytes)
    }
}

fn read(
    mut response: ureq::http::Response<ureq::Body>,
    max_bytes: u64,
) -> Result<HttpResponse, TransportFailure> {
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(max_bytes)
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
pub enum MinimaxError {
    /// The request did not complete; it may or may not have reached MiniMax.
    Transport(TransportFailure),
    /// MiniMax answered with an HTTP error status.
    Http { status: u16 },
    /// MiniMax answered with a non-zero `base_resp.status_code`.
    Api { code: i64, message: String },
    /// The answer was well-formed but held no image.
    NoImage,
    /// The answer could not be read as the response it should be.
    Malformed(&'static str),
    /// The prompt is empty or longer than the model accepts.
    InvalidPrompt,
    /// The request was refused before it was sent, for example a first
    /// frame that is empty or too large.
    InvalidRequest(&'static str),
    /// A remote task ended in failure.
    TaskFailed,
}

impl MinimaxError {
    /// Whether trying the same request again can succeed.
    ///
    /// Unknown API codes are terminal, so an unrecognised failure never turns
    /// into repeated spending.
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(_) => ErrorClass::Retryable,
            Self::Http { status } if *status == 429 || *status >= 500 => ErrorClass::Retryable,
            Self::Api { code, .. } => match code {
                // unknown error, timeout, rate limit, internal error,
                // connection limit, rate-growth limit
                1000 | 1001 | 1002 | 1013 | 1041 | 2045 => ErrorClass::Retryable,
                _ => ErrorClass::Terminal,
            },
            Self::Http { .. }
            | Self::NoImage
            | Self::Malformed(_)
            | Self::InvalidPrompt
            | Self::InvalidRequest(_)
            | Self::TaskFailed => ErrorClass::Terminal,
        }
    }

    /// Whether MiniMax certainly did not accept the request: it was never
    /// sent, no connection was made, or MiniMax answered with a refusal. Any
    /// other failure may have reached MiniMax and been accepted, and charged.
    #[must_use]
    pub fn certainly_not_accepted(&self) -> bool {
        match self {
            Self::InvalidPrompt
            | Self::InvalidRequest(_)
            | Self::Api { .. }
            | Self::Transport(TransportFailure::Connect) => true,
            Self::Http { status } => (400..500).contains(status),
            Self::Transport(_) | Self::NoImage | Self::Malformed(_) | Self::TaskFailed => false,
        }
    }
}

impl fmt::Display for MinimaxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(failure) => {
                write!(formatter, "MiniMax request failed: {failure:?}")
            }
            Self::Http { status } => write!(formatter, "MiniMax answered HTTP {status}"),
            Self::Api { code, message } => {
                write!(formatter, "MiniMax refused the request ({code}): {message}")
            }
            Self::NoImage => formatter.write_str("MiniMax returned no image"),
            Self::Malformed(what) => write!(formatter, "MiniMax response is malformed: {what}"),
            Self::InvalidPrompt => {
                formatter.write_str("prompt is empty or longer than the model accepts")
            }
            Self::InvalidRequest(what) => write!(formatter, "MiniMax request is invalid: {what}"),
            Self::TaskFailed => formatter.write_str("MiniMax reported the task as failed"),
        }
    }
}

impl Error for MinimaxError {}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    aspect_ratio: &'a str,
    response_format: &'a str,
    n: u8,
    prompt_optimizer: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    subject_reference: Option<[WireSubject; 1]>,
}

#[derive(Serialize)]
struct WireSubject {
    #[serde(rename = "type")]
    kind: &'static str,
    image_file: String,
}

#[derive(Deserialize)]
struct WireResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    data: Option<WireData>,
    base_resp: WireBaseResp,
}

#[derive(Deserialize)]
struct WireData {
    #[serde(default)]
    image_base64: Vec<String>,
}

#[derive(Deserialize)]
struct WireBaseResp {
    status_code: i64,
    #[serde(default)]
    status_msg: String,
}

/// A MiniMax client for one API key.
#[derive(Clone)]
pub struct MinimaxClient<T = HttpsTransport> {
    transport: T,
    api_key: SecretString,
    base_url: String,
}

impl MinimaxClient {
    #[must_use]
    pub fn new(api_key: SecretString) -> Self {
        Self::with_transport(HttpsTransport::new(), api_key, DEFAULT_BASE_URL)
    }
}

impl MinimaxClient<SharedTransport> {
    /// A client over HTTPS that can be shared between threads.
    #[must_use]
    pub fn shared(api_key: SecretString) -> Self {
        Self::with_transport(Arc::new(HttpsTransport::new()), api_key, DEFAULT_BASE_URL)
    }
}

impl<T: Transport> MinimaxClient<T> {
    pub fn with_transport(transport: T, api_key: SecretString, base_url: &str) -> Self {
        Self {
            transport,
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// What one image costs before it is requested.
    #[must_use]
    pub fn estimate_image_cost(&self) -> CostEstimate {
        CostEstimate {
            call_count: 1,
            amount_micros: IMAGE_PRICE_MICROS,
            currency: CurrencyCode::new(*b"USD").expect("static currency code is valid"),
            estimated_duration_ms: Some(20_000),
        }
    }

    /// Generate one image. This is a paid call.
    pub fn generate_image(&self, request: &ImageRequest) -> Result<GeneratedImage, MinimaxError> {
        let chars = request.prompt.chars().count();
        if request.prompt.trim().is_empty() || chars > MAX_PROMPT_CHARS {
            return Err(MinimaxError::InvalidPrompt);
        }
        let subject_reference = match &request.subject_reference {
            None => None,
            Some(SubjectReference(bytes)) => {
                let media_type = match sniff(bytes) {
                    Some(ImageFormat::Jpeg) => "image/jpeg",
                    Some(ImageFormat::Png) => "image/png",
                    _ => {
                        return Err(MinimaxError::InvalidRequest(
                            "a subject reference must be a JPEG or PNG",
                        ));
                    }
                };
                if bytes.len() > MAX_REFERENCE_BYTES {
                    return Err(MinimaxError::InvalidRequest(
                        "a subject reference must be under 10 MB",
                    ));
                }
                Some([WireSubject {
                    kind: "character",
                    image_file: format!(
                        "data:{media_type};base64,{}",
                        base64::engine::general_purpose::STANDARD.encode(bytes)
                    ),
                }])
            }
        };
        let body = serde_json::to_vec(&WireRequest {
            model: IMAGE_MODEL,
            prompt: &request.prompt,
            aspect_ratio: request.aspect_ratio.as_str(),
            // The image arrives in the TLS answer itself, so it needs no
            // second, unauthenticated download.
            response_format: "base64",
            n: 1,
            prompt_optimizer: false,
            subject_reference,
        })
        .map_err(|_| MinimaxError::InvalidRequest("request did not encode"))?;
        let response = self
            .transport
            .post_json(
                &format!("{}/v1/image_generation", self.base_url),
                &self.api_key,
                &body,
            )
            .map_err(MinimaxError::Transport)?;
        decode(&response)
    }
}

impl<T> fmt::Debug for MinimaxClient<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MinimaxClient")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key)
            .finish_non_exhaustive()
    }
}

fn decode(response: &HttpResponse) -> Result<GeneratedImage, MinimaxError> {
    let parsed = serde_json::from_slice::<WireResponse>(&response.body);
    if !(200..300).contains(&response.status) {
        // MiniMax reports most refusals in `base_resp` even on an error status.
        return Err(match parsed {
            Ok(wire) if wire.base_resp.status_code != 0 => api_error(&wire.base_resp),
            _ => MinimaxError::Http {
                status: response.status,
            },
        });
    }
    let wire = parsed.map_err(|_| MinimaxError::Malformed("body is not an image response"))?;
    if wire.base_resp.status_code != 0 {
        return Err(api_error(&wire.base_resp));
    }
    let encoded = wire
        .data
        .and_then(|data| data.image_base64.into_iter().next())
        .ok_or(MinimaxError::NoImage)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| MinimaxError::Malformed("image is not base64"))?;
    let format = sniff(&bytes).ok_or(MinimaxError::Malformed("image format is not recognised"))?;
    Ok(GeneratedImage {
        bytes,
        format,
        request_id: wire.id.unwrap_or_default(),
    })
}

fn api_error(base: &WireBaseResp) -> MinimaxError {
    MinimaxError::Api {
        code: base.status_code,
        message: base
            .status_msg
            .chars()
            .filter(|character| !character.is_control())
            .take(MAX_STATUS_MESSAGE_CHARS)
            .collect(),
    }
}

/// The image format from its leading bytes.
#[must_use]
pub fn sniff(bytes: &[u8]) -> Option<ImageFormat> {
    if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(ImageFormat::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(ImageFormat::Png)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(ImageFormat::Webp)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::VecDeque, sync::Mutex};

    const KEY: &str = "sk-test-secret-value-0123456789";
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F'];

    #[derive(Default)]
    struct Recorded {
        responses: Mutex<VecDeque<Result<HttpResponse, TransportFailure>>>,
        requests: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl Transport for Recorded {
        fn post_json(
            &self,
            url: &str,
            api_key: &SecretString,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            self.requests.lock().unwrap().push((
                url.to_owned(),
                api_key.expose().to_owned(),
                body.to_vec(),
            ));
            self.responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("a recorded response")
        }

        fn get(
            &self,
            _url: &str,
            _api_key: &SecretString,
        ) -> Result<HttpResponse, TransportFailure> {
            unreachable!("image generation makes no GET request")
        }

        fn download(&self, _url: &str, _max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
            unreachable!("image generation downloads nothing")
        }
    }

    fn client(
        responses: impl IntoIterator<Item = Result<HttpResponse, TransportFailure>>,
    ) -> MinimaxClient<Recorded> {
        MinimaxClient::with_transport(
            Recorded {
                responses: Mutex::new(responses.into_iter().collect()),
                requests: Mutex::new(Vec::new()),
            },
            SecretString::new(KEY.to_owned()).unwrap(),
            "https://example.test/",
        )
    }

    fn answer(status: u16, body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    fn success(image: &[u8]) -> String {
        format!(
            r#"{{"id":"070cfa8b","data":{{"image_base64":["{}"]}},"metadata":{{"failed_count":"0","success_count":"1"}},"base_resp":{{"status_code":0,"status_msg":"success"}}}}"#,
            base64::engine::general_purpose::STANDARD.encode(image)
        )
    }

    fn refused(code: i64, message: &str) -> String {
        format!(
            r#"{{"id":"x","data":null,"base_resp":{{"status_code":{code},"status_msg":"{message}"}}}}"#
        )
    }

    fn request() -> ImageRequest {
        ImageRequest {
            prompt: "A filmmaker hears the final line.".to_owned(),
            aspect_ratio: AspectRatio::Landscape16x9,
            subject_reference: None,
        }
    }

    #[test]
    fn a_subject_reference_is_sent_as_a_character_data_url_within_its_limits() {
        let client = client([answer(200, &success(JPEG))]);
        let mut request = request();
        request.aspect_ratio = AspectRatio::Portrait9x16;
        request.subject_reference = Some(SubjectReference(JPEG.to_vec()));
        client.generate_image(&request).unwrap();
        let requests = client.transport.requests.lock().unwrap();
        let sent: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
        assert_eq!(sent["aspect_ratio"], "9:16");
        assert_eq!(
            sent["subject_reference"],
            serde_json::json!([{
                "type": "character",
                "image_file": format!(
                    "data:image/jpeg;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(JPEG)
                ),
            }])
        );
        drop(requests);

        // What MiniMax would refuse is refused before any request.
        let client = self::client([]);
        let mut webp = b"RIFF\0\0\0\0WEBPVP8 ".to_vec();
        webp.resize(64, 0);
        let mut huge = JPEG.to_vec();
        huge.resize(MAX_REFERENCE_BYTES + 1, 0);
        for bytes in [webp, huge, Vec::new()] {
            let mut refused = request.clone();
            refused.subject_reference = Some(SubjectReference(bytes));
            let error = client.generate_image(&refused).unwrap_err();
            assert!(matches!(error, MinimaxError::InvalidRequest(_)), "{error}");
            assert!(error.certainly_not_accepted());
        }
        assert!(client.transport.requests.lock().unwrap().is_empty());
        assert_eq!(
            format!("{:?}", SubjectReference(vec![0; 3])),
            "SubjectReference(3 bytes)"
        );
    }

    #[test]
    fn a_successful_answer_yields_the_decoded_image() {
        let client = client([answer(200, &success(JPEG))]);
        let image = client.generate_image(&request()).unwrap();
        assert_eq!(image.bytes, JPEG);
        assert_eq!(image.format, ImageFormat::Jpeg);
        assert_eq!(image.request_id, "070cfa8b");

        let requests = client.transport.requests.lock().unwrap();
        let (url, key, body) = &requests[0];
        assert_eq!(url, "https://example.test/v1/image_generation");
        assert_eq!(key, KEY);
        let sent: serde_json::Value = serde_json::from_slice(body).unwrap();
        assert_eq!(
            sent,
            serde_json::json!({
                "model": "image-01",
                "prompt": "A filmmaker hears the final line.",
                "aspect_ratio": "16:9",
                "response_format": "base64",
                "n": 1,
                "prompt_optimizer": false,
            })
        );
        assert!(!String::from_utf8_lossy(body).contains(KEY));
    }

    #[test]
    fn failures_are_classified_for_the_job_runtime() {
        let cases = [
            (
                answer(200, &refused(1002, "rate limit")),
                ErrorClass::Retryable,
            ),
            (
                answer(200, &refused(1013, "internal")),
                ErrorClass::Retryable,
            ),
            (
                answer(200, &refused(1004, "auth failed")),
                ErrorClass::Terminal,
            ),
            (
                answer(200, &refused(2049, "invalid api key")),
                ErrorClass::Terminal,
            ),
            (
                answer(200, &refused(1008, "insufficient balance")),
                ErrorClass::Terminal,
            ),
            (
                answer(200, &refused(1026, "input sensitive")),
                ErrorClass::Terminal,
            ),
            (
                answer(200, &refused(2013, "invalid params")),
                ErrorClass::Terminal,
            ),
            (
                answer(200, &refused(9999, "something new")),
                ErrorClass::Terminal,
            ),
            (answer(429, "too many"), ErrorClass::Retryable),
            (answer(503, "<html>"), ErrorClass::Retryable),
            (
                answer(401, &refused(1004, "auth failed")),
                ErrorClass::Terminal,
            ),
            (answer(404, "not found"), ErrorClass::Terminal),
            (Err(TransportFailure::Timeout), ErrorClass::Retryable),
            (Err(TransportFailure::Connect), ErrorClass::Retryable),
            (answer(200, "not json"), ErrorClass::Terminal),
            (
                answer(
                    200,
                    r#"{"data":{"image_base64":[]},"base_resp":{"status_code":0}}"#,
                ),
                ErrorClass::Terminal,
            ),
            (answer(200, &success(b"not an image")), ErrorClass::Terminal),
        ];
        for (response, class) in cases {
            let error = client([response]).generate_image(&request()).unwrap_err();
            assert_eq!(error.class(), class, "{error}");
            assert!(!error.to_string().contains(KEY));
            assert!(!format!("{error:?}").contains(KEY));
        }
    }

    #[test]
    fn a_call_is_known_unaccepted_only_if_it_never_arrived_or_was_refused() {
        let cases = [
            (answer(200, &refused(1002, "rate limit")), true),
            (answer(401, &refused(1004, "auth failed")), true),
            (answer(404, "not found"), true),
            (Err(TransportFailure::Connect), true),
            // Each of these may have reached MiniMax and been charged.
            (Err(TransportFailure::Timeout), false),
            (Err(TransportFailure::Io), false),
            (Err(TransportFailure::Tls), false),
            (answer(503, "<html>"), false),
            (answer(200, "not json"), false),
            (answer(200, &success(b"not an image")), false),
        ];
        for (response, refused) in cases {
            let error = client([response]).generate_image(&request()).unwrap_err();
            assert_eq!(error.certainly_not_accepted(), refused, "{error}");
        }
        let unsent = ImageRequest {
            prompt: String::new(),
            aspect_ratio: AspectRatio::Square,
            subject_reference: None,
        };
        assert!(
            client([])
                .generate_image(&unsent)
                .unwrap_err()
                .certainly_not_accepted()
        );
    }

    #[test]
    fn declared_capabilities_match_what_the_client_sends() {
        use kineto_providers::{ImageRequirements, SeedSupport, evaluate_image};
        let capabilities = image_capabilities();
        // Every declared ratio is one the client can ask for, and back.
        let sent = [
            AspectRatio::Square,
            AspectRatio::Landscape16x9,
            AspectRatio::Portrait9x16,
        ]
        .map(AspectRatio::as_str);
        assert_eq!(capabilities.aspect_ratios.len(), sent.len());
        for ratio in &capabilities.aspect_ratios {
            assert!(sent.contains(&format!("{}:{}", ratio.width, ratio.height).as_str()));
        }
        assert_eq!(capabilities.max_prompt_chars, Some(MAX_PROMPT_CHARS));
        assert_eq!(capabilities.reference_limit(ReferenceRole::Identity), 1);
        assert_eq!(capabilities.reference_limit(ReferenceRole::Style), 0);
        assert_eq!(capabilities.reference_limit(ReferenceRole::Composition), 0);
        assert!(capabilities.max_reference_bytes < Some(10 * 1024 * 1024));
        let needs = |role| ImageRequirements {
            required_reference_role: Some(role),
            minimum_seed_support: SeedSupport::None,
            requires_negative_prompt: false,
            requires_mask: false,
            requires_synchronous: true,
        };
        assert!(evaluate_image(needs(ReferenceRole::Identity), &capabilities).is_compatible());
        assert!(!evaluate_image(needs(ReferenceRole::Style), &capabilities).is_compatible());
        // The client returns the image itself, at the price it estimates.
        assert!(!capabilities.async_jobs);
        let estimate = client([]).estimate_image_cost();
        assert_eq!(
            capabilities.price_per_image,
            Some(Price {
                amount_micros: estimate.amount_micros,
                currency: estimate.currency.bytes(),
            })
        );
    }

    #[test]
    fn api_refusals_keep_the_vendor_message_bounded_and_printable() {
        // A JSON-escaped control character, decoded by the parser.
        let message = format!("bad\\u0007{}", "x".repeat(500));
        let error = client([answer(200, &refused(2013, &message))])
            .generate_image(&request())
            .unwrap_err();
        let MinimaxError::Api { code, message } = error else {
            panic!("an API refusal");
        };
        assert_eq!(code, 2013);
        assert_eq!(message.chars().count(), MAX_STATUS_MESSAGE_CHARS);
        assert!(message.starts_with("badx"));
    }

    #[test]
    fn an_invalid_prompt_is_refused_before_any_request() {
        let client = client([]);
        for prompt in ["", "   ", &"x".repeat(MAX_PROMPT_CHARS + 1)] {
            let request = ImageRequest {
                prompt: prompt.to_owned(),
                aspect_ratio: AspectRatio::Square,
                subject_reference: None,
            };
            assert_eq!(
                client.generate_image(&request),
                Err(MinimaxError::InvalidPrompt)
            );
        }
        assert!(client.transport.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn the_client_never_prints_its_key() {
        let client = client([]);
        assert!(!format!("{client:?}").contains(KEY));
        assert_eq!(
            client.estimate_image_cost().amount_micros,
            IMAGE_PRICE_MICROS
        );
    }

    #[test]
    fn formats_are_recognised_from_their_leading_bytes() {
        assert_eq!(sniff(JPEG), Some(ImageFormat::Jpeg));
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\nrest"), Some(ImageFormat::Png));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some(ImageFormat::Webp));
        assert_eq!(sniff(b"GIF89a"), None);
        assert_eq!(sniff(b""), None);
    }
}
