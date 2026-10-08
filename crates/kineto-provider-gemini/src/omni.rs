//! Gemini Omni Flash video over the Gemini API's Interactions API.
//!
//! A clip starts from an image and may close on another. The paid step is
//! one interaction made in the background (`POST interactions`, with
//! `background` and `store`), which is answered at once with its ID. The
//! interaction is asked about (`GET interactions/<id>`, free) until it is
//! completed, when it holds the video inline, base64 encoded, or failed.
//! Google keeps an interaction for 55 days on the paid tier.
//!
//! Omni takes neither a length nor frame roles as parameters. The text sent
//! binds the images with Omni's tags (`<FIRST_FRAME>`, `<LAST_FRAME>`) and
//! says how long the clip lasts, both from the request ([`text`]), so the
//! clip's parameters cover them. No `task` is sent: Google advises
//! prompting first, and in a live trial a two-frame request without it
//! began on its first frame (SSIM 0.96) and ended near its last (0.84).
//!
//! Declared from Google's documentation, price list and live trials on
//! 2026-10-08: clips of the length asked for (4 and 6 s were tried), 720p
//! at 24 fps, 16:9 or 9:16, JPEG or PNG frames, and an AAC sound track Omni
//! makes itself. It is billed by output tokens, 5,792 for a second of 720p
//! video at $17.50 a million, about $0.10 a second. Taller clips are
//! upscaled at a price Google does not state, so only 720p is declared.
//! Omni takes reference images (`<IMAGE_REF_N>`) without a documented
//! limit; none are declared or sent.

use std::{collections::BTreeSet, error::Error, fmt, sync::Arc};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use kineto_jobs::ErrorClass;
use kineto_providers::{
    AspectRatio, CameraControl, ImageFormat, Price, VideoCapabilities, VideoClip, VideoContainer,
    VideoMode, secret::SecretString,
};
use serde::{Deserialize, Serialize};

use crate::{
    DEFAULT_BASE_URL, HttpResponse, HttpsTransport, SharedTransport, Transport, TransportFailure,
    clipped,
};

/// The name its key is read under: one Gemini API key, as in
/// `KINETO_GEMINI_API_KEY`.
pub const PROVIDER: &str = "gemini";
pub const MODEL: &str = "gemini-omni-1.1-flash";
/// Shortest and longest clip asked for, in seconds.
pub const MIN_SECONDS: u8 = 4;
pub const MAX_SECONDS: u8 = 10;
/// The only clip height declared.
pub const LINES: u16 = 720;
/// Video output tokens one second of 720p takes, as Google bills them.
pub const TOKENS_PER_SECOND: u64 = 5_792;
/// What a million video output tokens cost, in USD micros.
pub const MICROS_PER_MILLION_TOKENS: u64 = 17_500_000;
/// Largest first or last frame sent; a request carries both inline.
pub const MAX_FRAME_BYTES: u64 = 7 * 1024 * 1024;
/// Longest prompt sent, before the tags and length are added.
pub const MAX_PROMPT_CHARS: usize = 4_000;
/// Largest video taken from a finished interaction.
pub const MAX_VIDEO_BYTES: u64 = 256 * 1024 * 1024;
/// The frame shapes Omni renders.
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

/// Largest interaction read. It carries the frames it was given and, once
/// completed, the video, all base64 encoded.
const MAX_INTERACTION_BYTES: u64 = 128 * 1024 * 1024;
const FPS: u16 = 24;
/// Omni's media tags. A prompt that names one would bind images other than
/// the request does.
const TAGS: [&str; 6] = [
    "<FIRST_FRAME",
    "<LAST_FRAME",
    "<IMAGE_REF",
    "<VIDEO_REF",
    "<PREVIOUS_VIDEO",
    "[# ",
];

/// What a clip of `seconds` at 720p costs, in USD micros, for its video
/// tokens.
#[must_use]
pub const fn clip_micros(seconds: u8) -> u64 {
    (TOKENS_PER_SECOND * seconds as u64 * MICROS_PER_MILLION_TOKENS).div_ceil(1_000_000)
}

