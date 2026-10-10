//! The services the director added by the protocol they speak (ADR-0009),
//! as kineto asks them for images. A text connection writes documents
//! ([`crate::llm::DocumentModel::connection`]); an image connection plays a
//! plugin's part ([`crate::plugin::PluginProvider::connection`]), so its
//! candidates are recorded, recovered and refused as a plugin's are.

use std::fmt;

use image::{ImageFormat as Encoding, imageops};
use kineto_jobs::ErrorClass;
use kineto_plugins::settings::{Connection, Protocol};
use kineto_provider_gemini::image::GeminiImages;
use kineto_provider_openai::ImagesClient;
use kineto_providers::secret::SecretString;
use serde_json::{Map, Value};

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
            Protocol::OpenAiChat | Protocol::AnthropicMessages => None,
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
