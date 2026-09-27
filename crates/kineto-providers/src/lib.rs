use std::collections::BTreeSet;

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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ImageCapabilities {
    pub aspect_ratios: BTreeSet<AspectRatio>,
    pub resolutions: BTreeSet<Resolution>,
    pub seed_support: SeedSupport,
    pub negative_prompt: bool,
    pub max_reference_images: u16,
    pub reference_roles: BTreeSet<ReferenceRole>,
    pub inpainting: bool,
    pub masks: bool,
    pub output_formats: BTreeSet<ImageFormat>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TtsCapabilities {
    pub voice_reference: bool,
    pub locales: BTreeSet<String>,
    pub emotion_control: bool,
    pub rate_control: bool,
    pub timing_metadata: bool,
    pub streaming: bool,
    pub async_generation: bool,
    pub output_formats: BTreeSet<AudioFormat>,
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

    CompatibilityReport { gaps }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ImageRequirements {
    pub required_reference_role: Option<ReferenceRole>,
    pub minimum_seed_support: SeedSupport,
    pub requires_negative_prompt: bool,
    pub requires_mask: bool,
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
            inpainting: true,
            masks: true,
            output_formats: [ImageFormat::Png].into_iter().collect(),
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
    pub fn tts_basic() -> TtsCapabilities {
        TtsCapabilities {
            locales: ["en-US".to_owned(), "zh-CN".to_owned()]
                .into_iter()
                .collect(),
            rate_control: true,
            output_formats: [AudioFormat::Wav].into_iter().collect(),
            ..TtsCapabilities::default()
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
    fn i2v_fallback_never_silently_becomes_t2v() {
        let requirements = VideoRequirements {
            mode: VideoMode::ImageToVideo,
            requires_start_frame: true,
            requires_end_frame: false,
            requires_identity_reference: false,
            preferred_camera_control: CameraControl::Textual,
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
        };

        let report = evaluate_image(requirements, &fake::image_text_only());

        assert!(!report.is_compatible());
    }

    #[test]
    fn lesser_camera_control_is_visible_as_degradation() {
        let requirements = VideoRequirements {
            mode: VideoMode::ImageToVideo,
            requires_start_frame: true,
            requires_end_frame: false,
            requires_identity_reference: true,
            preferred_camera_control: CameraControl::Parametric,
        };

        let report = evaluate_video(requirements, &fake::video_i2v_resumable());

        assert!(report.is_compatible());
        assert!(!report.is_lossless());
        assert_eq!(report.gaps[0].severity, GapSeverity::Warning);
    }
}