/// What Omni can do, as documented and tried: 4 to 10 s at 720p, 24 fps,
/// 16:9 or 9:16, a first frame and a last one, camera motion in words, its
/// own sound.
#[must_use]
pub fn capabilities() -> VideoCapabilities {
    let clip_seconds: BTreeSet<u8> = (MIN_SECONDS..=MAX_SECONDS).collect();
    let clip = |seconds| VideoClip {
        seconds,
        lines: LINES,
    };
    let frames: BTreeSet<ImageFormat> = [ImageFormat::Jpeg, ImageFormat::Png].into();
    VideoCapabilities {
        modes: [VideoMode::ImageToVideo].into(),
        min_duration_ms: Some(u64::from(MIN_SECONDS) * 1000),
        max_duration_ms: Some(u64::from(MAX_SECONDS) * 1000),
        fps_options: [FPS].into(),
        start_frame: true,
        end_frame: true,
        camera_control: CameraControl::Textual,
        async_jobs: true,
        resumable_remote_handle: true,
        output_containers: [VideoContainer::Mp4].into(),
        aspect_ratios: ASPECT_RATIOS.into(),
        clips: clip_seconds.iter().map(|seconds| clip(*seconds)).collect(),
        clip_prices: clip_seconds
            .iter()
            .map(|seconds| (clip(*seconds), Price::usd(clip_micros(*seconds))))
            .collect(),
        clip_seconds,
        max_prompt_chars: Some(MAX_PROMPT_CHARS),
        start_frame_formats: frames.clone(),
        max_start_frame_bytes: Some(MAX_FRAME_BYTES),
        end_frame_formats: frames,
        max_end_frame_bytes: Some(MAX_FRAME_BYTES),
        audio: true,
        resolutions: [LINES].into(),
        ..VideoCapabilities::default()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OmniError {
    /// The request did not complete; it may or may not have reached Omni.
    Transport(TransportFailure),
    /// The API answered with an HTTP error status, and its message.
    Http { status: u16, message: String },
    /// The answer could not be read as the response it should be.
    Malformed(&'static str),
    /// The request was refused before it was sent.
    InvalidRequest(&'static str),
    /// The interaction failed or was cancelled, with what it said.
    Failed(String),
    /// The interaction completed without a video, with what Omni said
    /// instead.
    Withheld(String),
}

impl OmniError {
    /// Whether trying the same request again can succeed. A finished
    /// interaction answers the same way however often it is asked, so its
    /// failure is final; asking for the video again starts another.
    #[must_use]
    pub fn class(&self) -> ErrorClass {
        match self {
            Self::Transport(_) => ErrorClass::Retryable,
            Self::Http { status, .. } if *status == 429 || *status >= 500 => ErrorClass::Retryable,
            Self::Http { .. }
            | Self::Malformed(_)
            | Self::InvalidRequest(_)
            | Self::Failed(_)
            | Self::Withheld(_) => ErrorClass::Terminal,
        }
    }

    /// How an interaction that ended with this is to be read once it is
    /// final: retryable where asking for the video again may well work.
    #[must_use]
    pub fn ended_class(&self) -> ErrorClass {
        match self {
            Self::Failed(_) | Self::Withheld(_) => ErrorClass::Retryable,
            other => other.class(),
        }
    }

    /// Whether Omni certainly did not accept a submission: it was never
    /// sent, no connection was made, or the API refused it. Any other
    /// failure may have started an interaction, and been charged for.
    #[must_use]
    pub fn certainly_not_accepted(&self) -> bool {
        match self {
            Self::InvalidRequest(_) | Self::Transport(TransportFailure::Connect) => true,
            Self::Http { status, .. } => (400..500).contains(status),
            Self::Transport(_) | Self::Malformed(_) | Self::Failed(_) | Self::Withheld(_) => false,
        }
    }
}

impl fmt::Display for OmniError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(failure) => {
                write!(formatter, "Gemini Omni request failed: {failure:?}")
            }
            Self::Http { status, message } if message.is_empty() => {
                write!(formatter, "Gemini Omni answered HTTP {status}")
            }
            Self::Http { status, message } => {
                write!(formatter, "Gemini Omni answered HTTP {status}: {message}")
            }
            Self::Malformed(what) => write!(formatter, "Gemini Omni's answer is malformed: {what}"),
            Self::InvalidRequest(what) => {
                write!(formatter, "Gemini Omni request is invalid: {what}")
            }
            Self::Failed(message) => write!(
                formatter,
                "Gemini Omni could not make the video: {message}. Asking again may work"
            ),
            Self::Withheld(message) => write!(
                formatter,
                "Gemini Omni made no video: {message}. Asking again may work"
            ),
        }
    }
}

impl Error for OmniError {}

/// One video asked of Omni.
#[derive(Clone, PartialEq, Eq)]
pub struct OmniRequest {
    pub prompt: String,
    pub first_frame: (Vec<u8>, ImageFormat),
    pub last_frame: Option<(Vec<u8>, ImageFormat)>,
    pub seconds: u8,
    /// The clip's frame shape: 16:9 or 9:16.
    pub aspect_ratio: AspectRatio,
}

impl fmt::Debug for OmniRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OmniRequest")
            .field("prompt", &self.prompt)
            .field("first_frame", &self.first_frame.0.len())
            .field(
                "last_frame",
                &self.last_frame.as_ref().map(|(bytes, _)| bytes.len()),
            )
            .field("seconds", &self.seconds)
            .field("aspect_ratio", &self.aspect_ratio)
            .finish()
    }
}

