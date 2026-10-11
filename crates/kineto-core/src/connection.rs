//! The services the director added by the protocol they speak (ADR-0009),
//! as kineto asks them for images and clips. A text connection writes
//! documents ([`crate::llm::DocumentModel::connection`]); an image
//! connection plays a plugin's part
//! ([`crate::plugin::PluginProvider::connection`]), so its candidates are
//! recorded, recovered and refused as a plugin's are; a video connection is
//! a [`VideoGenerator`], whose clips are submitted, followed and recovered
//! as every video provider's are.

use std::{collections::BTreeSet, fmt, sync::Arc};

use image::{ImageFormat as Encoding, imageops};
use kineto_jobs::{ErrorClass, ProviderJobId};
use kineto_llm::openai::connection_provider;
use kineto_plugins::settings::{Connection, Modality, Protocol, micros};
use kineto_project::ContentHash;
use kineto_provider_ark::{ArkError, ArkVideos, ClipRequest};
use kineto_provider_gemini::image::GeminiImages;
use kineto_provider_openai::ImagesClient;
use kineto_providers::{
    AspectRatio, CameraControl, ImageFormat, Price, VideoCapabilities, VideoContainer, VideoMode,
    secret::{SecretSource, SecretString},
};
use serde_json::{Map, Value};

use crate::{
    generation::ProviderSelectionError,
    video::{TaskState, VideoGenerator, VideoSubmission},
};

/// How far an image's shape may be from the frame asked for before it is
/// cut to it: 2 %.
const FRAME_TOLERANCE: f64 = 0.02;

/// An image connection's client, by the protocol it speaks.
pub(crate) enum ImageConnection {
    OpenAiImages(ImagesClient),
    GeminiImages(GeminiImages),
}

impl fmt::Debug for ImageConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAiImages(client) => client.fmt(formatter),
            Self::GeminiImages(client) => client.fmt(formatter),
        }
    }
}

/// Why an image connection made no image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ConnectionFailure {
    pub class: ErrorClass,
    /// The request certainly made nothing to pay for twice: it never
    /// reached the service, or the service refused it.
    pub refused: bool,
    pub message: String,
}

impl ImageConnection {
    /// The client of `connection`, which speaks `protocol`, with its key.
    /// `None` when the protocol needs a key and there is none.
    pub(crate) fn new(
        connection: &Connection,
        protocol: Protocol,
        key: Option<SecretString>,
    ) -> Option<Self> {
        let request = request_fields(connection);
        match protocol {
            Protocol::OpenAiImages => Some(Self::OpenAiImages(ImagesClient::new(
                &connection.url,
                &connection.model,
                key,
                request,
            ))),
            Protocol::GeminiImages => key.map(|key| {
                Self::GeminiImages(GeminiImages::new(
                    &connection.url,
                    &connection.model,
                    key,
                    request,
                ))
            }),
            Protocol::OpenAiChat | Protocol::AnthropicMessages | Protocol::ArkVideo => None,
        }
    }

    /// One image of `prompt` in the frame `width:height`, following
    /// `references`, cut to that frame when the service drew another.
    pub(crate) fn generate(
        &self,
        prompt: &str,
        frame: (u16, u16),
        references: &[&[u8]],
    ) -> Result<Vec<u8>, ConnectionFailure> {
        let image = match self {
            // Its sizes only approach the frame; the words carry it.
            Self::OpenAiImages(client) => client
                .generate(&framed_prompt(prompt, frame), frame, references)
                .map_err(|error| ConnectionFailure {
                    class: error.class(),
                    refused: error.certainly_not_accepted(),
                    message: error.to_string(),
                }),
            Self::GeminiImages(client) => {
                client
                    .generate(prompt, frame, references)
                    .map_err(|error| ConnectionFailure {
                        class: error.class(),
                        refused: error.certainly_not_accepted(),
                        message: error.to_string(),
                    })
            }
        }?;
        Ok(cut_to_frame(image, frame))
    }
}

