//! Google Veo 3.1 video over the Gemini API.
//!
//! A clip starts from an image and may close on another
//! (`models/<model>:predictLongRunning`, the paid step). Veo answers with a
//! long-running operation, which is asked about until it holds the video's
//! URI or an error, and the video is then downloaded. Veo keeps a finished
//! video for 2 days.
//!
//! The key goes in an `x-goog-api-key` header, and only to the API's own
//! host. A download that is redirected elsewhere is fetched without it.
//!
//! What each model renders and costs is declared from Google's
//! documentation and price list, and from a live trial on 2026-10-05: a
//! last frame is taken only with an 8 s clip, and frames are sent as
//! `bytesBase64Encoded` (the documented `inlineData` is refused). Every
//! clip carries an AAC sound track Veo makes itself; it cannot be turned
//! off.
//!
//! A clip is framed 16:9 or 9:16. Standard and Fast also take up to 3
//! reference images of a subject to keep (`referenceImages`, as `asset`),
//! only in an 8 s clip; Lite takes none. They are sent the way frames are,
//! and have not been tried live.

use std::{collections::BTreeSet, error::Error, fmt, sync::Arc, time::Duration};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_jobs::ErrorClass;
use kineto_providers::{
    AspectRatio, CameraControl, ImageFormat, Price, ReferenceRole, VideoCapabilities, VideoClip,
    VideoContainer, VideoMode, secret::SecretString,
};
use serde::{Deserialize, Serialize};

/// The provider name a session selects Veo by, and its key is read under.
pub const PROVIDER: &str = "veo";
pub const DEFAULT_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
/// Largest video downloaded. An 8 s 4k clip is well under it.
pub const MAX_VIDEO_BYTES: u64 = 512 * 1024 * 1024;
/// Largest first or last frame sent. A request carries both, base64
/// encoded, under the API's 20 MB inline limit.
pub const MAX_FRAME_BYTES: u64 = 7 * 1024 * 1024;
/// Largest reference image sent.
pub const MAX_REFERENCE_BYTES: u64 = MAX_FRAME_BYTES;
/// Most images one request carries together, in bytes before encoding: two
/// frames at their largest, which keeps the request under the inline limit.
pub const MAX_REQUEST_IMAGE_BYTES: u64 = 2 * MAX_FRAME_BYTES;
/// Most reference images Standard and Fast take.
pub const MAX_REFERENCE_IMAGES: u16 = 3;
/// Longest prompt sent. Veo reads 1,024 tokens; English runs about four
/// characters a token, so this leaves room.
pub const MAX_PROMPT_CHARS: usize = 3_000;
/// The only length Veo closes a clip on a last frame at, or takes
/// reference images in.
pub const END_FRAME_SECONDS: u8 = 8;
/// The frame shapes Veo renders.
pub const ASPECT_RATIOS: [AspectRatio; 2] = [
    AspectRatio {
        width: 16,
        height: 9,
    },
    AspectRatio {
        width: 9,
        height: 16,
    },
];

const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
const MAX_MESSAGE_CHARS: usize = 200;
const FPS: u16 = 24;

/// A Veo 3.1 model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VeoModel {
    /// The cheapest: 720p or 1080p.
    #[default]
    Lite,
    Fast,
    Standard,
}