/// The text `request` is sent with: the frames bound by Omni's tags, the
/// prompt, and how long the clip lasts.
#[must_use]
pub fn text(request: &OmniRequest) -> String {
    let prompt = request.prompt.trim();
    match request.last_frame {
        None => format!(
            "<FIRST_FRAME> {prompt} The shot lasts {} seconds.",
            request.seconds
        ),
        Some(_) => format!(
            "<FIRST_FRAME> <LAST_FRAME> {prompt} The shot lasts {} seconds and ends exactly on \
             the last frame.",
            request.seconds
        ),
    }
}

/// Where an interaction is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionState {
    Running,
    /// Completed with a video, which [`OmniClient::video`] takes.
    Completed,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    model: &'static str,
    input: Vec<WireInput<'a>>,
    response_format: WireFormat,
    background: bool,
    store: bool,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireInput<'a> {
    Image {
        data: String,
        mime_type: &'static str,
    },
    Text {
        text: &'a str,
    },
}

#[derive(Serialize)]
struct WireFormat {
    #[serde(rename = "type")]
    kind: &'static str,
    aspect_ratio: &'static str,
    resolution: &'static str,
}

#[derive(Deserialize)]
struct WireCreated {
    #[serde(default)]
    id: Option<String>,
}

#[derive(Deserialize)]
struct WireInteraction {
    #[serde(default)]
    status: String,
    #[serde(default)]
    steps: Vec<WireStep>,
    #[serde(default)]
    error: Option<WireMessage>,
}

#[derive(Deserialize)]
struct WireStep {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    content: Vec<WireContent>,
}

#[derive(Deserialize)]
struct WireContent {
    #[serde(rename = "type", default)]
    kind: String,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize)]
struct WireMessage {
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct WireError {
    error: WireMessage,
}

/// What a finished interaction holds.
enum Finished {
    Video(String),
    Running,
}

/// A Gemini Omni client for one API key.
#[derive(Clone)]
pub struct OmniClient<T = HttpsTransport> {
    transport: T,
    api_key: SecretString,
    base_url: String,
}

impl OmniClient<SharedTransport> {
    /// A client over HTTPS to the Gemini API, shareable between threads.
    #[must_use]
    pub fn shared(api_key: SecretString) -> Self {
        Self::with_transport(Arc::new(HttpsTransport::new()), api_key, DEFAULT_BASE_URL)
    }
}

impl<T> fmt::Debug for OmniClient<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("OmniClient")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}

