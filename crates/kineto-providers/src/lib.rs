pub mod secret;

use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SeedSupport {
    #[default]
    None,
    BestEffort,
    Deterministic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReferenceRole {
    Identity,
    Style,
    Composition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImageFormat {
    Png,
    Jpeg,
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct AspectRatio {
    pub width: u16,
    pub height: u16,
}

impl AspectRatio {
    #[must_use]
    pub const fn new(width: u16, height: u16) -> Option<Self> {
        if width == 0 || height == 0 {
            None
        } else {
            Some(Self { width, height })
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Resolution {
    pub width: u32,
    pub height: u32,
}

impl Resolution {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Option<Self> {
        if width == 0 || height == 0 {
            None
        } else {
            Some(Self { width, height })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LlmCapabilities {
    pub context_window_tokens: Option<u32>,
    pub tools: bool,
    pub structured_output: bool,
    pub vision_input: bool,
}

/// What something costs, in millionths of a currency unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Price {
    pub amount_micros: u64,
    /// ISO 4217 code, such as `USD`.
    pub currency: [u8; 3],
}

impl Price {
    #[must_use]
    pub const fn usd(amount_micros: u64) -> Self {
        Self {
            amount_micros,
            currency: *b"USD",
        }
    }

    #[must_use]
    pub const fn is_free(self) -> bool {
        self.amount_micros == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageCapabilities {
    pub aspect_ratios: BTreeSet<AspectRatio>,
    pub resolutions: BTreeSet<Resolution>,
    pub seed_support: SeedSupport,
    pub negative_prompt: bool,
    /// Reference images one request may carry, across every role.
    pub max_reference_images: u16,
    pub reference_roles: BTreeSet<ReferenceRole>,
    /// Formats a reference image may be sent in.
    pub reference_formats: BTreeSet<ImageFormat>,
    /// Largest reference image the provider accepts, in bytes.
    pub max_reference_bytes: Option<u64>,
    pub inpainting: bool,
    pub masks: bool,
    pub output_formats: BTreeSet<ImageFormat>,
    /// Longest prompt the provider accepts, in characters; `None` when it
    /// sets no limit.
    pub max_prompt_chars: Option<usize>,
    /// Whether an image arrives as a remote job to follow, rather than in
    /// the provider's answer to the request.
    pub async_jobs: bool,
    /// What one image costs; `None` when it is not known.
    pub price_per_image: Option<Price>,
}

impl ImageCapabilities {
    /// How many reference images of `role` one request may carry.
    #[must_use]
    pub fn reference_limit(&self, role: ReferenceRole) -> u16 {
        if self.reference_roles.contains(&role) {
            self.max_reference_images
        } else {
            0
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VideoMode {
    TextToVideo,
    ImageToVideo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum CameraControl {
    #[default]
    None,
    Textual,
    Parametric,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VideoContainer {
    Mp4,
    Webm,
    Mov,
}

/// One clip a provider renders: how long, and how tall in lines, as in 768
/// for 768P.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VideoClip {
    pub seconds: u8,
    pub lines: u16,
}

/// The shortest clip height a production asks for when a provider offers
/// one at least this tall.
pub const PRODUCTION_LINES: u16 = 720;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VideoCapabilities {
    pub modes: BTreeSet<VideoMode>,
    pub min_duration_ms: Option<u64>,
    pub max_duration_ms: Option<u64>,
    pub fps_options: BTreeSet<u16>,
    pub start_frame: bool,
    pub end_frame: bool,
    pub camera_control: CameraControl,
    pub identity_reference: bool,
    pub async_jobs: bool,
    pub resumable_remote_handle: bool,
    pub output_containers: BTreeSet<VideoContainer>,
    /// Clip lengths the provider renders, in seconds; empty when it renders
    /// any length between the minimum and maximum duration.
    pub clip_seconds: BTreeSet<u8>,
    /// Longest prompt the provider accepts, in characters; `None` when it
    /// sets no limit.
    pub max_prompt_chars: Option<usize>,
    /// Formats a start frame may be sent in.
    pub start_frame_formats: BTreeSet<ImageFormat>,
    /// Largest start frame the provider accepts, in bytes.
    pub max_start_frame_bytes: Option<u64>,
    /// Formats an end frame may be sent in, when it takes one.
    pub end_frame_formats: BTreeSet<ImageFormat>,
    /// Largest end frame the provider accepts, in bytes.
    pub max_end_frame_bytes: Option<u64>,
    /// The clip lengths an end frame can be sent with, in seconds; empty
    /// when any length it renders can take one.
    pub end_frame_seconds: BTreeSet<u8>,
    /// Whether its clips come with a sound track it makes itself.
    pub audio: bool,
    /// Clip heights it renders, in lines; empty when it does not say and
    /// chooses itself.
    pub resolutions: BTreeSet<u16>,
    /// Each length and height it renders together; empty when it renders
    /// every length it declares at every height it declares.
    pub clips: BTreeSet<VideoClip>,
    /// What one clip costs, by its length and height; a clip missing here
    /// has no known price.
    pub clip_prices: BTreeMap<VideoClip, Price>,
}

impl VideoCapabilities {
    /// The clip length closest to `seconds` that the provider renders;
    /// ties go to the shorter, cheaper clip. `None` when it declares no
    /// lengths and no range.
    #[must_use]
    pub fn nearest_clip_seconds(&self, seconds: f64) -> Option<u8> {
        if self.clip_seconds.is_empty() {
            let min = self.min_duration_ms? as f64 / 1000.0;
            let max = self.max_duration_ms? as f64 / 1000.0;
            let clamped = seconds.clamp(min, max).round();
            return u8::try_from(clamped as u64).ok();
        }
        self.clip_seconds.iter().copied().min_by(|a, b| {
            (f64::from(*a) - seconds)
                .abs()
                .total_cmp(&(f64::from(*b) - seconds).abs())
                .then(a.cmp(b))
        })
    }

    /// Whether it renders a clip of `seconds` at `lines`.
    #[must_use]
    pub fn renders(&self, clip: VideoClip) -> bool {
        if !self.clips.is_empty() {
            return self.clips.contains(&clip);
        }
        let length = if self.clip_seconds.is_empty() {
            let ms = u64::from(clip.seconds) * 1000;
            self.min_duration_ms.is_none_or(|min| ms >= min)
                && self.max_duration_ms.is_none_or(|max| ms <= max)
        } else {
            self.clip_seconds.contains(&clip.seconds)
        };
        length && (self.resolutions.is_empty() || self.resolutions.contains(&clip.lines))
    }

    /// The height a clip of `seconds` is made at: the lowest it renders at
    /// that length that is at least [`PRODUCTION_LINES`] tall, or the
    /// tallest it renders when none is. `None` when it declares no heights,
    /// or renders none at that length.
    #[must_use]
    pub fn clip_lines(&self, seconds: u8) -> Option<u16> {
        let heights: Vec<u16> = self
            .resolutions
            .iter()
            .copied()
            .filter(|lines| {
                self.renders(VideoClip {
                    seconds,
                    lines: *lines,
                })
            })
            .collect();
        heights
            .iter()
            .copied()
            .find(|lines| *lines >= PRODUCTION_LINES)
            .or_else(|| heights.last().copied())
    }

    /// What a clip of `seconds` costs at the height it is made at; `None`
    /// when that is not known.
    #[must_use]
    pub fn clip_price(&self, seconds: u8) -> Option<Price> {
        let lines = self.clip_lines(seconds)?;
        self.clip_prices.get(&VideoClip { seconds, lines }).copied()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
}

impl AudioFormat {
    /// The file extension audio of this format is stored with.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
        }
    }
}

/// Where a provider's voices come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum VoiceSource {
    /// Voices the provider lists, chosen by ID.
    Catalog,
    /// A voice made from reference audio.
    Cloned,
    /// A voice made from a written description.
    Designed,
}

/// How finely a provider says when each part of what it spoke was heard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum SpeechTiming {
    /// Only the length of the whole.
    #[default]
    None,
    /// When each sentence starts and ends.
    Sentence,
    /// When each word starts and ends.
    Word,
}

/// A delivery a line can be asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Emotion {
    Happy,
    Sad,
    Angry,
    Fearful,
    Disgusted,
    Surprised,
    Calm,
    /// Even and natural, without colour.
    Fluent,
    Whisper,
}

impl Emotion {
    pub const ALL: [Self; 9] = [
        Self::Happy,
        Self::Sad,
        Self::Angry,
        Self::Fearful,
        Self::Disgusted,
        Self::Surprised,
        Self::Calm,
        Self::Fluent,
        Self::Whisper,
    ];

    /// Its name in project records.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Happy => "happy",
            Self::Sad => "sad",
            Self::Angry => "angry",
            Self::Fearful => "fearful",
            Self::Disgusted => "disgusted",
            Self::Surprised => "surprised",
            Self::Calm => "calm",
            Self::Fluent => "fluent",
            Self::Whisper => "whisper",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|emotion| emotion.name() == name)
    }
}

/// What a text-to-speech provider declares it can do.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpeechCapabilities {
    pub voice_sources: BTreeSet<VoiceSource>,
    /// Languages it speaks, as ISO 639-1 codes such as `en`.
    pub languages: BTreeSet<String>,
    /// Deliveries it can be asked for; empty when it takes none.
    pub emotions: BTreeSet<Emotion>,
    /// Slowest and fastest speaking rate, in thousandths of its normal rate;
    /// `None` when the rate cannot be set.
    pub speed_permille: Option<(u16, u16)>,
    /// Lowest and highest pitch shift, in semitones; `None` when pitch
    /// cannot be set.
    pub pitch_semitones: Option<(i8, i8)>,
    pub timing: SpeechTiming,
    /// Whether speech arrives as a remote job to follow, rather than in the
    /// provider's answer to the request.
    pub async_jobs: bool,
    pub output_formats: BTreeSet<AudioFormat>,
    /// Longest text one request may carry, in characters; `None` when it
    /// sets no limit.
    pub max_text_chars: Option<usize>,
    /// What one character of text costs; `None` when it is not known.
    pub price_per_character: Option<Price>,
}

impl SpeechCapabilities {
    /// What speaking `characters` characters costs, in millionths of the
    /// price's currency; `None` when the price is not known.
    #[must_use]
    pub fn cost_micros(&self, characters: usize) -> Option<u64> {
        self.price_per_character.map(|price| {
            price
                .amount_micros
                .saturating_mul(u64::try_from(characters).unwrap_or(u64::MAX))
        })
    }

    /// The format speech is asked for in: compressed when the provider
    /// offers it, which keeps a project's lines small.
    #[must_use]
    pub fn preferred_format(&self) -> Option<AudioFormat> {
        [AudioFormat::Mp3, AudioFormat::Flac, AudioFormat::Wav]
            .into_iter()
            .find(|format| self.output_formats.contains(format))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapSeverity {
    Warning,
    Incompatible,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGap {
    pub capability: &'static str,
    pub severity: GapSeverity,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompatibilityReport {
    pub gaps: Vec<CapabilityGap>,
}

impl CompatibilityReport {
    #[must_use]
    pub fn is_compatible(&self) -> bool {
        !self
            .gaps
            .iter()
            .any(|gap| gap.severity == GapSeverity::Incompatible)
    }

    #[must_use]
    pub fn is_lossless(&self) -> bool {
        self.gaps.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoRequirements {
    pub mode: VideoMode,
    pub requires_start_frame: bool,
    pub requires_end_frame: bool,
    pub requires_identity_reference: bool,
    pub preferred_camera_control: CameraControl,
    /// The workflow follows a remote task that must survive a restart, so
    /// a paid video is never lost or asked for twice.
    pub requires_resumable_jobs: bool,
}

#[must_use]
pub fn evaluate_video(
    requirements: VideoRequirements,
    capabilities: &VideoCapabilities,
) -> CompatibilityReport {
    let mut gaps = Vec::new();

    if !capabilities.modes.contains(&requirements.mode) {
        gaps.push(CapabilityGap {
            capability: "video.mode",
            severity: GapSeverity::Incompatible,
        });
    }
    if requirements.requires_start_frame && !capabilities.start_frame {
        gaps.push(CapabilityGap {
            capability: "video.start_frame",
            severity: GapSeverity::Incompatible,
        });
    }
    if requirements.requires_end_frame && !capabilities.end_frame {
        gaps.push(CapabilityGap {
            capability: "video.end_frame",
            severity: GapSeverity::Incompatible,
        });
    }
    if requirements.requires_identity_reference && !capabilities.identity_reference {
        gaps.push(CapabilityGap {
            capability: "video.identity_reference",
            severity: GapSeverity::Incompatible,
        });
    }
    if capabilities.camera_control < requirements.preferred_camera_control {
        gaps.push(CapabilityGap {
            capability: "video.camera_control",
            severity: GapSeverity::Warning,
        });
    }
    if requirements.requires_resumable_jobs && !capabilities.async_jobs {
        gaps.push(CapabilityGap {
            capability: "video.async_jobs",
            severity: GapSeverity::Incompatible,
        });
    }
    if requirements.requires_resumable_jobs && !capabilities.resumable_remote_handle {
        gaps.push(CapabilityGap {
            capability: "video.resumable_remote_handle",
            severity: GapSeverity::Incompatible,
        });
    }

    CompatibilityReport { gaps }
}

/// What speaking a line with a chosen voice needs of a provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechRequirements {
    pub voice_source: VoiceSource,
    /// ISO 639-1 code of the language spoken.
    pub language: String,
    /// The delivery the voice was chosen with.
    pub emotion: Option<Emotion>,
    pub speed_permille: u16,
    pub pitch_semitones: i8,
    /// The timing the workflow would like; less is reported, not refused,
    /// because a line's length is known from its audio either way.
    pub preferred_timing: SpeechTiming,
    /// The workflow keeps the audio the provider answers with and follows
    /// no remote job.
    pub requires_synchronous: bool,
}

/// Whether a provider can speak as `requirements` ask. A voice the director
/// chose is kept as chosen: a provider that would change its language,
/// delivery, rate or pitch is incompatible, never quietly used.
#[must_use]
pub fn evaluate_speech(
    requirements: &SpeechRequirements,
    capabilities: &SpeechCapabilities,
) -> CompatibilityReport {
    let mut gaps = Vec::new();
    let mut incompatible = |capability, missing: bool| {
        if missing {
            gaps.push(CapabilityGap {
                capability,
                severity: GapSeverity::Incompatible,
            });
        }
    };
    incompatible(
        "speech.voice_source",
        !capabilities
            .voice_sources
            .contains(&requirements.voice_source),
    );
    incompatible(
        "speech.language",
        !capabilities.languages.contains(&requirements.language),
    );
    incompatible(
        "speech.emotion",
        requirements
            .emotion
            .is_some_and(|emotion| !capabilities.emotions.contains(&emotion)),
    );
    incompatible(
        "speech.speed",
        match capabilities.speed_permille {
            Some((slowest, fastest)) => !(slowest..=fastest).contains(&requirements.speed_permille),
            None => requirements.speed_permille != 1000,
        },
    );
    incompatible(
        "speech.pitch",
        match capabilities.pitch_semitones {
            Some((lowest, highest)) => !(lowest..=highest).contains(&requirements.pitch_semitones),
            None => requirements.pitch_semitones != 0,
        },
    );
    incompatible(
        "speech.async_jobs",
        requirements.requires_synchronous && capabilities.async_jobs,
    );
    if capabilities.timing < requirements.preferred_timing {
        gaps.push(CapabilityGap {
            capability: "speech.timing",
            severity: GapSeverity::Warning,
        });
    }

    CompatibilityReport { gaps }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRequirements {
    pub required_reference_role: Option<ReferenceRole>,
    pub minimum_seed_support: SeedSupport,
    pub requires_negative_prompt: bool,
    pub requires_mask: bool,
    /// The workflow keeps the image the provider answers with and follows
    /// no remote job.
    pub requires_synchronous: bool,
}

#[must_use]
pub fn evaluate_image(
    requirements: ImageRequirements,
    capabilities: &ImageCapabilities,
) -> CompatibilityReport {
    let mut gaps = Vec::new();

    if let Some(role) = requirements.required_reference_role
        && (capabilities.max_reference_images == 0 || !capabilities.reference_roles.contains(&role))
    {
        gaps.push(CapabilityGap {
            capability: "image.reference_role",
            severity: GapSeverity::Incompatible,
        });
    }
    if capabilities.seed_support < requirements.minimum_seed_support {
        gaps.push(CapabilityGap {
            capability: "image.seed",
            severity: GapSeverity::Warning,
        });
    }
    if requirements.requires_negative_prompt && !capabilities.negative_prompt {
        gaps.push(CapabilityGap {
            capability: "image.negative_prompt",
            severity: GapSeverity::Warning,
        });
    }
    if requirements.requires_mask && !capabilities.masks {
        gaps.push(CapabilityGap {
            capability: "image.mask",
            severity: GapSeverity::Incompatible,
        });
    }
    if requirements.requires_synchronous && capabilities.async_jobs {
        gaps.push(CapabilityGap {
            capability: "image.async_jobs",
            severity: GapSeverity::Incompatible,
        });
    }

    CompatibilityReport { gaps }
}

#[cfg(any(test, feature = "testing"))]
pub mod fake {
    use super::*;

    #[must_use]
    pub fn image_full() -> ImageCapabilities {
        ImageCapabilities {
            aspect_ratios: [
                AspectRatio::new(1, 1).unwrap(),
                AspectRatio::new(16, 9).unwrap(),
            ]
            .into_iter()
            .collect(),
            resolutions: [Resolution::new(1024, 1024).unwrap()].into_iter().collect(),
            seed_support: SeedSupport::Deterministic,
            negative_prompt: true,
            max_reference_images: 4,
            reference_roles: [
                ReferenceRole::Identity,
                ReferenceRole::Style,
                ReferenceRole::Composition,
            ]
            .into_iter()
            .collect(),
            reference_formats: [ImageFormat::Png, ImageFormat::Jpeg].into_iter().collect(),
            max_reference_bytes: Some(10 * 1024 * 1024),
            inpainting: true,
            masks: true,
            output_formats: [ImageFormat::Png].into_iter().collect(),
            max_prompt_chars: None,
            async_jobs: true,
            price_per_image: Some(Price::usd(40_000)),
        }
    }

    #[must_use]
    pub fn image_text_only() -> ImageCapabilities {
        ImageCapabilities {
            output_formats: [ImageFormat::Png].into_iter().collect(),
            ..ImageCapabilities::default()
        }
    }

    #[must_use]
    pub fn video_i2v_resumable() -> VideoCapabilities {
        VideoCapabilities {
            modes: [VideoMode::ImageToVideo].into_iter().collect(),
            min_duration_ms: Some(1_000),
            max_duration_ms: Some(10_000),
            fps_options: [24, 30].into_iter().collect(),
            start_frame: true,
            end_frame: false,
            camera_control: CameraControl::Textual,
            identity_reference: true,
            async_jobs: true,
            resumable_remote_handle: true,
            output_containers: [VideoContainer::Mp4].into_iter().collect(),
            clip_seconds: [5, 10].into_iter().collect(),
            max_prompt_chars: Some(2_000),
            start_frame_formats: [ImageFormat::Jpeg, ImageFormat::Png].into_iter().collect(),
            max_start_frame_bytes: Some(20 * 1024 * 1024),
            end_frame_formats: BTreeSet::new(),
            max_end_frame_bytes: None,
            resolutions: [480, 720, 1080].into_iter().collect(),
            clips: [(5, 480), (5, 720), (10, 720), (5, 1080)]
                .into_iter()
                .map(|(seconds, lines)| VideoClip { seconds, lines })
                .collect(),
            clip_prices: [
                ((5, 480), 100_000),
                ((5, 720), 250_000),
                ((10, 720), 500_000),
                ((5, 1080), 450_000),
            ]
            .into_iter()
            .map(|((seconds, lines), micros)| (VideoClip { seconds, lines }, Price::usd(micros)))
            .collect(),
            end_frame_seconds: BTreeSet::new(),
            audio: false,
        }
    }

    #[must_use]
    pub fn video_t2v_only() -> VideoCapabilities {
        VideoCapabilities {
            modes: [VideoMode::TextToVideo].into_iter().collect(),
            min_duration_ms: Some(1_000),
            max_duration_ms: Some(8_000),
            fps_options: [24].into_iter().collect(),
            camera_control: CameraControl::Textual,
            async_jobs: true,
            output_containers: [VideoContainer::Mp4].into_iter().collect(),
            ..VideoCapabilities::default()
        }
    }

    #[must_use]
    pub fn tts_basic() -> SpeechCapabilities {
        SpeechCapabilities {
            voice_sources: [VoiceSource::Catalog].into_iter().collect(),
            languages: ["en".to_owned(), "zh".to_owned()].into_iter().collect(),
            speed_permille: Some((500, 2000)),
            output_formats: [AudioFormat::Wav].into_iter().collect(),
            ..SpeechCapabilities::default()
        }
    }

    #[must_use]
    pub fn tts_full() -> SpeechCapabilities {
        SpeechCapabilities {
            voice_sources: [VoiceSource::Catalog, VoiceSource::Cloned]
                .into_iter()
                .collect(),
            languages: ["en".to_owned(), "ja".to_owned(), "zh".to_owned()]
                .into_iter()
                .collect(),
            emotions: Emotion::ALL.into_iter().collect(),
            speed_permille: Some((500, 2000)),
            pitch_semitones: Some((-12, 12)),
            timing: SpeechTiming::Word,
            output_formats: [AudioFormat::Mp3, AudioFormat::Wav].into_iter().collect(),
            max_text_chars: Some(9_999),
            price_per_character: Some(Price::usd(100)),
            ..SpeechCapabilities::default()
        }
    }

    #[must_use]
    pub fn llm_structured() -> LlmCapabilities {
        LlmCapabilities {
            context_window_tokens: Some(128_000),
            tools: true,
            structured_output: true,
            vision_input: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clip_is_the_nearest_length_the_provider_renders() {
        let listed = fake::video_i2v_resumable();
        assert_eq!(listed.nearest_clip_seconds(4.0), Some(5));
        assert_eq!(
            listed.nearest_clip_seconds(7.5),
            Some(5),
            "a tie goes to the shorter clip"
        );
        assert_eq!(listed.nearest_clip_seconds(8.0), Some(10));
        assert_eq!(listed.nearest_clip_seconds(30.0), Some(10));

        let ranged = fake::video_t2v_only();
        assert_eq!(ranged.nearest_clip_seconds(4.4), Some(4));
        assert_eq!(ranged.nearest_clip_seconds(0.2), Some(1));
        assert_eq!(ranged.nearest_clip_seconds(12.0), Some(8));
        assert_eq!(VideoCapabilities::default().nearest_clip_seconds(5.0), None);
    }

    #[test]
    fn i2v_fallback_never_silently_becomes_t2v() {
        let requirements = VideoRequirements {
            mode: VideoMode::ImageToVideo,
            requires_start_frame: true,
            requires_end_frame: false,
            requires_identity_reference: false,
            preferred_camera_control: CameraControl::Textual,
            requires_resumable_jobs: false,
        };

        let report = evaluate_video(requirements, &fake::video_t2v_only());

        assert!(!report.is_compatible());
        assert!(report.gaps.iter().any(|gap| gap.capability == "video.mode"));
        assert!(
            report
                .gaps
                .iter()
                .any(|gap| gap.capability == "video.start_frame")
        );
    }

    #[test]
    fn identity_reference_requirement_rejects_text_only_image_provider() {
        let requirements = ImageRequirements {
            required_reference_role: Some(ReferenceRole::Identity),
            minimum_seed_support: SeedSupport::None,
            requires_negative_prompt: false,
            requires_mask: false,
            requires_synchronous: false,
        };

        let report = evaluate_image(requirements, &fake::image_text_only());

        assert!(!report.is_compatible());
    }

    #[test]
    fn a_workflow_that_keeps_the_answer_rejects_an_image_provider_of_remote_jobs() {
        let requirements = ImageRequirements {
            required_reference_role: None,
            minimum_seed_support: SeedSupport::None,
            requires_negative_prompt: false,
            requires_mask: false,
            requires_synchronous: true,
        };
        let report = evaluate_image(requirements, &fake::image_full());
        assert_eq!(
            report.gaps,
            [CapabilityGap {
                capability: "image.async_jobs",
                severity: GapSeverity::Incompatible,
            }]
        );
        assert!(evaluate_image(requirements, &fake::image_text_only()).is_compatible());
        assert_eq!(
            fake::image_full()
                .price_per_image
                .map(|price| price.currency),
            Some(*b"USD")
        );
        assert!(Price::usd(0).is_free());
    }

    fn speaking(language: &str) -> SpeechRequirements {
        SpeechRequirements {
            voice_source: VoiceSource::Catalog,
            language: language.to_owned(),
            emotion: None,
            speed_permille: 1000,
            pitch_semitones: 0,
            preferred_timing: SpeechTiming::Sentence,
            requires_synchronous: true,
        }
    }

    #[test]
    fn a_chosen_voice_is_never_quietly_changed() {
        let full = fake::tts_full();
        assert!(evaluate_speech(&speaking("en"), &full).is_lossless());

        let basic = fake::tts_basic();
        let calm_and_low = SpeechRequirements {
            emotion: Some(Emotion::Calm),
            pitch_semitones: -3,
            ..speaking("ja")
        };
        let report = evaluate_speech(&calm_and_low, &basic);
        let missing: Vec<_> = report
            .gaps
            .iter()
            .map(|gap| (gap.capability, gap.severity))
            .collect();
        assert_eq!(
            missing,
            [
                ("speech.language", GapSeverity::Incompatible),
                ("speech.emotion", GapSeverity::Incompatible),
                ("speech.pitch", GapSeverity::Incompatible),
                ("speech.timing", GapSeverity::Warning),
            ]
        );

        // Within the declared range a rate is kept; outside it, or where
        // the rate cannot be set at all, it would change.
        let fast = SpeechRequirements {
            speed_permille: 1800,
            ..speaking("en")
        };
        assert!(evaluate_speech(&fast, &basic).is_compatible());
        let too_fast = SpeechRequirements {
            speed_permille: 2500,
            ..speaking("en")
        };
        assert!(!evaluate_speech(&too_fast, &basic).is_compatible());
        let fixed_rate = SpeechCapabilities {
            speed_permille: None,
            ..basic.clone()
        };
        assert!(!evaluate_speech(&fast, &fixed_rate).is_compatible());
        assert!(evaluate_speech(&speaking("en"), &fixed_rate).is_compatible());

        let remote = SpeechCapabilities {
            async_jobs: true,
            ..full.clone()
        };
        assert!(!evaluate_speech(&speaking("en"), &remote).is_compatible());
    }

    #[test]
    fn speech_is_priced_by_the_character_and_asked_for_compressed() {
        let full = fake::tts_full();
        assert_eq!(full.cost_micros(1_200), Some(120_000));
        assert_eq!(full.preferred_format(), Some(AudioFormat::Mp3));
        assert_eq!(fake::tts_basic().cost_micros(10), None);
        assert_eq!(fake::tts_basic().preferred_format(), Some(AudioFormat::Wav));
        assert_eq!(AudioFormat::Flac.extension(), "flac");
        for emotion in Emotion::ALL {
            assert_eq!(Emotion::parse(emotion.name()), Some(emotion));
        }
        assert_eq!(Emotion::parse("wistful"), None);
    }

    #[test]
    fn lesser_camera_control_is_visible_as_degradation() {
        let requirements = VideoRequirements {
            mode: VideoMode::ImageToVideo,
            requires_start_frame: true,
            requires_end_frame: false,
            requires_identity_reference: true,
            preferred_camera_control: CameraControl::Parametric,
            requires_resumable_jobs: true,
        };

        let report = evaluate_video(requirements, &fake::video_i2v_resumable());

        assert!(report.is_compatible());
        assert!(!report.is_lossless());
        assert_eq!(report.gaps[0].severity, GapSeverity::Warning);
    }

    #[test]
    fn a_paid_video_needs_a_task_that_outlives_a_restart() {
        let requirements = VideoRequirements {
            mode: VideoMode::ImageToVideo,
            requires_start_frame: true,
            requires_end_frame: false,
            requires_identity_reference: false,
            preferred_camera_control: CameraControl::None,
            requires_resumable_jobs: true,
        };
        let forgetful = VideoCapabilities {
            resumable_remote_handle: false,
            ..fake::video_i2v_resumable()
        };
        let report = evaluate_video(requirements, &forgetful);
        assert_eq!(
            report.gaps,
            [CapabilityGap {
                capability: "video.resumable_remote_handle",
                severity: GapSeverity::Incompatible,
            }]
        );
        let at_once = VideoCapabilities {
            async_jobs: false,
            ..fake::video_i2v_resumable()
        };
        assert!(
            evaluate_video(requirements, &at_once)
                .gaps
                .iter()
                .any(|gap| gap.capability == "video.async_jobs")
        );
    }

    #[test]
    fn a_clip_is_made_at_the_lowest_production_height_it_renders_at_that_length() {
        let capabilities = fake::video_i2v_resumable();
        // 720 lines is the lowest production height at 5 s; 10 s renders
        // only at 720.
        assert_eq!(capabilities.clip_lines(5), Some(720));
        assert_eq!(capabilities.clip_lines(10), Some(720));
        assert_eq!(capabilities.clip_price(5), Some(Price::usd(250_000)));
        assert!(!capabilities.renders(VideoClip {
            seconds: 10,
            lines: 1080
        }));
        // Below production height, the tallest it has.
        let small = VideoCapabilities {
            resolutions: [180, 360].into_iter().collect(),
            clips: BTreeSet::new(),
            ..fake::video_i2v_resumable()
        };
        assert_eq!(small.clip_lines(5), Some(360));
        assert!(small.renders(VideoClip {
            seconds: 10,
            lines: 180
        }));
        assert_eq!(VideoCapabilities::default().clip_lines(5), None);
    }
}