impl VeoModel {
    /// The name a session selects it by after `veo:`, as in `veo:fast`.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "lite" => Some(Self::Lite),
            "fast" => Some(Self::Fast),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }

    /// Its model ID on the Gemini API.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Lite => "veo-3.1-lite-generate-preview",
            Self::Fast => "veo-3.1-fast-generate-preview",
            Self::Standard => "veo-3.1-generate-preview",
        }
    }

    /// What one second costs at each height it renders, in USD micros.
    const fn per_second(self) -> &'static [(u16, u64)] {
        match self {
            Self::Lite => &[(720, 50_000), (1080, 80_000)],
            Self::Fast => &[(720, 100_000), (1080, 120_000), (2160, 300_000)],
            Self::Standard => &[(720, 400_000), (1080, 400_000), (2160, 600_000)],
        }
    }

    /// How many reference images it takes: none for Lite.
    const fn max_reference_images(self) -> u16 {
        match self {
            Self::Lite => 0,
            Self::Fast | Self::Standard => MAX_REFERENCE_IMAGES,
        }
    }

    /// What it can do, as documented: 4, 6 or 8 s at 720p, and only 8 s
    /// above it; 24 fps; 16:9 or 9:16; a first frame and, in an 8 s clip, a
    /// last one and reference images where it takes them; camera motion in
    /// words; its own sound.
    #[must_use]
    pub fn capabilities(self) -> VideoCapabilities {
        let mut clips = BTreeSet::new();
        let mut clip_prices = std::collections::BTreeMap::new();
        for &(lines, micros) in self.per_second() {
            let lengths: &[u8] = if lines == 720 { &[4, 6, 8] } else { &[8] };
            for &seconds in lengths {
                let clip = VideoClip { seconds, lines };
                clips.insert(clip);
                clip_prices.insert(clip, Price::usd(micros * u64::from(seconds)));
            }
        }
        let frames: BTreeSet<ImageFormat> = [ImageFormat::Jpeg, ImageFormat::Png].into();
        VideoCapabilities {
            modes: [VideoMode::ImageToVideo].into(),
            min_duration_ms: Some(4_000),
            max_duration_ms: Some(8_000),
            fps_options: [FPS].into(),
            start_frame: true,
            end_frame: true,
            camera_control: CameraControl::Textual,
            async_jobs: true,
            resumable_remote_handle: true,
            output_containers: [VideoContainer::Mp4].into(),
            aspect_ratios: ASPECT_RATIOS.into(),
            max_reference_images: self.max_reference_images(),
            reference_roles: if self.max_reference_images() == 0 {
                BTreeSet::new()
            } else {
                [ReferenceRole::Identity].into()
            },
            reference_formats: if self.max_reference_images() == 0 {
                BTreeSet::new()
            } else {
                frames.clone()
            },
            max_reference_bytes: (self.max_reference_images() > 0).then_some(MAX_REFERENCE_BYTES),
            reference_seconds: [END_FRAME_SECONDS].into(),
            clip_seconds: [4, 6, 8].into(),
            max_prompt_chars: Some(MAX_PROMPT_CHARS),
            start_frame_formats: frames.clone(),
            max_start_frame_bytes: Some(MAX_FRAME_BYTES),
            end_frame_formats: frames,
            max_end_frame_bytes: Some(MAX_FRAME_BYTES),
            resolutions: self.per_second().iter().map(|(lines, _)| *lines).collect(),
            clips,
            clip_prices,
            end_frame_seconds: [END_FRAME_SECONDS].into(),
            audio: true,
        }
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

/// Talks to the Gemini API. Production uses [`HttpsTransport`]; tests
/// replay recorded responses.
pub trait Transport {
    /// A POST of a JSON body to the API, with the key.
    fn post_json(
        &self,
        url: &str,
        api_key: &SecretString,
        body: &[u8],
    ) -> Result<HttpResponse, TransportFailure>;

    /// A GET from the API, with the key.
    fn get(&self, url: &str, api_key: &SecretString) -> Result<HttpResponse, TransportFailure>;

    /// A GET of a video file on the API, with the key, at most `max_bytes`
    /// long. A redirect to another host is followed without the key.
    fn download(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure>;
}

/// A transport shared between clients and threads.
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

    fn download(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        self.as_ref().download(url, api_key, max_bytes)
    }
}

/// Blocking HTTPS with rustls and the bundled Mozilla roots.
#[derive(Clone)]
pub struct HttpsTransport {
    agent: ureq::Agent,
    files: ureq::Agent,
    redirected: ureq::Agent,
}

impl HttpsTransport {
    #[must_use]
    pub fn new() -> Self {
        let agent = |timeout, redirects| -> ureq::Agent {
            ureq::Agent::config_builder()
                .https_only(true)
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
            agent: agent(REQUEST_TIMEOUT, 0),
            files: agent(DOWNLOAD_TIMEOUT, 0),
            // Where a file was redirected to carries no key.
            redirected: agent(DOWNLOAD_TIMEOUT, 3),
        }
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
            .header("x-goog-api-key", api_key.expose())
            .header("Content-Type", "application/json")
            .send(body)
            .map_err(failure)?;
        read(response, MAX_RESPONSE_BYTES)
    }

    fn get(&self, url: &str, api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
        let response = self
            .agent
            .get(url)
            .header("x-goog-api-key", api_key.expose())
            .call()
            .map_err(failure)?;
        read(response, MAX_RESPONSE_BYTES)
    }

    fn download(
        &self,
        url: &str,
        api_key: &SecretString,
        max_bytes: u64,
    ) -> Result<HttpResponse, TransportFailure> {
        let response = self
            .files
            .get(url)
            .header("x-goog-api-key", api_key.expose())
            .call()
            .map_err(failure)?;
        if !response.status().is_redirection() {
            return read(response, max_bytes);
        }
        let location = response
            .headers()
            .get("location")
            .and_then(|value| value.to_str().ok())
            .ok_or(TransportFailure::Protocol)?
            .to_owned();
        let response = if same_host(url, &location) {
            self.files
                .get(&location)
                .header("x-goog-api-key", api_key.expose())
                .call()
        } else {
            self.redirected.get(&location).call()
        }
        .map_err(failure)?;
        read(response, max_bytes)
    }
}

/// The scheme and host of `url`, as in `https://example.com`.
fn origin(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    Some(&url[.."https://".len() + end])
}

fn same_host(one: &str, other: &str) -> bool {
    origin(one).is_some_and(|origin| Some(origin) == self::origin(other))
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
pub enum VeoError {
    /// The request did not complete; it may or may not have reached Veo.
    Transport(TransportFailure),
    /// The API answered with an HTTP error status, and its message.
    Http { status: u16, message: String },
    /// The answer could not be read as the response it should be.
    Malformed(&'static str),
    /// The request was refused before it was sent.
    InvalidRequest(&'static str),
    /// The operation ended without a video. Nothing is charged for it.
    Failed { code: i64, message: String },
    /// The operation ended with its video withheld by Veo's filters.
    Filtered(String),
    /// The video's file answered with an HTTP error status.
    Download { status: u16 },
}

impl VeoError {
    /// Whether trying the same request again can succeed. A finished
    /// operation answers the same way however often it is asked, so its
    /// failure is final; asking for the video again starts another.
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(_) => ErrorClass::Retryable,
            Self::Http { status, .. } if *status == 429 || *status >= 500 => ErrorClass::Retryable,
            // Veo keeps a video for 2 days; after that the file is gone.
            Self::Download { status } => match status {
                429 | 500.. => ErrorClass::Retryable,
                _ => ErrorClass::Terminal,
            },
            Self::Http { .. }
            | Self::Malformed(_)
            | Self::InvalidRequest(_)
            | Self::Failed { .. }
            | Self::Filtered(_) => ErrorClass::Terminal,
        }
    }

    /// How an operation that ended with this is to be read once it is
    /// final: retryable where asking for the video again often works. Veo
    /// charges only for a video it delivers, and an operation it was too
    /// busy for, or whose picture or sound its filters withheld, often
    /// succeeds the next time.
    #[must_use]
    pub fn ended_class(&self) -> ErrorClass {
        match self {
            Self::Failed { .. } | Self::Filtered(_) => ErrorClass::Retryable,
            other => other.class(),
        }
    }

    /// Whether Veo certainly did not accept a submission: it was never
    /// sent, no connection was made, or the API refused it. Any other
    /// failure may have started an operation, and been charged for.
    #[must_use]
    pub fn certainly_not_accepted(&self) -> bool {
        match self {
            Self::InvalidRequest(_) | Self::Transport(TransportFailure::Connect) => true,
            Self::Http { status, .. } => (400..500).contains(status),
            Self::Transport(_)
            | Self::Malformed(_)
            | Self::Failed { .. }
            | Self::Filtered(_)
            | Self::Download { .. } => false,
        }
    }
}

impl fmt::Display for VeoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(failure) => write!(formatter, "Veo request failed: {failure:?}"),
            Self::Http { status, message } if message.is_empty() => {
                write!(formatter, "Veo answered HTTP {status}")
            }
            Self::Http { status, message } => {
                write!(formatter, "Veo answered HTTP {status}: {message}")
            }
            Self::Malformed(what) => write!(formatter, "Veo response is malformed: {what}"),
            Self::InvalidRequest(what) => write!(formatter, "Veo request is invalid: {what}"),
            // Code 14: Veo was too busy. Asking again a little later works.
            Self::Failed { code: 14, message } => write!(
                formatter,
                "Veo was too busy to make the video (14): {message}. Nothing was charged; ask again"
            ),
            Self::Failed { code, message } => write!(
                formatter,
                "Veo could not make the video ({code}): {message}. Nothing was charged"
            ),
            Self::Filtered(reason) => write!(
                formatter,
                "Veo withheld the video: {reason}. No video was made, so nothing was charged"
            ),
            Self::Download { status } => write!(
                formatter,
                "the video's file answered HTTP {status}; Veo keeps a video for 2 days"
            ),
        }
    }
}