impl<T: Transport> OmniClient<T> {
    pub fn with_transport(transport: T, api_key: SecretString, base_url: &str) -> Self {
        Self {
            transport,
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
        }
    }

    /// Start making `request`'s video: the paid call. Returns the
    /// interaction to follow.
    pub fn submit(&self, request: &OmniRequest) -> Result<String, OmniError> {
        let capabilities = capabilities();
        let prompt = request.prompt.trim();
        if prompt.is_empty() || prompt.chars().count() > MAX_PROMPT_CHARS {
            return Err(OmniError::InvalidRequest(
                "the prompt is empty or longer than Omni is sent",
            ));
        }
        if TAGS.iter().any(|tag| prompt.contains(tag)) {
            return Err(OmniError::InvalidRequest(
                "the prompt names one of Omni's media tags, which would bind other images",
            ));
        }
        if !capabilities.clip_seconds.contains(&request.seconds) {
            return Err(OmniError::InvalidRequest(
                "Omni is asked for clips of 4 to 10 s",
            ));
        }
        let aspect_ratio = match (request.aspect_ratio.width, request.aspect_ratio.height) {
            (16, 9) => "16:9",
            (9, 16) => "9:16",
            _ => return Err(OmniError::InvalidRequest("Omni frames a clip 16:9 or 9:16")),
        };
        let image = |(bytes, format): &(Vec<u8>, ImageFormat)| {
            let mime_type = match format {
                ImageFormat::Jpeg => "image/jpeg",
                ImageFormat::Png => "image/png",
                ImageFormat::Webp => {
                    return Err(OmniError::InvalidRequest("Omni is sent JPEG or PNG frames"));
                }
            };
            if bytes.is_empty() || bytes.len() as u64 > MAX_FRAME_BYTES {
                return Err(OmniError::InvalidRequest(
                    "a frame is empty or larger than Omni is sent",
                ));
            }
            Ok(WireInput::Image {
                data: STANDARD.encode(bytes),
                mime_type,
            })
        };
        let text = text(request);
        let mut input = vec![image(&request.first_frame)?];
        if let Some(last) = &request.last_frame {
            input.push(image(last)?);
        }
        input.push(WireInput::Text { text: &text });
        let body = serde_json::to_vec(&WireRequest {
            model: MODEL,
            input,
            response_format: WireFormat {
                kind: "video",
                aspect_ratio,
                resolution: "720p",
            },
            background: true,
            store: true,
        })
        .map_err(|_| OmniError::InvalidRequest("the request did not encode"))?;
        let url = format!("{}/interactions", self.base_url);
        let response = self
            .transport
            .post_json(&url, &self.api_key, &body)
            .map_err(OmniError::Transport)?;
        let created: WireCreated = serde_json::from_slice(&answered(response)?)
            .map_err(|_| OmniError::Malformed("the submission's answer is not JSON"))?;
        created
            .id
            .filter(|id| is_interaction(id))
            .ok_or(OmniError::Malformed("the submission named no interaction"))
    }

    /// Where interaction `id` is. Free.
    pub fn interaction(&self, id: &str) -> Result<InteractionState, OmniError> {
        Ok(match self.finished(id)? {
            Finished::Running => InteractionState::Running,
            Finished::Video(_) => InteractionState::Completed,
        })
    }

    /// The video completed interaction `id` holds. Free.
    pub fn video(&self, id: &str) -> Result<Vec<u8>, OmniError> {
        let Finished::Video(data) = self.finished(id)? else {
            return Err(OmniError::Malformed("the interaction has not completed"));
        };
        if data.len() as u64 > MAX_VIDEO_BYTES.div_ceil(3) * 4 {
            return Err(OmniError::Malformed(
                "the video is larger than kineto takes",
            ));
        }
        let video = STANDARD
            .decode(data.as_bytes())
            .map_err(|_| OmniError::Malformed("the video is not base64"))?;
        if video.get(4..8) != Some(b"ftyp") {
            return Err(OmniError::Malformed("the video is not an MP4"));
        }
        Ok(video)
    }