/// The height a video connection's clips are asked for in, unless the
/// director's request fields name another.
const VIDEO_LINES: u16 = 720;
/// The frame rate Seedance renders at.
const VIDEO_FPS: u16 = 24;
/// Largest frame sent, before it is written as a data URL.
const MAX_VIDEO_FRAME_BYTES: u64 = 10 * 1024 * 1024;

/// A video connection: the service the director added that makes a shot's
/// clips. Without its key it is described, with its price, and makes
/// nothing new; clips asked of it earlier wait for a session with the key.
pub struct VideoConnection {
    /// `connection.<name>`, which its jobs are recorded under.
    provider: String,
    model: String,
    capabilities: VideoCapabilities,
    /// The fields the director sends with every request, as they enter a
    /// clip's input hash.
    request_hash: String,
    client: Option<ArkVideos>,
}

impl fmt::Debug for VideoConnection {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("VideoConnection")
            .field("provider", &self.provider)
            .field("model", &self.model)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl VideoConnection {
    /// `connection` as a video provider, with the key it names from
    /// `secrets`. One that makes no clips, or that this build cannot use,
    /// is refused.
    pub fn new(
        connection: &Connection,
        secrets: &impl SecretSource,
    ) -> Result<Self, ProviderSelectionError> {
        let protocol = connection
            .check()
            .map_err(|error| ProviderSelectionError::Settings(error.to_string()))?;
        if protocol.modality() != Modality::Video {
            return Err(ProviderSelectionError::Settings(format!(
                "{} makes no clips",
                connection.name
            )));
        }
        let key = match connection.key_name() {
            Some(name) => secrets
                .api_key(name)
                .map_err(ProviderSelectionError::Secret)?,
            None => None,
        };
        let request = request_fields(connection);
        let request_hash = ContentHash::of(
            serde_json::to_string(&request)
                .unwrap_or_default()
                .as_bytes(),
        )
        .as_str()
        .to_owned();
        let capabilities = video_capabilities(connection, &request);
        // Ark's video tasks are the one video protocol.
        let client =
            key.map(|key| ArkVideos::new(&connection.url, &connection.model, key, request));
        Ok(Self {
            provider: connection_provider(&connection.name),
            model: connection.model.clone(),
            capabilities,
            request_hash,
            client,
        })
    }

    /// The key its clips are recorded under.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.provider
    }

    #[must_use]
    pub fn capabilities(&self) -> &VideoCapabilities {
        &self.capabilities
    }

    /// Whether it can be asked for a clip: its key is at hand.
    #[must_use]
    pub fn callable(&self) -> bool {
        self.client.is_some()
    }

    fn client(&self) -> Result<&ArkVideos, ArkError> {
        self.client.as_ref().ok_or(ArkError::InvalidRequest(
            "no key is kept for this connection",
        ))
    }
}