impl Error for VeoError {}

/// One video asked of Veo.
#[derive(Clone, PartialEq, Eq)]
pub struct VeoRequest {
    pub prompt: String,
    pub first_frame: (Vec<u8>, ImageFormat),
    pub last_frame: Option<(Vec<u8>, ImageFormat)>,
    pub seconds: u8,
    /// The clip's height in lines: 720, 1080 or 2160.
    pub lines: u16,
    /// The clip's frame shape: 16:9 or 9:16.
    pub aspect_ratio: AspectRatio,
    /// Images of subjects the clip keeps as they look, as `asset`
    /// references.
    pub references: Vec<(Vec<u8>, ImageFormat)>,
}

impl fmt::Debug for VeoRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VeoRequest")
            .field("prompt", &self.prompt)
            .field("first_frame", &self.first_frame.0.len())
            .field(
                "last_frame",
                &self.last_frame.as_ref().map(|(bytes, _)| bytes.len()),
            )
            .field("seconds", &self.seconds)
            .field("lines", &self.lines)
            .field("aspect_ratio", &self.aspect_ratio)
            .field(
                "references",
                &self
                    .references
                    .iter()
                    .map(|(bytes, _)| bytes.len())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Where an operation is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationState {
    Running,
    /// Done, with the URI of its video.
    Succeeded {
        video: String,
    },
}

#[derive(Serialize)]
struct WireRequest<'a> {
    instances: [WireInstance<'a>; 1],
    parameters: WireParameters,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireInstance<'a> {
    prompt: &'a str,
    image: WireImage,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_frame: Option<WireImage>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    reference_images: Vec<WireReference>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireReference {
    image: WireImage,
    reference_type: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireImage {
    bytes_base64_encoded: String,
    mime_type: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct WireParameters {
    aspect_ratio: &'static str,
    duration_seconds: u8,
    resolution: &'static str,
    person_generation: &'static str,
}

#[derive(Deserialize)]
struct WireSubmitted {
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct WireOperation {
    #[serde(default)]
    done: bool,
    #[serde(default)]
    error: Option<WireStatus>,
    #[serde(default)]
    response: Option<WireResponse>,
}

#[derive(Deserialize)]
struct WireStatus {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireResponse {
    #[serde(default)]
    generate_video_response: Option<WireVideoResponse>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WireVideoResponse {
    #[serde(default)]
    generated_samples: Vec<WireSample>,
    #[serde(default)]
    rai_media_filtered_reasons: Vec<String>,
}

#[derive(Deserialize)]
struct WireSample {
    #[serde(default)]
    video: Option<WireVideo>,
}

#[derive(Deserialize)]
struct WireVideo {
    #[serde(default)]
    uri: Option<String>,
}

#[derive(Deserialize)]
struct WireError {
    error: WireStatus,
}

/// A Veo client for one API key and model.
#[derive(Clone)]
pub struct VeoClient<T = HttpsTransport> {
    transport: T,
    api_key: SecretString,
    base_url: String,
    model: VeoModel,
}

impl VeoClient<SharedTransport> {
    /// A client over HTTPS to the Gemini API, shareable between threads.
    #[must_use]
    pub fn shared(api_key: SecretString, model: VeoModel) -> Self {
        Self::with_transport(
            Arc::new(HttpsTransport::new()),
            api_key,
            DEFAULT_BASE_URL,
            model,
        )
    }
}

impl<T> fmt::Debug for VeoClient<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VeoClient")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .finish_non_exhaustive()
    }
}

impl<T: Transport> VeoClient<T> {
    pub fn with_transport(
        transport: T,
        api_key: SecretString,
        base_url: &str,
        model: VeoModel,
    ) -> Self {
        Self {
            transport,
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model,
        }
    }

    #[must_use]
    pub const fn model(&self) -> VeoModel {
        self.model
    }

    /// Start making `request`'s video: the paid call. Returns the
    /// operation to follow.
    pub fn submit(&self, request: &VeoRequest) -> Result<String, VeoError> {
        let capabilities = self.model.capabilities();
        let prompt = request.prompt.trim();
        if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(VeoError::InvalidRequest(
                "the prompt is empty or longer than Veo reads",
            ));
        }
        if !capabilities.renders(VideoClip {
            seconds: request.seconds,
            lines: request.lines,
        }) {
            return Err(VeoError::InvalidRequest(
                "the model does not render a clip of that length and height",
            ));
        }
        if request.last_frame.is_some() && request.seconds != END_FRAME_SECONDS {
            return Err(VeoError::InvalidRequest(
                "Veo closes on a last frame only in an 8 s clip",
            ));
        }
        let aspect_ratio = aspect_ratio(request.aspect_ratio)
            .filter(|_| capabilities.frames(request.aspect_ratio))
            .ok_or(VeoError::InvalidRequest("Veo frames a clip 16:9 or 9:16"))?;
        if request.references.len() > usize::from(capabilities.max_reference_images) {
            return Err(VeoError::InvalidRequest(
                "the model takes fewer reference images; Lite takes none",
            ));
        }
        if !request.references.is_empty()
            && !capabilities.reference_seconds.contains(&request.seconds)
        {
            return Err(VeoError::InvalidRequest(
                "Veo takes reference images only in an 8 s clip",
            ));
        }
        let images = std::iter::once(&request.first_frame)
            .chain(&request.last_frame)
            .chain(&request.references);
        if images.map(|(bytes, _)| bytes.len() as u64).sum::<u64>() > MAX_REQUEST_IMAGE_BYTES {
            return Err(VeoError::InvalidRequest(
                "the request's images are larger together than Veo takes",
            ));
        }
        let image = |max: u64| {
            move |(bytes, format): &(Vec<u8>, ImageFormat)| {
                let mime_type = match format {
                    ImageFormat::Jpeg => "image/jpeg",
                    ImageFormat::Png => "image/png",
                    ImageFormat::Webp => {
                        return Err(VeoError::InvalidRequest("Veo takes JPEG or PNG images"));
                    }
                };
                if bytes.is_empty() || bytes.len() as u64 > max {
                    return Err(VeoError::InvalidRequest(
                        "an image is empty or larger than Veo takes",
                    ));
                }
                Ok(WireImage {
                    bytes_base64_encoded: STANDARD.encode(bytes),
                    mime_type,
                })
            }
        };
        let frame = image(MAX_FRAME_BYTES);
        let body = serde_json::to_vec(&WireRequest {
            instances: [WireInstance {
                prompt,
                image: frame(&request.first_frame)?,
                last_frame: request.last_frame.as_ref().map(&frame).transpose()?,
                reference_images: request
                    .references
                    .iter()
                    .map(|reference| {
                        image(MAX_REFERENCE_BYTES)(reference).map(|image| WireReference {
                            image,
                            reference_type: "asset",
                        })
                    })
                    .collect::<Result<_, _>>()?,
            }],
            parameters: WireParameters {
                aspect_ratio,
                duration_seconds: request.seconds,
                resolution: resolution(request.lines)
                    .ok_or(VeoError::InvalidRequest("Veo renders 720p, 1080p or 4k"))?,
                // The only setting Veo allows from an image.
                person_generation: "allow_adult",
            },
        })
        .map_err(|_| VeoError::InvalidRequest("the request did not encode"))?;
        let url = format!(
            "{}/models/{}:predictLongRunning",
            self.base_url,
            self.model.id()
        );
        let response = self
            .transport
            .post_json(&url, &self.api_key, &body)
            .map_err(VeoError::Transport)?;
        let body = answered(response)?;
        let submitted: WireSubmitted = serde_json::from_slice(&body)
            .map_err(|_| VeoError::Malformed("the submission's answer is not JSON"))?;
        submitted
            .name
            .filter(|name| is_operation(name))
            .ok_or(VeoError::Malformed("the submission named no operation"))
    }

    /// Where operation `name` is. Free.
    pub fn operation(&self, name: &str) -> Result<OperationState, VeoError> {
        if !is_operation(name) {
            return Err(VeoError::InvalidRequest("that is not an operation name"));
        }
        let url = format!("{}/{name}", self.base_url);
        let response = self
            .transport
            .get(&url, &self.api_key)
            .map_err(VeoError::Transport)?;
        let operation: WireOperation = serde_json::from_slice(&answered(response)?)
            .map_err(|_| VeoError::Malformed("the operation is not JSON"))?;
        if !operation.done {
            return Ok(OperationState::Running);
        }
        if let Some(error) = operation.error {
            return Err(VeoError::Failed {
                code: error.code,
                message: clipped(&error.message),
            });
        }
        let response = operation
            .response
            .and_then(|response| response.generate_video_response)
            .ok_or(VeoError::Malformed(
                "a finished operation holds no response",
            ))?;
        match response
            .generated_samples
            .into_iter()
            .find_map(|sample| sample.video.and_then(|video| video.uri))
        {
            Some(video) if video.starts_with("https://") => Ok(OperationState::Succeeded { video }),
            Some(_) => Err(VeoError::Malformed("the video's URI is not HTTPS")),
            None if !response.rai_media_filtered_reasons.is_empty() => Err(VeoError::Filtered(
                clipped(&response.rai_media_filtered_reasons.join("; ")),
            )),
            None => Err(VeoError::Malformed("a finished operation holds no video")),
        }
    }

    /// Download the video at `uri`, which [`Self::operation`] gave. Free.
    /// The key is sent only when the file is on the API's own host.
    pub fn download(&self, uri: &str) -> Result<Vec<u8>, VeoError> {
        if !same_host(uri, &self.base_url) {
            return Err(VeoError::Malformed("the video is not on the API's host"));
        }
        let response = self
            .transport
            .download(uri, &self.api_key, MAX_VIDEO_BYTES)
            .map_err(VeoError::Transport)?;
        if !(200..300).contains(&response.status) {
            return Err(VeoError::Download {
                status: response.status,
            });
        }
        if response.body.get(4..8) != Some(b"ftyp") {
            return Err(VeoError::Malformed("the video is not an MP4"));
        }
        Ok(response.body)
    }
}

/// The name Veo gives a frame shape.
fn aspect_ratio(ratio: AspectRatio) -> Option<&'static str> {
    match (ratio.width, ratio.height) {
        (16, 9) => Some("16:9"),
        (9, 16) => Some("9:16"),
        _ => None,
    }
}

/// The resolution Veo names a height by.
fn resolution(lines: u16) -> Option<&'static str> {
    match lines {
        720 => Some("720p"),
        1080 => Some("1080p"),
        2160 => Some("4k"),
        _ => None,
    }
}

/// Whether `name` is an operation as Veo names one:
/// `models/<model>/operations/<id>`, nothing that leaves the API's path.
fn is_operation(name: &str) -> bool {
    name.starts_with("models/")
        && name.contains("/operations/")
        && !name.contains("..")
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
}

/// The body of a successful answer, or the API's error.
fn answered(response: HttpResponse) -> Result<Vec<u8>, VeoError> {
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let message = serde_json::from_slice::<WireError>(&response.body)
        .map(|wire| clipped(&wire.error.message))
        .unwrap_or_default();
    Err(VeoError::Http {
        status: response.status,
        message,
    })
}

fn clipped(message: &str) -> String {
    let message = message.trim();
    match message.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((end, _)) => format!("{}…", &message[..end]),
        None => message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const OPERATION: &str = "models/veo-3.1-lite-generate-preview/operations/op123";
    const VIDEO: &str =
        "https://generativelanguage.googleapis.com/v1beta/files/abc:download?alt=media";
    const MP4: &[u8] = b"\0\0\0\x18ftypmp42rest of the file";

    /// The Gemini API, offline: the answers in order, and each request.
    #[derive(Default)]
    struct Recorded {
        answers: Mutex<Vec<Result<HttpResponse, TransportFailure>>>,
        requests: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl Recorded {
        fn answering(answers: Vec<Result<HttpResponse, TransportFailure>>) -> Self {
            Self {
                answers: Mutex::new(answers.into_iter().rev().collect()),
                requests: Mutex::default(),
            }
        }

        fn next(
            &self,
            method: &str,
            url: &str,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            self.requests
                .lock()
                .unwrap()
                .push((method.to_owned(), url.to_owned(), body.to_vec()));
            self.answers
                .lock()
                .unwrap()
                .pop()
                .expect("an answer is recorded")
        }
    }

    impl Transport for &Recorded {
        fn post_json(
            &self,
            url: &str,
            api_key: &SecretString,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            assert_eq!(api_key.expose(), "AQ.test");
            self.next("POST", url, body)
        }

        fn get(&self, url: &str, api_key: &SecretString) -> Result<HttpResponse, TransportFailure> {
            assert_eq!(api_key.expose(), "AQ.test");
            self.next("GET", url, &[])
        }

        fn download(
            &self,
            url: &str,
            api_key: &SecretString,
            max_bytes: u64,
        ) -> Result<HttpResponse, TransportFailure> {
            assert_eq!(api_key.expose(), "AQ.test");
            assert_eq!(max_bytes, MAX_VIDEO_BYTES);
            self.next("DOWNLOAD", url, &[])
        }
    }

    fn ok(body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status: 200,
            body: body.as_bytes().to_vec(),
        })
    }

    fn client(recorded: &Recorded, model: VeoModel) -> VeoClient<&Recorded> {
        VeoClient::with_transport(
            recorded,
            SecretString::new("AQ.test".to_owned()).unwrap(),
            DEFAULT_BASE_URL,
            model,
        )
    }

    fn request(seconds: u8, last_frame: bool) -> VeoRequest {
        VeoRequest {
            prompt: "A woman waits by the window. No speech".to_owned(),
            first_frame: (vec![0xff, 0xd8, 0xff, 0xe0], ImageFormat::Jpeg),
            last_frame: last_frame.then(|| (vec![0x89, b'P', b'N', b'G'], ImageFormat::Png)),
            seconds,
            lines: 720,
            aspect_ratio: ASPECT_RATIOS[0],
            references: Vec::new(),
        }
    }

    #[test]
    fn each_model_declares_its_clips_and_prices_as_documented() {
        let lite = VeoModel::Lite.capabilities();
        let clip = |seconds, lines| VideoClip { seconds, lines };
        assert_eq!(
            lite.clips,
            [clip(4, 720), clip(6, 720), clip(8, 720), clip(8, 1080)].into()
        );
        assert_eq!(lite.clip_prices[&clip(8, 720)], Price::usd(400_000));
        assert_eq!(lite.clip_prices[&clip(4, 720)], Price::usd(200_000));
        assert_eq!(lite.clip_prices[&clip(8, 1080)], Price::usd(640_000));
        assert_eq!(lite.clip_lines(4), Some(720));
        assert_eq!(lite.clip_lines(8), Some(720));
        assert!(!lite.renders(clip(6, 1080)));
        assert_eq!(lite.end_frame_seconds, [8].into());
        assert!(lite.audio && lite.end_frame && lite.async_jobs && lite.resumable_remote_handle);
        assert_eq!(lite.camera_control, CameraControl::Textual);
        let wide = AspectRatio::new(16, 9).unwrap();
        let tall = AspectRatio::new(9, 16).unwrap();
        assert_eq!(lite.aspect_ratios, [wide, tall].into());
        assert!(!lite.frames(AspectRatio::new(1, 1).unwrap()));
        assert_eq!(lite.max_reference_images, 0);
        assert_eq!(lite.reference_limit(ReferenceRole::Identity), 0);

        for model in [VeoModel::Fast, VeoModel::Standard] {
            let capabilities = model.capabilities();
            assert_eq!(capabilities.aspect_ratios, [wide, tall].into());
            assert_eq!(capabilities.reference_limit(ReferenceRole::Identity), 3);
            assert_eq!(capabilities.reference_limit(ReferenceRole::Style), 0);
            assert_eq!(capabilities.reference_seconds, [8].into());
            assert_eq!(
                capabilities.reference_formats,
                [ImageFormat::Jpeg, ImageFormat::Png].into()
            );
            assert_eq!(capabilities.max_reference_bytes, Some(MAX_REFERENCE_BYTES));
        }

        let fast = VeoModel::Fast.capabilities();
        assert!(fast.renders(clip(8, 2160)));
        assert_eq!(fast.clip_prices[&clip(8, 2160)], Price::usd(2_400_000));
        assert_eq!(fast.clip_prices[&clip(6, 720)], Price::usd(600_000));
        let standard = VeoModel::Standard.capabilities();
        assert_eq!(standard.clip_prices[&clip(8, 1080)], Price::usd(3_200_000));
        assert_eq!(standard.clip_prices[&clip(8, 2160)], Price::usd(4_800_000));

        assert_eq!(VeoModel::parse("fast"), Some(VeoModel::Fast));
        assert_eq!(VeoModel::parse("Fast"), None);
        assert_eq!(VeoModel::default().id(), "veo-3.1-lite-generate-preview");
    }

    #[test]
    fn a_clip_is_submitted_with_its_frames_as_base64_and_followed_to_its_video() {
        let recorded = Recorded::answering(vec![
            ok(&format!(r#"{{"name":"{OPERATION}"}}"#)),
            ok(&format!(r#"{{"name":"{OPERATION}"}}"#)),
            ok(&format!(
                r#"{{"name":"{OPERATION}","done":true,"response":{{"@type":"type.googleapis.com/google.ai.generativelanguage.v1beta.PredictLongRunningResponse","generateVideoResponse":{{"generatedSamples":[{{"video":{{"uri":"{VIDEO}"}}}}]}}}}}}"#
            )),
            Ok(HttpResponse {
                status: 200,
                body: MP4.to_vec(),
            }),
        ]);
        let veo = client(&recorded, VeoModel::Lite);
        assert_eq!(veo.submit(&request(8, true)).unwrap(), OPERATION);
        assert_eq!(veo.operation(OPERATION).unwrap(), OperationState::Running);
        assert_eq!(
            veo.operation(OPERATION).unwrap(),
            OperationState::Succeeded {
                video: VIDEO.to_owned()
            }
        );
        assert_eq!(veo.download(VIDEO).unwrap(), MP4);

        let requests = recorded.requests.lock().unwrap();
        assert_eq!(
            requests[0].1,
            "https://generativelanguage.googleapis.com/v1beta/models/veo-3.1-lite-generate-preview:predictLongRunning"
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "instances": [{
                    "prompt": "A woman waits by the window. No speech",
                    "image": {"bytesBase64Encoded": "/9j/4A==", "mimeType": "image/jpeg"},
                    "lastFrame": {"bytesBase64Encoded": "iVBORw==", "mimeType": "image/png"}
                }],
                "parameters": {
                    "aspectRatio": "16:9",
                    "durationSeconds": 8,
                    "resolution": "720p",
                    "personGeneration": "allow_adult"
                }
            })
        );
        assert_eq!(
            requests[1].1,
            format!("https://generativelanguage.googleapis.com/v1beta/{OPERATION}")
        );
        assert_eq!(
            requests[3],
            ("DOWNLOAD".to_owned(), VIDEO.to_owned(), Vec::new())
        );
    }

    #[test]
    fn a_tall_clip_with_references_is_sent_as_asked() {
        let recorded = Recorded::answering(vec![ok(&format!(r#"{{"name":"{OPERATION}"}}"#))]);
        let veo = client(&recorded, VeoModel::Fast);
        let mut tall = request(8, false);
        tall.aspect_ratio = AspectRatio::new(9, 16).unwrap();
        tall.references = vec![
            (vec![0x89, b'P', b'N', b'G'], ImageFormat::Png),
            (vec![0xff, 0xd8, 0xff, 0xe1], ImageFormat::Jpeg),
        ];
        assert_eq!(veo.submit(&tall).unwrap(), OPERATION);

        let requests = recorded.requests.lock().unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "instances": [{
                    "prompt": "A woman waits by the window. No speech",
                    "image": {"bytesBase64Encoded": "/9j/4A==", "mimeType": "image/jpeg"},
                    "referenceImages": [
                        {
                            "image": {"bytesBase64Encoded": "iVBORw==", "mimeType": "image/png"},
                            "referenceType": "asset"
                        },
                        {
                            "image": {"bytesBase64Encoded": "/9j/4Q==", "mimeType": "image/jpeg"},
                            "referenceType": "asset"
                        }
                    ]
                }],
                "parameters": {
                    "aspectRatio": "9:16",
                    "durationSeconds": 8,
                    "resolution": "720p",
                    "personGeneration": "allow_adult"
                }
            })
        );
    }

    #[test]
    fn references_are_refused_where_the_model_takes_none_or_fewer() {
        let recorded = Recorded::default();
        let reference = (vec![0x89, b'P', b'N', b'G'], ImageFormat::Png);
        let with = |count: usize, seconds: u8| VeoRequest {
            references: vec![reference.clone(); count],
            ..request(seconds, false)
        };
        let refused = |model, request: &VeoRequest| {
            matches!(
                client(&recorded, model).submit(request),
                Err(VeoError::InvalidRequest(_))
            )
        };
        assert!(refused(VeoModel::Lite, &with(1, 8)), "Lite takes none");
        assert!(refused(VeoModel::Fast, &with(4, 8)), "at most 3");
        assert!(refused(VeoModel::Standard, &with(4, 8)), "at most 3");
        assert!(refused(VeoModel::Fast, &with(1, 6)), "only in an 8 s clip");
        let mut webp = with(1, 8);
        webp.references[0].1 = ImageFormat::Webp;
        assert!(refused(VeoModel::Fast, &webp));
        let mut heavy = with(3, 8);
        heavy.first_frame.0 = vec![0; MAX_FRAME_BYTES as usize];
        heavy.references[0].0 = vec![0; MAX_FRAME_BYTES as usize];
        assert!(refused(VeoModel::Fast, &heavy), "the images together");
        let mut square = request(8, false);
        square.aspect_ratio = AspectRatio::new(1, 1).unwrap();
        assert!(refused(VeoModel::Lite, &square));
        assert!(recorded.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn what_veo_cannot_take_is_refused_before_anything_is_sent() {
        let recorded = Recorded::default();
        let veo = client(&recorded, VeoModel::Lite);
        // A last frame needs 8 s; Veo answers 400 otherwise.
        assert!(matches!(
            veo.submit(&request(4, true)),
            Err(VeoError::InvalidRequest(_))
        ));
        let mut tall = request(4, false);
        tall.lines = 1080;
        assert!(matches!(
            veo.submit(&tall),
            Err(VeoError::InvalidRequest(_))
        ));
        let mut webp = request(8, false);
        webp.first_frame.1 = ImageFormat::Webp;
        assert!(matches!(
            veo.submit(&webp),
            Err(VeoError::InvalidRequest(_))
        ));
        let mut huge = request(8, false);
        huge.first_frame.0 = vec![0; MAX_FRAME_BYTES as usize + 1];
        assert!(matches!(
            veo.submit(&huge),
            Err(VeoError::InvalidRequest(_))
        ));
        let mut empty = request(8, false);
        empty.prompt = " ".to_owned();
        assert!(matches!(
            veo.submit(&empty),
            Err(VeoError::InvalidRequest(_))
        ));
        // Neither an operation name from elsewhere nor a video off the API.
        assert!(matches!(
            veo.operation("../../files/x"),
            Err(VeoError::InvalidRequest(_))
        ));
        assert!(matches!(
            veo.download("https://example.com/video.mp4"),
            Err(VeoError::Malformed(_))
        ));
        assert!(recorded.requests.lock().unwrap().is_empty());
        for error in [
            VeoError::InvalidRequest("x"),
            VeoError::Transport(TransportFailure::Connect),
        ] {
            assert!(error.certainly_not_accepted());
        }
    }

    #[test]
    fn a_refusal_is_not_charged_and_an_unclear_answer_may_have_been() {
        let refusal = r#"{"error":{"code":400,"message":"Your use case is currently not supported.","status":"INVALID_ARGUMENT"}}"#;
        let recorded = Recorded::answering(vec![
            Ok(HttpResponse {
                status: 400,
                body: refusal.as_bytes().to_vec(),
            }),
            Ok(HttpResponse {
                status: 429,
                body: Vec::new(),
            }),
            Ok(HttpResponse {
                status: 503,
                body: Vec::new(),
            }),
            Err(TransportFailure::Timeout),
            ok("{}"),
        ]);
        let veo = client(&recorded, VeoModel::Lite);
        let refused = veo.submit(&request(8, false)).unwrap_err();
        assert_eq!(
            refused,
            VeoError::Http {
                status: 400,
                message: "Your use case is currently not supported.".to_owned()
            }
        );
        assert!(refused.certainly_not_accepted());
        assert_eq!(refused.class(), ErrorClass::Terminal);
        let busy = veo.submit(&request(8, false)).unwrap_err();
        assert!(busy.certainly_not_accepted());
        assert_eq!(busy.class(), ErrorClass::Retryable);
        for unclear in [
            veo.submit(&request(8, false)).unwrap_err(),
            veo.submit(&request(8, false)).unwrap_err(),
            veo.submit(&request(8, false)).unwrap_err(),
        ] {
            assert!(!unclear.certainly_not_accepted(), "{unclear}");
        }
    }

    #[test]
    fn a_failed_filtered_or_expired_video_is_final() {
        let recorded = Recorded::answering(vec![
            ok(
                r#"{"done":true,"error":{"code":14,"message":"The service is currently unavailable due to high demand."}}"#,
            ),
            ok(
                r#"{"done":true,"response":{"generateVideoResponse":{"raiMediaFilteredCount":1,"raiMediaFilteredReasons":["The video could not be made safely."]}}}"#,
            ),
            Ok(HttpResponse {
                status: 404,
                body: Vec::new(),
            }),
            Ok(HttpResponse {
                status: 503,
                body: Vec::new(),
            }),
            Ok(HttpResponse {
                status: 200,
                body: b"<html>".to_vec(),
            }),
        ]);
        let veo = client(&recorded, VeoModel::Lite);
        let busy = veo.operation(OPERATION).unwrap_err();
        assert_eq!(busy.class(), ErrorClass::Terminal);
        assert_eq!(busy.ended_class(), ErrorClass::Retryable);
        assert!(busy.to_string().contains("ask again"), "{busy}");
        let withheld = veo.operation(OPERATION).unwrap_err();
        assert!(matches!(
            &withheld,
            VeoError::Filtered(reason) if reason == "The video could not be made safely."
        ));
        assert_eq!(
            (withheld.class(), withheld.ended_class()),
            (ErrorClass::Terminal, ErrorClass::Retryable)
        );
        let expired = veo.download(VIDEO).unwrap_err();
        assert_eq!(expired, VeoError::Download { status: 404 });
        assert_eq!(expired.class(), ErrorClass::Terminal);
        assert_eq!(expired.ended_class(), ErrorClass::Terminal);
        assert_eq!(
            veo.download(VIDEO).unwrap_err().class(),
            ErrorClass::Retryable
        );
        assert!(matches!(veo.download(VIDEO), Err(VeoError::Malformed(_))));
    }

    #[test]
    fn hosts_are_compared_by_scheme_and_host() {
        assert!(same_host(VIDEO, DEFAULT_BASE_URL));
        assert!(!same_host(
            "https://generativelanguage.googleapis.com.example.com/x",
            DEFAULT_BASE_URL
        ));
        assert!(!same_host(
            "http://generativelanguage.googleapis.com/x",
            DEFAULT_BASE_URL
        ));
        assert_eq!(origin("https://a.example/b?c"), Some("https://a.example"));
    }
}