    fn finished(&self, id: &str) -> Result<Finished, OmniError> {
        if !is_interaction(id) {
            return Err(OmniError::InvalidRequest("that is not an interaction ID"));
        }
        let url = format!("{}/interactions/{id}", self.base_url);
        let response = self
            .transport
            .get(&url, &self.api_key, MAX_INTERACTION_BYTES)
            .map_err(OmniError::Transport)?;
        let interaction: WireInteraction = serde_json::from_slice(&answered(response)?)
            .map_err(|_| OmniError::Malformed("the interaction is not JSON"))?;
        match interaction.status.as_str() {
            "in_progress" => Ok(Finished::Running),
            "completed" => {
                let outputs = || {
                    interaction
                        .steps
                        .iter()
                        .filter(|step| step.kind == "model_output")
                        .flat_map(|step| &step.content)
                };
                if let Some(data) = outputs()
                    .find(|content| content.kind == "video")
                    .and_then(|content| content.data.clone())
                {
                    return Ok(Finished::Video(data));
                }
                let said: Vec<&str> = outputs()
                    .filter_map(|content| content.text.as_deref())
                    .collect();
                Err(OmniError::Withheld(if said.is_empty() {
                    "it gave no reason".to_owned()
                } else {
                    clipped(&said.join(" "))
                }))
            }
            "failed" | "cancelled" | "incomplete" => Err(OmniError::Failed(
                interaction
                    .error
                    .map(|error| clipped(&error.message))
                    .filter(|message| !message.is_empty())
                    .unwrap_or_else(|| format!("the interaction is {}", interaction.status)),
            )),
            _ => Err(OmniError::Malformed(
                "the interaction is in a state kineto does not follow",
            )),
        }
    }
}