/// What a video connection renders: a shot's clip from its keyframe, and
/// the next shot's to close on, 4 to 15 seconds long, in one of the
/// protocol's frame shapes, at the height its request fields name, priced
/// by the second when the director gave a price.
fn video_capabilities(connection: &Connection, request: &Map<String, Value>) -> VideoCapabilities {
    let lines = request
        .get("resolution")
        .and_then(Value::as_str)
        .and_then(|resolution| resolution.strip_suffix('p'))
        .and_then(|lines| lines.parse::<u16>().ok())
        .unwrap_or(VIDEO_LINES);
    let audio = request.get("generate_audio") != Some(&Value::Bool(false));
    let seconds: BTreeSet<u8> = kineto_provider_ark::CLIP_SECONDS.collect();
    let clip_prices = connection
        .price_per_second
        .map(|usd| {
            seconds
                .iter()
                .map(|&seconds| {
                    (
                        kineto_providers::VideoClip { seconds, lines },
                        Price::usd(micros(usd) * u64::from(seconds)),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let frames: BTreeSet<ImageFormat> = [ImageFormat::Jpeg, ImageFormat::Png, ImageFormat::Webp]
        .into_iter()
        .collect();
    VideoCapabilities {
        modes: [VideoMode::ImageToVideo].into_iter().collect(),
        min_duration_ms: Some(u64::from(*kineto_provider_ark::CLIP_SECONDS.start()) * 1000),
        max_duration_ms: Some(u64::from(*kineto_provider_ark::CLIP_SECONDS.end()) * 1000),
        fps_options: [VIDEO_FPS].into_iter().collect(),
        start_frame: true,
        end_frame: true,
        camera_control: CameraControl::Textual,
        async_jobs: true,
        resumable_remote_handle: true,
        output_containers: [VideoContainer::Mp4].into_iter().collect(),
        aspect_ratios: kineto_provider_ark::ASPECT_RATIOS
            .into_iter()
            .filter_map(|(width, height)| AspectRatio::new(width, height))
            .collect(),
        clip_seconds: seconds,
        max_prompt_chars: Some(kineto_provider_ark::MAX_PROMPT_CHARS),
        start_frame_formats: frames.clone(),
        max_start_frame_bytes: Some(MAX_VIDEO_FRAME_BYTES),
        end_frame_formats: frames,
        max_end_frame_bytes: Some(MAX_VIDEO_FRAME_BYTES),
        audio,
        resolutions: [lines].into_iter().collect(),
        clip_prices,
        ..VideoCapabilities::default()
    }
}

impl VideoGenerator for Arc<VideoConnection> {
    type Error = ArkError;

    fn provider(&self) -> &str {
        &self.provider
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn capabilities(&self) -> VideoCapabilities {
        self.capabilities.clone()
    }

    /// Its length, height and frame shape, and the fields the director
    /// sends with every request.
    fn canonical_params(&self, request: &VideoSubmission) -> String {
        format!(
            "duration={};resolution={}p;aspect={}:{};request={}",
            request.clip_seconds,
            request.lines.unwrap_or(VIDEO_LINES),
            request.aspect_ratio.width,
            request.aspect_ratio.height,
            self.request_hash
        )
    }

    fn submit(&self, request: &VideoSubmission) -> Result<String, ArkError> {
        let client = self.client()?;
        let Some((frame, format)) = &request.first_frame else {
            return Err(ArkError::InvalidRequest("a clip starts from its keyframe"));
        };
        if !request.references.is_empty() {
            return Err(ArkError::InvalidRequest(
                "a video connection takes no reference images",
            ));
        }
        client.submit(&ClipRequest {
            prompt: &request.prompt,
            first_frame: (frame, *format),
            last_frame: request
                .last_frame
                .as_ref()
                .map(|(bytes, format)| (bytes.as_slice(), *format)),
            seconds: request.clip_seconds,
            aspect_ratio: request.aspect_ratio,
            lines: request.lines.unwrap_or(VIDEO_LINES),
        })
    }

    fn query(&self, task: &ProviderJobId) -> Result<TaskState, ArkError> {
        Ok(match self.client()?.task(task.as_str())? {
            kineto_provider_ark::TaskState::Running => TaskState::Pending,
            kineto_provider_ark::TaskState::Succeeded { video_url } => {
                TaskState::Succeeded { file: video_url }
            }
        })
    }

    fn fetch(&self, file: &str) -> Result<Vec<u8>, ArkError> {
        self.client()?.download(file)
    }

    fn class(&self, error: &ArkError) -> ErrorClass {
        error.class()
    }

    fn ended_class(&self, error: &ArkError) -> ErrorClass {
        error.ended_class()
    }

    fn certainly_not_accepted(&self, error: &ArkError) -> bool {
        error.certainly_not_accepted()
    }
}

/// The fields the director wrote for every request, as JSON.
fn request_fields(connection: &Connection) -> Map<String, Value> {
    match serde_json::to_value(&connection.request) {
        Ok(Value::Object(fields)) => fields,
        _ => Map::new(),
    }
}

/// `prompt` with the frame it is for in words.
fn framed_prompt(prompt: &str, (width, height): (u16, u16)) -> String {
    let orientation = match width.cmp(&height) {
        std::cmp::Ordering::Greater => "landscape",
        std::cmp::Ordering::Less => "portrait",
        std::cmp::Ordering::Equal => "square",
    };
    format!(
        "{prompt}\n\nFrame: {width}:{height} {orientation}, composed for exactly this aspect ratio."
    )
}

/// `bytes` cut from their middle to the frame `width:height`, as PNG, when
/// their shape is further from it than [`FRAME_TOLERANCE`]; as they are
/// otherwise, or when they cannot be read here.
pub(crate) fn cut_to_frame(bytes: Vec<u8>, (width, height): (u16, u16)) -> Vec<u8> {
    let Ok(image) = image::load_from_memory(&bytes) else {
        return bytes;
    };
    let (have_width, have_height) = (image.width(), image.height());
    if have_width == 0 || have_height == 0 || width == 0 || height == 0 {
        return bytes;
    }
    let wanted = f64::from(width) / f64::from(height);
    let have = f64::from(have_width) / f64::from(have_height);
    if ((have - wanted) / wanted).abs() <= FRAME_TOLERANCE {
        return bytes;
    }
    // Keep the whole of the side that already fits.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (cut_width, cut_height) = if have > wanted {
        (
            ((f64::from(have_height) * wanted).round() as u32).clamp(1, have_width),
            have_height,
        )
    } else {
        (
            have_width,
            ((f64::from(have_width) / wanted).round() as u32).clamp(1, have_height),
        )
    };
    let cut = imageops::crop_imm(
        &image,
        (have_width - cut_width) / 2,
        (have_height - cut_height) / 2,
        cut_width,
        cut_height,
    )
    .to_image();
    let mut png = std::io::Cursor::new(Vec::new());
    match cut.write_to(&mut png, Encoding::Png) {
        Ok(()) => png.into_inner(),
        Err(_) => bytes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{GenericImageView, Rgba, RgbaImage};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut image = RgbaImage::new(width, height);
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            // A left half red and a right half blue, to see where a cut fell.
            *pixel = if x < width / 2 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 255])
            };
            let _ = y;
        }
        let mut bytes = std::io::Cursor::new(Vec::new());
        image.write_to(&mut bytes, Encoding::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn an_image_of_another_shape_is_cut_to_the_frame_from_its_middle() {
        let cut = cut_to_frame(png(1536, 1024), (16, 9));
        let image = image::load_from_memory(&cut).unwrap();
        assert_eq!(image.dimensions(), (1536, 864));

        let cut = cut_to_frame(png(1024, 1024), (9, 16));
        let image = image::load_from_memory(&cut).unwrap();
        assert_eq!(image.dimensions(), (576, 1024));
        // The middle kept: red on the left, blue on the right.
        assert_eq!(image.get_pixel(10, 500), Rgba([255, 0, 0, 255]));
        assert_eq!(image.get_pixel(565, 500), Rgba([0, 0, 255, 255]));
    }

    #[test]
    fn an_image_in_its_frame_is_kept_as_it_came() {
        let exact = png(1344, 768);
        assert_eq!(cut_to_frame(exact.clone(), (16, 9)), exact, "within 2 %");
        let unreadable = b"\x89PNG\r\n\x1a\nnot really".to_vec();
        assert_eq!(cut_to_frame(unreadable.clone(), (16, 9)), unreadable);
    }

    #[test]
    fn the_frame_is_said_in_words_for_sizes_that_only_approach_it() {
        assert_eq!(
            framed_prompt("A door.", (16, 9)),
            "A door.\n\nFrame: 16:9 landscape, composed for exactly this aspect ratio."
        );
        assert!(framed_prompt("A door.", (1, 1)).contains("1:1 square"));
    }

    #[test]
    fn a_connection_needs_the_key_its_protocol_takes() {
        let gemini = Connection::new(
            "nano",
            Protocol::GeminiImages,
            "https://generativelanguage.example/v1beta",
            "gemini-image",
        );
        assert!(ImageConnection::new(&gemini, Protocol::GeminiImages, None).is_none());
        let key = SecretString::new("AIza-test".to_owned());
        assert!(ImageConnection::new(&gemini, Protocol::GeminiImages, key).is_some());
        let local = Connection::new(
            "comfy",
            Protocol::OpenAiImages,
            "http://localhost:8188/v1",
            "flux",
        );
        assert!(
            ImageConnection::new(&local, Protocol::OpenAiImages, None).is_some(),
            "a local server takes no key"
        );
    }
}