/// Whether `id` is an interaction ID as Omni gives one: nothing that leaves
/// the API's path.
fn is_interaction(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// The body of a successful answer, or the API's error.
fn answered(response: HttpResponse) -> Result<Vec<u8>, OmniError> {
    if (200..300).contains(&response.status) {
        return Ok(response.body);
    }
    let message = serde_json::from_slice::<WireError>(&response.body)
        .map(|wire| clipped(&wire.error.message))
        .unwrap_or_default();
    Err(OmniError::Http {
        status: response.status,
        message,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    const ID: &str = "v1_ChdkRExIYXVIZko4YlVqTWNQOTZpTTRBaxIXZERMSGF1SGZKOGJVak1jUDk2aU00QWs";
    const MP4: &[u8] = b"\0\0\0\x18ftypisomrest of the file";

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

        fn get(
            &self,
            url: &str,
            api_key: &SecretString,
            max_bytes: u64,
        ) -> Result<HttpResponse, TransportFailure> {
            assert_eq!(api_key.expose(), "AQ.test");
            assert_eq!(max_bytes, MAX_INTERACTION_BYTES);
            self.next("GET", url, &[])
        }

        fn download(
            &self,
            _url: &str,
            _api_key: &SecretString,
            _max_bytes: u64,
        ) -> Result<HttpResponse, TransportFailure> {
            unreachable!("Omni's video comes inline")
        }
    }

    fn ok(body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status: 200,
            body: body.as_bytes().to_vec(),
        })
    }

    /// An interaction as the API answers it, with its output steps.
    fn interaction(status: &str, output: &str) -> Result<HttpResponse, TransportFailure> {
        ok(&format!(
            r#"{{"id":"{ID}","status":"{status}","steps":[{{"type":"user_input","content":[{{"type":"image","data":"/9j/4A==","mime_type":"image/jpeg"}}]}},{{"type":"thought","signature":"x"}}{output}],"object":"interaction"}}"#
        ))
    }

    fn client(recorded: &Recorded) -> OmniClient<&Recorded> {
        OmniClient::with_transport(
            recorded,
            SecretString::new("AQ.test".to_owned()).unwrap(),
            DEFAULT_BASE_URL,
        )
    }

    fn request(seconds: u8, last_frame: bool) -> OmniRequest {
        OmniRequest {
            prompt: "A woman waits by the window. No speech".to_owned(),
            first_frame: (vec![0xff, 0xd8, 0xff, 0xe0], ImageFormat::Jpeg),
            last_frame: last_frame.then(|| (vec![0x89, b'P', b'N', b'G'], ImageFormat::Png)),
            seconds,
            aspect_ratio: ASPECT_RATIOS[0],
        }
    }

    #[test]
    fn it_declares_its_clips_and_prices_by_the_token() {
        let capabilities = capabilities();
        assert_eq!(capabilities.clip_seconds, (4..=10).collect());
        assert_eq!(capabilities.resolutions, [720].into());
        assert_eq!(capabilities.clip_lines(6), Some(720));
        // 6 s is 34,752 video tokens at $17.50 a million, as billed live.
        assert_eq!(capabilities.clip_price(6), Some(Price::usd(608_160)));
        assert_eq!(clip_micros(4), 405_440);
        assert!(capabilities.end_frame && capabilities.end_frame_seconds.is_empty());
        assert!(capabilities.audio && capabilities.async_jobs);
        assert!(capabilities.resumable_remote_handle);
        assert_eq!(capabilities.aspect_ratios, ASPECT_RATIOS.into());
        assert_eq!(capabilities.max_reference_images, 0);
    }

    #[test]
    fn a_clip_is_made_in_the_background_and_its_video_taken_inline() {
        let video = format!(
            r#",{{"type":"model_output","content":[{{"type":"video","mime_type":"video/mp4","data":"{}"}}]}}"#,
            STANDARD.encode(MP4)
        );
        let recorded = Recorded::answering(vec![
            ok(&format!(r#"{{"id":"{ID}","status":"in_progress"}}"#)),
            interaction("in_progress", ""),
            interaction("completed", &video),
            interaction("completed", &video),
        ]);
        let omni = client(&recorded);
        assert_eq!(omni.submit(&request(6, true)).unwrap(), ID);
        assert_eq!(omni.interaction(ID).unwrap(), InteractionState::Running);
        assert_eq!(omni.interaction(ID).unwrap(), InteractionState::Completed);
        assert_eq!(omni.video(ID).unwrap(), MP4);

        let requests = recorded.requests.lock().unwrap();
        assert_eq!(
            requests[0].1,
            "https://generativelanguage.googleapis.com/v1beta/interactions"
        );
        let body: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "model": "gemini-omni-1.1-flash",
                "input": [
                    {"type": "image", "data": "/9j/4A==", "mime_type": "image/jpeg"},
                    {"type": "image", "data": "iVBORw==", "mime_type": "image/png"},
                    {"type": "text", "text": "<FIRST_FRAME> <LAST_FRAME> A woman waits by the \
                        window. No speech The shot lasts 6 seconds and ends exactly on the last \
                        frame."}
                ],
                "response_format": {"type": "video", "aspect_ratio": "16:9", "resolution": "720p"},
                "background": true,
                "store": true
            })
        );
        assert_eq!(
            requests[1],
            (
                "GET".to_owned(),
                format!("https://generativelanguage.googleapis.com/v1beta/interactions/{ID}"),
                Vec::new()
            )
        );
    }

    #[test]
    fn a_tall_clip_from_one_frame_says_only_its_length() {
        let tall = OmniRequest {
            aspect_ratio: ASPECT_RATIOS[1],
            ..request(4, false)
        };
        assert_eq!(
            text(&tall),
            "<FIRST_FRAME> A woman waits by the window. No speech The shot lasts 4 seconds."
        );
        let recorded = Recorded::answering(vec![ok(&format!(r#"{{"id":"{ID}"}}"#))]);
        client(&recorded).submit(&tall).unwrap();
        let body: serde_json::Value =
            serde_json::from_slice(&recorded.requests.lock().unwrap()[0].2).unwrap();
        assert_eq!(body["response_format"]["aspect_ratio"], "9:16");
        assert_eq!(body["input"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn what_omni_is_not_sent_is_refused_before_anything_is_sent() {
        let recorded = Recorded::default();
        let omni = client(&recorded);
        let refused = |request: &OmniRequest| {
            matches!(omni.submit(request), Err(OmniError::InvalidRequest(_)))
        };
        assert!(refused(&request(3, false)));
        assert!(refused(&request(11, false)));
        let mut square = request(6, false);
        square.aspect_ratio = AspectRatio::new(1, 1).unwrap();
        assert!(refused(&square));
        let mut webp = request(6, false);
        webp.first_frame.1 = ImageFormat::Webp;
        assert!(refused(&webp));
        let mut huge = request(6, true);
        huge.last_frame = Some((vec![0; MAX_FRAME_BYTES as usize + 1], ImageFormat::Png));
        assert!(refused(&huge));
        for prompt in [
            " ",
            "<LAST_FRAME> the woman",
            "[# Sources <FIRST_FRAME>@Image2]",
        ] {
            let mut tagged = request(6, false);
            tagged.prompt = prompt.to_owned();
            assert!(refused(&tagged), "{prompt}");
        }
        // Neither an interaction from elsewhere on the API.
        for id in ["../../models", "v1_a/b", "", "v1 a"] {
            assert!(matches!(
                omni.interaction(id),
                Err(OmniError::InvalidRequest(_))
            ));
        }
        assert!(recorded.requests.lock().unwrap().is_empty());
        assert!(OmniError::InvalidRequest("x").certainly_not_accepted());
        assert!(OmniError::Transport(TransportFailure::Connect).certainly_not_accepted());
    }

    #[test]
    fn a_refusal_is_not_accepted_and_an_unclear_answer_may_have_been() {
        let refusal = r#"{"error":{"message":"Invalid model.","code":"invalid_request"}}"#;
        let recorded = Recorded::answering(vec![
            Ok(HttpResponse {
                status: 400,
                body: refusal.as_bytes().to_vec(),
            }),
            Ok(HttpResponse {
                status: 503,
                body: Vec::new(),
            }),
            Err(TransportFailure::Timeout),
            ok("{}"),
        ]);
        let omni = client(&recorded);
        let refused = omni.submit(&request(6, false)).unwrap_err();
        assert_eq!(
            refused,
            OmniError::Http {
                status: 400,
                message: "Invalid model.".to_owned()
            }
        );
        assert!(refused.certainly_not_accepted());
        assert_eq!(refused.class(), ErrorClass::Terminal);
        for unclear in [
            omni.submit(&request(6, false)).unwrap_err(),
            omni.submit(&request(6, false)).unwrap_err(),
            omni.submit(&request(6, false)).unwrap_err(),
        ] {
            assert!(!unclear.certainly_not_accepted(), "{unclear}");
        }
    }

    #[test]
    fn a_failed_or_withheld_interaction_is_final_and_worth_asking_again() {
        let recorded = Recorded::answering(vec![
            ok(&format!(
                r#"{{"id":"{ID}","status":"failed","error":{{"message":"The video could not be made safely."}}}}"#
            )),
            interaction(
                "completed",
                r#",{"type":"model_output","content":[{"type":"text","text":"I can't make that video."}]}"#,
            ),
            ok(&format!(r#"{{"id":"{ID}","status":"requires_action"}}"#)),
            interaction(
                "completed",
                r#",{"type":"model_output","content":[{"type":"video","data":"PGh0bWw+"}]}"#,
            ),
        ]);
        let omni = client(&recorded);
        let failed = omni.interaction(ID).unwrap_err();
        assert_eq!(
            failed,
            OmniError::Failed("The video could not be made safely.".to_owned())
        );
        assert_eq!(
            (failed.class(), failed.ended_class()),
            (ErrorClass::Terminal, ErrorClass::Retryable)
        );
        assert_eq!(
            omni.interaction(ID).unwrap_err(),
            OmniError::Withheld("I can't make that video.".to_owned())
        );
        assert!(matches!(omni.interaction(ID), Err(OmniError::Malformed(_))));
        assert!(matches!(omni.video(ID), Err(OmniError::Malformed(_))));
    }
}
