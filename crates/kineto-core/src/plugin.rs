//! Provider plugins (ADR-0006) as image, video and speech providers.
//!
//! A plugin the user registered is described once, when a session selects
//! it: what it declares is read into the same `ImageCapabilities` and
//! `SpeechCapabilities` built-ins declare, so it is routed, priced and
//! refused exactly as they are. Its jobs and records are kept under its
//! provider key, `plugin.<id>`.
//!
//! Images keep MiniMax's write-ahead discipline, with one step more. Before
//! the call, what the call is (its prompt's provenance, what it depends on,
//! where its image will be kept) is written next to where its receipt will
//! be; after the call, the image and then its receipt are written before
//! the call returns, so a restart completes the candidate from disk instead
//! of paying again:
//!
//! ```text
//! sent record -> plugin invoke -> image -> receipt -> intent invoked -> attach -> acknowledge
//! ```
//!
//! A call that was sent and left no receipt is asked about by idempotency
//! key (`reconcile`), and a plugin that kept the answer gives it back; a
//! call never sent is not asked about. Only a plugin that never started, or
//! that says the provider did not take a request (`accepted: false`),
//! proves a call was not charged; anything else leaves it unresolved, made
//! again only on the director's decision to risk paying twice.
//!
//! Speech runs through the same speech jobs and receipts as every speech
//! provider; the plugin only speaks. Video runs through the same video jobs
//! as every video provider: the plugin's `invoke` answers a job to follow,
//! and asking about that job (`reconcile`) is free; its clip is checked as
//! an MP4 before it reaches the project.

use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    num::NonZeroU16,
    sync::{Arc, Mutex},
    time::Duration,
};

use kineto_job_runtime::{
    Invocation, JobRuntimePolicy, PaidJobAdapter, ProviderKey, Reconciliation,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, CurrencyCode, ErrorClass, FallbackPolicy, IdempotencyKey,
    PaidFallback, ProviderExecutionPolicy, ProviderJobId,
};
use kineto_plugins::{
    Answer, ConfigError, Done, FailureClass, HostError, Input, Limits, PluginConfig, PluginEntry,
};
use kineto_project::{
    ArtifactDependency, ArtifactId, ContentHash, InputHash, cast,
    fs::{ProjectFsError, ProjectRelativePath},
    manifest::CanonicalProject,
    revisions::DependencyRecord,
    shot::{CandidateOutput, PromptProvenance, keyframe_media_path},
    style,
};
use kineto_providers::{
    AspectRatio, AudioFormat, CameraControl, Emotion, ImageCapabilities, ImageFormat, Price,
    ReferenceRole, SpeechCapabilities, SpeechTiming, VideoCapabilities, VideoClip, VideoContainer,
    VideoMode, VoiceSource,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    generation::{
        CandidateGenerator, CandidateOwner, CandidateRequest, GeneratedCandidate,
        parse_candidate_key, provenance,
    },
    speech::{SpeechGenerator, SpeechRequest, Spoken, Voice},
    video::{TaskState, VideoGenerator, VideoSubmission},
};

const RECORD_SCHEMA_VERSION: u32 = 1;

/// The prefix a selection names a plugin with, as in `plugin:studio-images`.
pub const SELECTION_PREFIX: &str = "plugin:";

/// The request field identity references travel in, as a cast lock records
/// it.
pub const REFERENCE_MECHANISM: &str = "references";

#[derive(Debug)]
pub enum PluginError {
    /// The plugin could not be run, or answered outside the protocol.
    Host(HostError),
    /// The plugin's description is not one Kineto can use.
    Described(String),
    /// The plugin answered with a failure.
    Failed {
        class: FailureClass,
        /// `Some(false)` only when the provider certainly did not take it.
        accepted: Option<bool>,
        message: String,
    },
    /// The plugin answered something the operation does not take.
    Unexpected(&'static str),
    /// The request asks for something the plugin does not declare; nothing
    /// was sent.
    Unsupported(&'static str),
    /// The candidate has no compiled prompt to send.
    MissingPrompt,
    /// A call's records or file are missing or do not match. Nothing is
    /// made again; a person needs to look.
    Receipt(&'static str),
    /// Writing down a call's answer failed after the plugin gave it.
    Fs(ProjectFsError),
    /// The project could not be prepared for a call, so none was made.
    Unsent(ProjectFsError),
}

impl fmt::Display for PluginError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(error) => error.fmt(formatter),
            Self::Described(reason) => {
                write!(formatter, "the plugin's description is unusable: {reason}")
            }
            Self::Failed { message, .. } => write!(formatter, "the plugin failed: {message}"),
            Self::Unexpected(what) => write!(formatter, "the plugin answered {what}"),
            Self::Unsupported(what) => write!(formatter, "no plugin call was made: {what}"),
            Self::MissingPrompt => formatter.write_str("the candidate has no compiled prompt"),
            Self::Receipt(what) => write!(formatter, "plugin call record: {what}"),
            Self::Fs(error) => error.fmt(formatter),
            Self::Unsent(error) => write!(formatter, "no plugin call was made: {error}"),
        }
    }
}

impl Error for PluginError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Host(error) => Some(error),
            Self::Fs(error) | Self::Unsent(error) => Some(error),
            Self::Described(_)
            | Self::Failed { .. }
            | Self::Unexpected(_)
            | Self::Unsupported(_)
            | Self::MissingPrompt
            | Self::Receipt(_) => None,
        }
    }
}

impl PluginError {
    /// Whether the call certainly was not charged: it was never sent, the
    /// plugin never started, or it said the provider did not take it.
    #[must_use]
    pub const fn certainly_not_accepted(&self) -> bool {
        match self {
            Self::Host(error) => error.unsent(),
            Self::Failed { accepted, .. } => matches!(accepted, Some(false)),
            Self::Described(_) | Self::Unsupported(_) | Self::MissingPrompt | Self::Unsent(_) => {
                true
            }
            // Raised after the plugin answered, or on a kept answer.
            Self::Unexpected(_) | Self::Receipt(_) | Self::Fs(_) => false,
        }
    }

    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Host(HostError::Timeout | HostError::Exited(_) | HostError::Spawn(_))
            | Self::Fs(_)
            | Self::Unsent(_)
            | Self::Failed {
                class: FailureClass::Retryable,
                ..
            } => ErrorClass::Retryable,
            _ => ErrorClass::Terminal,
        }
    }
}

/// Why a selection names no usable plugin.
#[derive(Debug)]
pub enum PluginSelectionError {
    Config(ConfigError),
    /// No enabled plugin has this ID.
    Unknown(String),
    /// It is registered but could not describe itself.
    Unusable(String, PluginError),
}

impl fmt::Display for PluginSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => error.fmt(formatter),
            Self::Unknown(id) => write!(formatter, "no enabled plugin is registered as {id:?}"),
            Self::Unusable(id, error) => write!(formatter, "plugin {id:?}: {error}"),
        }
    }
}

impl Error for PluginSelectionError {}

/// The registered, enabled plugin `id` of `config`, described.
pub fn select(config: &PluginConfig, id: &str) -> Result<PluginProvider, PluginSelectionError> {
    let entry = config
        .enabled(id)
        .ok_or_else(|| PluginSelectionError::Unknown(id.to_owned()))?;
    PluginProvider::connect(entry.clone())
        .map_err(|error| PluginSelectionError::Unusable(id.to_owned(), error))
}

/// The plugin a selection such as `plugin:studio-images` names, from the
/// user's configuration; `None` when the selection names no plugin.
pub fn select_named(selection: &str) -> Option<Result<PluginProvider, PluginSelectionError>> {
    let id = selection.strip_prefix(SELECTION_PREFIX)?;
    Some(
        PluginConfig::load_default()
            .map_err(PluginSelectionError::Config)
            .and_then(|config| select(&config, id)),
    )
}

/// A registered plugin as settings show it.
#[derive(Debug)]
pub struct PluginListing {
    pub entry: PluginEntry,
    /// `None` when it is disabled; otherwise what it described, or why it
    /// could not.
    pub described: Option<Result<PluginProvider, String>>,
}

/// Every plugin `config` registers, in its order, each enabled one started
/// once to describe itself.
#[must_use]
pub fn list(config: &PluginConfig) -> Vec<PluginListing> {
    config
        .entries()
        .iter()
        .map(|entry| PluginListing {
            entry: entry.clone(),
            described: entry
                .enabled
                .then(|| PluginProvider::connect(entry.clone()).map_err(|error| error.to_string())),
        })
        .collect()
}

/// A registered plugin, as it described itself.
#[derive(Debug, Clone)]
pub struct PluginProvider {
    entry: PluginEntry,
    key: String,
    name: String,
    version: String,
    image: Option<(ImageCapabilities, String)>,
    speech: Option<(SpeechCapabilities, String)>,
    video: Option<(VideoCapabilities, String)>,
    /// How long an `invoke` may take: 15 minutes, or less if it says so.
    invoke: Limits,
}

/// A secret the entry names, read from the environment. OS secure storage
/// is for the desktop shell to add.
fn environment_secret(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

impl PluginProvider {
    /// Start `entry` once to read what it declares.
    pub fn connect(entry: PluginEntry) -> Result<Self, PluginError> {
        let done = kineto_plugins::run(
            &entry,
            &environment_secret,
            "describe",
            json!({}),
            &[],
            Limits::QUICK,
        )
        .map_err(PluginError::Host)?;
        let Answer::Ok { body } = done.answer else {
            return Err(PluginError::Unexpected(
                "something other than its description",
            ));
        };
        let text = |field: &str| body[field].as_str().unwrap_or_default().to_owned();
        let capabilities = &body["capabilities"];
        let image = capabilities
            .get("image.generate")
            .map(image_capabilities)
            .transpose()
            .map_err(PluginError::Described)?;
        let speech = capabilities
            .get("tts.speak")
            .map(speech_capabilities)
            .transpose()
            .map_err(PluginError::Described)?;
        let video = capabilities
            .get("video.generate")
            .map(video_capabilities)
            .transpose()
            .map_err(PluginError::Described)?;
        if image.is_none() && speech.is_none() && video.is_none() {
            return Err(PluginError::Described(
                "it declares none of image.generate, video.generate and tts.speak".to_owned(),
            ));
        }
        let deadline = number(&body, "invoke_deadline_seconds")
            .map_err(PluginError::Described)?
            .filter(|seconds| *seconds > 0)
            .map_or(Limits::INVOKE.deadline, |seconds| {
                Limits::INVOKE.deadline.min(Duration::from_secs(seconds))
            });
        Ok(Self {
            key: entry.provider_key(),
            entry,
            name: text("name"),
            version: text("version"),
            image,
            speech,
            video,
            invoke: Limits {
                deadline,
                ..Limits::INVOKE
            },
        })
    }

    #[must_use]
    pub fn id(&self) -> &str {
        &self.entry.id
    }

    /// The key its jobs and records are kept under, `plugin.<id>`.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The name it gives itself.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn image_capabilities(&self) -> Option<&ImageCapabilities> {
        self.image.as_ref().map(|(capabilities, _)| capabilities)
    }

    #[must_use]
    pub fn image_model(&self) -> Option<&str> {
        self.image.as_ref().map(|(_, model)| model.as_str())
    }

    #[must_use]
    pub fn speech_capabilities(&self) -> Option<&SpeechCapabilities> {
        self.speech.as_ref().map(|(capabilities, _)| capabilities)
    }

    #[must_use]
    pub fn video_capabilities(&self) -> Option<&VideoCapabilities> {
        self.video.as_ref().map(|(capabilities, _)| capabilities)
    }

    #[must_use]
    pub fn video_model(&self) -> Option<&str> {
        self.video.as_ref().map(|(_, model)| model.as_str())
    }

    #[must_use]
    pub fn speech_model(&self) -> Option<&str> {
        self.speech.as_ref().map(|(_, model)| model.as_str())
    }

    fn call(
        &self,
        operation: &str,
        fields: Value,
        inputs: &[Input],
        limits: Limits,
    ) -> Result<Done, PluginError> {
        kineto_plugins::run(
            &self.entry,
            &environment_secret,
            operation,
            fields,
            inputs,
            limits,
        )
        .map_err(PluginError::Host)
    }
}

fn names(value: &Value, field: &str) -> Result<Vec<String>, String> {
    match &value[field] {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| format!("{field} lists something other than a name"))
            })
            .collect(),
        _ => Err(format!("{field} is not a list")),
    }
}

fn number(value: &Value, field: &str) -> Result<Option<u64>, String> {
    match &value[field] {
        Value::Null => Ok(None),
        found => found
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{field} is not a whole number")),
    }
}

fn limit(value: &Value, field: &str) -> Result<Option<usize>, String> {
    number(value, field).map(|found| found.map(|n| usize::try_from(n).unwrap_or(usize::MAX)))
}

fn image_format(name: &str) -> Result<ImageFormat, String> {
    match name {
        "jpeg" => Ok(ImageFormat::Jpeg),
        "png" => Ok(ImageFormat::Png),
        "webp" => Ok(ImageFormat::Webp),
        other => Err(format!("{other:?} is not an image format")),
    }
}

const fn extension(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Png => "png",
        ImageFormat::Webp => "webp",
    }
}

fn ratio(name: &str) -> Result<AspectRatio, String> {
    name.split_once(':')
        .and_then(|(width, height)| AspectRatio::new(width.parse().ok()?, height.parse().ok()?))
        .ok_or_else(|| format!("{name:?} is not an aspect ratio"))
}

fn model(declared: &Value) -> Result<String, String> {
    match declared["model"].as_str().map(str::trim) {
        Some(model) if !model.is_empty() && model.len() <= 128 => Ok(model.to_owned()),
        _ => Err("it names no model".to_owned()),
    }
}

/// What a plugin declares for `image.generate`, and its model. Images that
/// arrive as jobs to follow are not taken yet.
pub fn image_capabilities(declared: &Value) -> Result<(ImageCapabilities, String), String> {
    let reference_roles = names(declared, "reference_roles")?
        .iter()
        .map(|role| match role.as_str() {
            "identity" => Ok(ReferenceRole::Identity),
            "style" => Ok(ReferenceRole::Style),
            "composition" => Ok(ReferenceRole::Composition),
            other => Err(format!("{other:?} is not a reference role")),
        })
        .collect::<Result<BTreeSet<_>, _>>()?;
    let capabilities = ImageCapabilities {
        aspect_ratios: names(declared, "aspect_ratios")?
            .iter()
            .map(|name| ratio(name))
            .collect::<Result<_, _>>()?,
        max_reference_images: u16::try_from(number(declared, "max_reference_images")?.unwrap_or(0))
            .map_err(|_| "max_reference_images is too large".to_owned())?,
        reference_roles,
        reference_formats: names(declared, "reference_formats")?
            .iter()
            .map(|name| image_format(name))
            .collect::<Result<_, _>>()?,
        max_reference_bytes: number(declared, "max_reference_bytes")?,
        output_formats: names(declared, "output_formats")?
            .iter()
            .map(|name| image_format(name))
            .collect::<Result<_, _>>()?,
        max_prompt_chars: limit(declared, "max_prompt_chars")?,
        async_jobs: false,
        price_per_image: number(declared, "price_per_image_micros")?.map(Price::usd),
        ..ImageCapabilities::default()
    };
    if capabilities.aspect_ratios.is_empty() || capabilities.output_formats.is_empty() {
        return Err("image.generate declares no aspect ratio or no output format".to_owned());
    }
    Ok((capabilities, model(declared)?))
}

/// What a plugin declares for `tts.speak`, and its model. A plugin speaks
/// with the voices it lists.
pub fn speech_capabilities(declared: &Value) -> Result<(SpeechCapabilities, String), String> {
    let bounds = |field: &str| -> Result<Option<(i64, i64)>, String> {
        match &declared[field] {
            Value::Null => Ok(None),
            Value::Array(bounds) => match bounds.as_slice() {
                [low, high] => low
                    .as_i64()
                    .zip(high.as_i64())
                    .filter(|(low, high)| low <= high)
                    .map(Some)
                    .ok_or_else(|| format!("{field} is not [lowest, highest]")),
                _ => Err(format!("{field} is not [lowest, highest]")),
            },
            _ => Err(format!("{field} is not [lowest, highest]")),
        }
    };
    let speed_permille = bounds("speed_permille")?
        .map(|(low, high)| u16::try_from(low).ok().zip(u16::try_from(high).ok()))
        .map(|found| found.ok_or_else(|| "speed_permille is out of range".to_owned()))
        .transpose()?;
    let pitch_semitones = bounds("pitch_semitones")?
        .map(|(low, high)| i8::try_from(low).ok().zip(i8::try_from(high).ok()))
        .map(|found| found.ok_or_else(|| "pitch_semitones is out of range".to_owned()))
        .transpose()?;
    let capabilities = SpeechCapabilities {
        voice_sources: BTreeSet::from([VoiceSource::Catalog]),
        languages: names(declared, "languages")?.into_iter().collect(),
        emotions: names(declared, "emotions")?
            .iter()
            .map(|name| Emotion::parse(name).ok_or_else(|| format!("{name:?} is not a delivery")))
            .collect::<Result<_, _>>()?,
        speed_permille,
        pitch_semitones,
        timing: SpeechTiming::None,
        async_jobs: false,
        output_formats: names(declared, "output_formats")?
            .iter()
            .map(|name| match name.as_str() {
                "wav" => Ok(AudioFormat::Wav),
                "mp3" => Ok(AudioFormat::Mp3),
                "flac" => Ok(AudioFormat::Flac),
                other => Err(format!("{other:?} is not an audio format")),
            })
            .collect::<Result<_, _>>()?,
        max_text_chars: limit(declared, "max_text_chars")?,
        price_per_character: number(declared, "price_per_character_micros")?.map(Price::usd),
    };
    if capabilities.languages.is_empty() || capabilities.output_formats.is_empty() {
        return Err("tts.speak declares no language or no output format".to_owned());
    }
    Ok((capabilities, model(declared)?))
}

/// What a plugin declares for `video.generate`, and its model. A plugin
/// starts every clip from the shot's locked keyframe, lists each clip it
/// renders by its length and height with its price, may close a clip on a
/// last frame, and answers a job to follow.
pub fn video_capabilities(declared: &Value) -> Result<(VideoCapabilities, String), String> {
    let Value::Array(listed) = &declared["clips"] else {
        return Err("video.generate declares no clips".to_owned());
    };
    let mut clips = BTreeSet::new();
    let mut clip_prices = BTreeMap::new();
    for clip in listed {
        let whole = |field: &str| clip[field].as_u64().filter(|whole| *whole > 0);
        let (Some(seconds), Some(lines)) = (
            whole("seconds").and_then(|seconds| u8::try_from(seconds).ok()),
            whole("lines").and_then(|lines| u16::try_from(lines).ok()),
        ) else {
            return Err("a clip is not a length in seconds and a height in lines".to_owned());
        };
        let rendered = VideoClip { seconds, lines };
        if !clips.insert(rendered) {
            return Err(format!(
                "the {seconds}-second {lines}-line clip is listed twice"
            ));
        }
        if let Some(price) = number(clip, "price_micros")? {
            clip_prices.insert(rendered, Price::usd(price));
        }
    }
    if clips.is_empty() {
        return Err("video.generate declares no clips".to_owned());
    }
    let clip_seconds: BTreeSet<u8> = clips.iter().map(|clip| clip.seconds).collect();
    let resolutions: BTreeSet<u16> = clips.iter().map(|clip| clip.lines).collect();
    let start_frame_formats: BTreeSet<ImageFormat> = names(declared, "start_frame_formats")?
        .iter()
        .map(|name| image_format(name))
        .collect::<Result<_, _>>()?;
    if start_frame_formats.is_empty() {
        return Err("video.generate declares no start_frame_formats".to_owned());
    }
    let end_frame_formats: BTreeSet<ImageFormat> = names(declared, "end_frame_formats")?
        .iter()
        .map(|name| image_format(name))
        .collect::<Result<_, _>>()?;
    let to_ms = |seconds: Option<&u8>| seconds.map(|seconds| u64::from(*seconds) * 1000);
    let fps = number(declared, "fps")?
        .and_then(|fps| u16::try_from(fps).ok())
        .unwrap_or(24);
    let capabilities = VideoCapabilities {
        modes: BTreeSet::from([VideoMode::ImageToVideo]),
        min_duration_ms: to_ms(clip_seconds.first()),
        max_duration_ms: to_ms(clip_seconds.last()),
        fps_options: BTreeSet::from([fps]),
        start_frame: true,
        end_frame: !end_frame_formats.is_empty(),
        camera_control: CameraControl::None,
        async_jobs: true,
        resumable_remote_handle: true,
        output_containers: BTreeSet::from([VideoContainer::Mp4]),
        clip_seconds,
        max_prompt_chars: limit(declared, "max_prompt_chars")?,
        start_frame_formats,
        max_start_frame_bytes: number(declared, "max_start_frame_bytes")?,
        end_frame_formats,
        max_end_frame_bytes: number(declared, "max_end_frame_bytes")?,
        resolutions,
        clips,
        clip_prices,
        ..VideoCapabilities::default()
    };
    Ok((capabilities, model(declared)?))
}

/// What a plugin image call is, written before it is sent. Runtime state,
/// never canonical.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Sent {
    schema_version: u32,
    provider: String,
    model: String,
    artifact_id: String,
    input_hash: String,
    /// Where its image is kept, by file extension.
    media_paths: BTreeMap<String, String>,
    provenance: PromptProvenance,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    dependencies: Vec<DependencyRecord>,
}

/// What is kept of a finished plugin image call until its candidate is
/// acknowledged.
#[derive(Debug, Serialize, Deserialize)]
struct Receipt {
    #[serde(flatten)]
    sent: Sent,
    media_path: String,
    content_hash: String,
}

/// Proof that a plugin said the provider did not take a candidate's latest
/// call.
#[derive(Debug, Serialize, Deserialize)]
struct Refusal {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
}

/// A plugin's images for one project. Cheap to copy: every candidate of a
/// generation gets its own copy and they run in parallel.
#[derive(Clone, Copy)]
pub struct PluginImages<'a> {
    project: &'a CanonicalProject,
    plugin: &'a PluginProvider,
}

impl<'a> PluginImages<'a> {
    #[must_use]
    pub const fn new(project: &'a CanonicalProject, plugin: &'a PluginProvider) -> Self {
        Self { project, plugin }
    }

    fn path(artifact_id: &ArtifactId, suffix: &str) -> Result<ProjectRelativePath, PluginError> {
        ProjectRelativePath::new(format!(
            ".kineto/generation/{}{suffix}.json",
            artifact_id.as_str()
        ))
        .map_err(|_| PluginError::Receipt("artifact ID is not a valid file name"))
    }

    fn write<T: Serialize>(
        &self,
        artifact_id: &ArtifactId,
        suffix: &str,
        record: &T,
    ) -> Result<(), ProjectFsError> {
        let invalid = |kind| ProjectFsError::Io(std::io::Error::from(kind));
        let mut encoded = serde_json::to_vec_pretty(record)
            .map_err(|_| invalid(std::io::ErrorKind::InvalidData))?;
        encoded.push(b'\n');
        let path = Self::path(artifact_id, suffix)
            .map_err(|_| invalid(std::io::ErrorKind::InvalidInput))?;
        self.project.root().write_atomic(&path, &encoded)
    }

    fn read<T: for<'de> Deserialize<'de>>(
        &self,
        artifact_id: &ArtifactId,
        suffix: &str,
    ) -> Result<Option<T>, PluginError> {
        match self.project.root().read(&Self::path(artifact_id, suffix)?) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map(Some)
                .map_err(|_| PluginError::Receipt("a call record is not readable")),
            Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(None)
            }
            Err(error) => Err(PluginError::Fs(error)),
        }
    }

    fn remove(&self, artifact_id: &ArtifactId, suffix: &str) -> std::io::Result<()> {
        let Ok(path) = Self::path(artifact_id, suffix) else {
            return Ok(());
        };
        match std::fs::remove_file(self.project.root().resolve(&path)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn refused(&self, artifact_id: &ArtifactId, input_hash: &InputHash) -> bool {
        matches!(
            self.read::<Refusal>(artifact_id, ".refused"),
            Ok(Some(refusal)) if refusal.schema_version == RECORD_SCHEMA_VERSION
                && refusal.artifact_id == artifact_id.as_str()
                && refusal.input_hash == input_hash.as_str()
        )
    }

    /// Best effort: without this proof the call stays unresolved, which
    /// fails closed.
    fn record_refusal(&self, artifact_id: &ArtifactId, input_hash: &InputHash) {
        let _ = self.write(
            artifact_id,
            ".refused",
            &Refusal {
                schema_version: RECORD_SCHEMA_VERSION,
                artifact_id: artifact_id.as_str().to_owned(),
                input_hash: input_hash.as_str().to_owned(),
            },
        );
    }

    /// The record of a sent call for exactly this candidate.
    fn sent(
        &self,
        artifact_id: &ArtifactId,
        input_hash: &InputHash,
    ) -> Result<Option<Sent>, PluginError> {
        Ok(self
            .read::<Sent>(artifact_id, ".sent")?
            .filter(|sent| self.belongs(sent, artifact_id, input_hash)))
    }

    fn belongs(&self, sent: &Sent, artifact_id: &ArtifactId, input_hash: &InputHash) -> bool {
        sent.schema_version == RECORD_SCHEMA_VERSION
            && sent.artifact_id == artifact_id.as_str()
            && sent.input_hash == input_hash.as_str()
            && sent.provider == self.plugin.key
    }

    /// Rebuild a candidate from its receipt, after proving the image on
    /// disk is the one the receipt names.
    fn candidate_from_receipt(
        &self,
        artifact_id: &ArtifactId,
        input_hash: &InputHash,
        receipt: Receipt,
    ) -> Result<GeneratedCandidate, PluginError> {
        if !self.belongs(&receipt.sent, artifact_id, input_hash) {
            return Err(PluginError::Receipt("receipt belongs to another candidate"));
        }
        let media_path = ProjectRelativePath::new(&receipt.media_path)
            .map_err(|_| PluginError::Receipt("receipt media path is invalid"))?;
        let content_hash = ContentHash::new(receipt.content_hash)
            .map_err(|_| PluginError::Receipt("receipt content hash is invalid"))?;
        let bytes = self
            .project
            .root()
            .read(&media_path)
            .map_err(|error| match error {
                ProjectFsError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    PluginError::Receipt("image file is missing")
                }
                error => PluginError::Fs(error),
            })?;
        if ContentHash::of(&bytes) != content_hash {
            return Err(PluginError::Receipt(
                "image file does not match its receipt",
            ));
        }
        let dependencies = receipt
            .sent
            .dependencies
            .iter()
            .map(|dependency| {
                Ok(ArtifactDependency {
                    artifact_id: ArtifactId::new(dependency.artifact_id.clone())
                        .map_err(|_| PluginError::Receipt("a dependency is invalid"))?,
                    content_hash: ContentHash::new(dependency.content_hash.clone())
                        .map_err(|_| PluginError::Receipt("a dependency is invalid"))?,
                    impact: dependency.impact,
                    field: dependency.field.clone(),
                })
            })
            .collect::<Result<_, PluginError>>()?;
        Ok(GeneratedCandidate {
            output: CandidateOutput {
                artifact_id: artifact_id.clone(),
                content_hash,
                provider: receipt.sent.provider,
                model: receipt.sent.model,
                media_path: Some(media_path),
                provenance: Some(receipt.sent.provenance),
                dependencies,
            },
            input_hash: input_hash.clone(),
        })
    }

    fn completed_from_receipt(
        &self,
        artifact_id: &ArtifactId,
        input_hash: &InputHash,
    ) -> Result<Option<GeneratedCandidate>, PluginError> {
        self.read::<Receipt>(artifact_id, "")?
            .map(|receipt| self.candidate_from_receipt(artifact_id, input_hash, receipt))
            .transpose()
    }

    /// Keep the image a plugin answered `sent` with: write it where `sent`
    /// says, then its receipt, then return the candidate.
    fn keep(&self, sent: Sent, done: &Done) -> Result<GeneratedCandidate, PluginError> {
        let [file] = done.files.as_slice() else {
            return Err(PluginError::Unexpected("other than one image"));
        };
        let declared = self
            .plugin
            .image_capabilities()
            .map(|capabilities| &capabilities.output_formats);
        let format = kineto_provider_minimax::sniff(&file.bytes)
            .filter(|format| declared.is_some_and(|formats| formats.contains(format)))
            .ok_or(PluginError::Unexpected(
                "an image in a format it does not declare",
            ))?;
        let media_path = sent
            .media_paths
            .get(extension(format))
            .cloned()
            .ok_or(PluginError::Receipt("no place was kept for this format"))?;
        let path = ProjectRelativePath::new(&media_path)
            .map_err(|_| PluginError::Receipt("a kept media path is invalid"))?;
        let artifact_id = ArtifactId::new(sent.artifact_id.clone())
            .map_err(|_| PluginError::Receipt("a call record names an invalid ID"))?;
        let input_hash = InputHash::new(sent.input_hash.clone())
            .map_err(|_| PluginError::Receipt("a call record names an invalid input"))?;
        self.project
            .root()
            .write_atomic(&path, &file.bytes)
            .map_err(PluginError::Fs)?;
        let receipt = Receipt {
            media_path,
            content_hash: ContentHash::of(&file.bytes).as_str().to_owned(),
            sent,
        };
        self.write(&artifact_id, "", &receipt)
            .map_err(PluginError::Fs)?;
        self.candidate_from_receipt(&artifact_id, &input_hash, receipt)
    }

    /// Where `request`'s image is kept in each format the plugin declares.
    fn media_paths(
        request: &CandidateRequest,
        formats: &BTreeSet<ImageFormat>,
    ) -> Result<BTreeMap<String, String>, PluginError> {
        formats
            .iter()
            .map(|format| {
                let extension = extension(*format);
                let path = match &request.owner {
                    CandidateOwner::Shot(key) => {
                        keyframe_media_path(key, &request.artifact_id, extension).ok()
                    }
                    CandidateOwner::Style => {
                        style::sample_media_path(&request.artifact_id, extension).ok()
                    }
                    CandidateOwner::Character { id, kind } => {
                        cast::media_path(id, &request.artifact_id, *kind, extension).ok()
                    }
                }
                .ok_or(PluginError::Unsupported("no image path for this candidate"))?;
                Ok((
                    extension.to_owned(),
                    path.as_path().to_string_lossy().replace('\\', "/"),
                ))
            })
            .collect()
    }
}

/// The references a request sends, as inputs and as the request lists them.
fn references(request: &CandidateRequest) -> (Vec<Input>, Vec<Value>) {
    let mut inputs = Vec::new();
    let mut listed = Vec::new();
    let roles = request
        .identities
        .iter()
        .map(|image| ("identity", image))
        .chain(
            request
                .composition
                .iter()
                .map(|image| ("composition", image)),
        );
    for (index, (role, image)) in roles.enumerate() {
        let format = kineto_provider_minimax::sniff(&image.bytes).unwrap_or(ImageFormat::Jpeg);
        let name = format!("{role}-{index}.{}", extension(format));
        listed.push(json!({ "input": format!("inputs/{name}"), "role": role }));
        inputs.push(Input {
            name,
            bytes: image.bytes.to_vec(),
        });
    }
    (inputs, listed)
}

impl PaidJobAdapter for PluginImages<'_> {
    type Request = CandidateRequest;
    type Output = GeneratedCandidate;
    type Error = PluginError;

    fn estimate_cost(&self, _request: &CandidateRequest) -> Result<CostEstimate, PluginError> {
        let price = self
            .plugin
            .image_capabilities()
            .and_then(|capabilities| capabilities.price_per_image)
            .unwrap_or(Price::usd(0));
        Ok(CostEstimate {
            call_count: 1,
            amount_micros: price.amount_micros,
            currency: CurrencyCode::new(price.currency)
                .map_err(|_| PluginError::Described("its price has no currency".to_owned()))?,
            estimated_duration_ms: None,
        })
    }

    fn invoke(
        &mut self,
        request: &CandidateRequest,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<GeneratedCandidate>, PluginError> {
        let prompt = request
            .prompt
            .as_deref()
            .ok_or(PluginError::MissingPrompt)?;
        // An earlier call answered and its receipt was written, but the
        // intent never said so: use that answer instead of paying again.
        if let Some(candidate) =
            self.completed_from_receipt(&request.artifact_id, &request.input_hash)?
        {
            return Ok(Invocation::Completed(candidate));
        }
        self.remove(&request.artifact_id, ".refused")
            .map_err(|error| PluginError::Unsent(ProjectFsError::Io(error)))?;
        let capabilities = self
            .plugin
            .image_capabilities()
            .ok_or(PluginError::Unsupported("the plugin makes no images"))?;
        let refuse = |why| {
            self.record_refusal(&request.artifact_id, &request.input_hash);
            Err(PluginError::Unsupported(why))
        };
        if !capabilities.aspect_ratios.contains(&request.aspect_ratio) {
            return refuse("the plugin does not take this aspect ratio");
        }
        let (inputs, listed) = references(request);
        let takes = |role| capabilities.reference_roles.contains(&role);
        if inputs.len() > usize::from(capabilities.max_reference_images)
            || (!request.identities.is_empty() && !takes(ReferenceRole::Identity))
            || (request.composition.is_some() && !takes(ReferenceRole::Composition))
        {
            return refuse("the plugin does not take these references");
        }
        let sent = Sent {
            schema_version: RECORD_SCHEMA_VERSION,
            provider: self.plugin.key.clone(),
            model: self.plugin.image_model().unwrap_or_default().to_owned(),
            artifact_id: request.artifact_id.as_str().to_owned(),
            input_hash: request.input_hash.as_str().to_owned(),
            media_paths: Self::media_paths(request, &capabilities.output_formats)?,
            provenance: provenance(prompt),
            dependencies: prompt
                .dependencies
                .iter()
                .map(DependencyRecord::from)
                .collect(),
        };
        self.write(&request.artifact_id, ".sent", &sent)
            .map_err(PluginError::Unsent)?;
        let fields = json!({
            "capability": "image.generate",
            "idempotency_key": idempotency_key.as_str(),
            "request": {
                "prompt": prompt.compiled.prompt(),
                "aspect_ratio": format!(
                    "{}:{}",
                    request.aspect_ratio.width, request.aspect_ratio.height
                ),
                "references": listed,
            },
        });
        let answered = self
            .plugin
            .call("invoke", fields, &inputs, self.plugin.invoke)
            .and_then(|done| match &done.answer {
                Answer::Completed { .. } => self.keep(sent, &done),
                Answer::Failed {
                    class,
                    accepted,
                    message,
                } => Err(PluginError::Failed {
                    class: *class,
                    accepted: *accepted,
                    message: message.clone(),
                }),
                Answer::Pending { .. } => Err(PluginError::Unexpected(
                    "a job to follow, which images do not take",
                )),
                Answer::Ok { .. } | Answer::Unknown => Err(PluginError::Unexpected("no image")),
            });
        if let Err(error) = &answered
            && error.certainly_not_accepted()
        {
            self.record_refusal(&request.artifact_id, &request.input_hash);
        }
        answered.map(Invocation::Completed)
    }

    fn reconcile(
        &mut self,
        _provider_job_id: &ProviderJobId,
    ) -> Result<Reconciliation<GeneratedCandidate>, PluginError> {
        // Image calls answer at once; no job handle is ever recorded.
        Ok(Reconciliation::NotFound)
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Reconciliation<GeneratedCandidate>, PluginError> {
        let (artifact_id, input_hash) = parse_candidate_key(idempotency_key)
            .map_err(|_| PluginError::Receipt("idempotency key is not a candidate key"))?;
        if let Some(output) = self.completed_from_receipt(&artifact_id, &input_hash)? {
            return Ok(Reconciliation::Completed {
                output,
                provider_job_id: None,
            });
        }
        // A call never sent is not asked about; one sent may have been kept
        // by the plugin. One already proven refused needs no asking.
        if self.refused(&artifact_id, &input_hash) {
            return Ok(Reconciliation::NotFound);
        }
        let Some(sent) = self.sent(&artifact_id, &input_hash)? else {
            return Ok(Reconciliation::NotFound);
        };
        let done = self.plugin.call(
            "reconcile",
            json!({
                "capability": "image.generate",
                "idempotency_key": idempotency_key.as_str(),
            }),
            &[],
            Limits::QUICK,
        )?;
        match &done.answer {
            Answer::Completed { .. } => Ok(Reconciliation::Completed {
                output: self.keep(sent, &done)?,
                provider_job_id: None,
            }),
            Answer::Failed {
                accepted: Some(false),
                ..
            } => {
                self.record_refusal(&artifact_id, &input_hash);
                Ok(Reconciliation::NotFound)
            }
            Answer::Pending { .. } => Err(PluginError::Unexpected(
                "a job to follow, which images do not take",
            )),
            Answer::Failed { .. } | Answer::Unknown | Answer::Ok { .. } => {
                Ok(Reconciliation::NotFound)
            }
        }
    }

    fn classify_error(&self, error: &PluginError) -> ErrorClass {
        error.class()
    }
}

impl CandidateGenerator for PluginImages<'_> {
    fn provider(&self) -> ProviderKey {
        ProviderKey::new(self.plugin.key.clone()).expect("a plugin key is a provider key")
    }

    fn model(&self) -> String {
        self.plugin.image_model().unwrap_or_default().to_owned()
    }

    fn policy(&self) -> JobRuntimePolicy {
        JobRuntimePolicy {
            execution: ProviderExecutionPolicy::new(
                NonZeroU16::new(3).expect("static concurrency is non-zero"),
                BackoffPolicy::new(2_000, 30_000, 3).expect("static backoff policy is valid"),
            ),
            // Generation is an explicit, chosen spend; nothing falls back to it.
            fallback_policy: FallbackPolicy {
                paid_fallback: PaidFallback::Forbidden,
                max_paid_amount_micros: Some(0),
                require_user_approval: true,
            },
        }
    }

    fn needs_prompt(&self) -> bool {
        true
    }

    fn capabilities(&self) -> ImageCapabilities {
        self.plugin
            .image_capabilities()
            .cloned()
            .unwrap_or_default()
    }

    fn can_invoke(&self) -> bool {
        self.plugin.image.is_some()
    }

    fn recover(&mut self, request: &CandidateRequest) -> Result<GeneratedCandidate, PluginError> {
        self.completed_from_receipt(&request.artifact_id, &request.input_hash)?
            .ok_or(PluginError::Receipt("a finished call has no receipt"))
    }

    fn may_call_again(&self, request: &CandidateRequest) -> bool {
        // A kept answer is used without a call; a refusal proves nothing was
        // charged.
        matches!(
            self.completed_from_receipt(&request.artifact_id, &request.input_hash),
            Ok(Some(_))
        ) || self.refused(&request.artifact_id, &request.input_hash)
    }

    fn certainly_not_charged(&self, error: &PluginError) -> bool {
        error.certainly_not_accepted()
    }

    fn release(&mut self, request: &CandidateRequest) {
        // Best effort: records left behind are never read again once their
        // candidate is acknowledged.
        for suffix in ["", ".sent", ".refused"] {
            let _ = self.remove(&request.artifact_id, suffix);
        }
    }
}

/// A plugin's speech.
#[derive(Debug, Clone, Copy)]
pub struct PluginSpeech<'a> {
    plugin: &'a PluginProvider,
}

impl<'a> PluginSpeech<'a> {
    #[must_use]
    pub const fn new(plugin: &'a PluginProvider) -> Self {
        Self { plugin }
    }
}

impl SpeechGenerator for PluginSpeech<'_> {
    type Error = PluginError;

    fn provider(&self) -> &str {
        &self.plugin.key
    }

    fn model(&self) -> &str {
        self.plugin.speech_model().unwrap_or_default()
    }

    fn capabilities(&self) -> SpeechCapabilities {
        self.plugin
            .speech_capabilities()
            .cloned()
            .unwrap_or_default()
    }

    fn voices(&self) -> Result<Vec<Voice>, PluginError> {
        if self.plugin.speech.is_none() {
            return Err(PluginError::Unsupported("the plugin does not speak"));
        }
        let done = self.plugin.call("voices", json!({}), &[], Limits::QUICK)?;
        let Answer::Ok { body } = done.answer else {
            return Err(PluginError::Unexpected("something other than its voices"));
        };
        body["voices"]
            .as_array()
            .ok_or(PluginError::Unexpected("no list of voices"))?
            .iter()
            .map(|voice| {
                let text = |field: &str| voice[field].as_str().map(str::to_owned);
                Ok(Voice {
                    id: text("id")
                        .filter(|id| !id.is_empty())
                        .ok_or(PluginError::Unexpected("a voice without an ID"))?,
                    name: text("name").unwrap_or_default(),
                    description: text("description").unwrap_or_default(),
                })
            })
            .collect()
    }

    fn synthesize(&self, request: &SpeechRequest) -> Result<Spoken, PluginError> {
        if self.plugin.speech.is_none() {
            return Err(PluginError::Unsupported("the plugin does not speak"));
        }
        let fields = json!({
            "capability": "tts.speak",
            "request": {
                "text": request.text,
                "voice_id": request.voice.voice_id,
                "language": request.voice.language,
                "emotion": request.voice.emotion.map(Emotion::name),
                "speed_permille": request.voice.speed_permille,
                "pitch_semitones": request.voice.pitch_semitones,
                "format": request.format.extension(),
            },
        });
        let done = self
            .plugin
            .call("invoke", fields, &[], self.plugin.invoke)?;
        match done.answer {
            Answer::Completed { metadata, .. } => {
                let [file] = done.files.as_slice() else {
                    return Err(PluginError::Unexpected("other than one recording"));
                };
                let duration_ms = metadata["duration_ms"]
                    .as_u64()
                    .and_then(|ms| u32::try_from(ms).ok())
                    .filter(|ms| *ms > 0)
                    .ok_or(PluginError::Unexpected("speech without its length"))?;
                Ok(Spoken {
                    audio: file.bytes.clone(),
                    format: request.format,
                    duration_ms,
                    sentences: Vec::new(),
                })
            }
            Answer::Failed {
                class,
                accepted,
                message,
            } => Err(PluginError::Failed {
                class,
                accepted,
                message,
            }),
            Answer::Pending { .. } => Err(PluginError::Unexpected(
                "a job to follow, which speech does not take",
            )),
            Answer::Ok { .. } | Answer::Unknown => Err(PluginError::Unexpected("no speech")),
        }
    }

    fn class(&self, error: &PluginError) -> ErrorClass {
        error.class()
    }

    fn certainly_not_accepted(&self, error: &PluginError) -> bool {
        error.certainly_not_accepted()
    }
}

/// Whether `bytes` read as an MP4: an ISO base media file whose first box
/// is `ftyp`.
fn is_mp4(bytes: &[u8]) -> bool {
    bytes.get(4..8) == Some(b"ftyp")
}

/// A plugin's videos. Owned, so a watcher thread can follow a job with it.
/// Its jobs are kept under `plugin.<id>-video`, apart from its images, as
/// MiniMax's are.
#[derive(Debug)]
pub struct PluginVideos {
    plugin: Arc<PluginProvider>,
    key: String,
    /// Clips a query found finished, until they are fetched.
    finished: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl PluginVideos {
    #[must_use]
    pub fn new(plugin: Arc<PluginProvider>) -> Self {
        Self {
            key: video_key(plugin.key()),
            plugin,
            finished: Mutex::new(BTreeMap::new()),
        }
    }

    fn finished(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Vec<u8>>> {
        self.finished
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// The key a plugin's video jobs are kept under.
#[must_use]
pub fn video_key(plugin_key: &str) -> String {
    format!("{plugin_key}-video")
}

impl VideoGenerator for PluginVideos {
    type Error = PluginError;

    fn provider(&self) -> &str {
        &self.key
    }

    fn model(&self) -> &str {
        self.plugin.video_model().unwrap_or_default()
    }

    fn capabilities(&self) -> VideoCapabilities {
        self.plugin
            .video_capabilities()
            .cloned()
            .unwrap_or_default()
    }

    fn canonical_params(&self, clip_seconds: u8, lines: Option<u16>) -> String {
        match lines {
            Some(lines) => format!("duration={clip_seconds};lines={lines}"),
            None => format!("duration={clip_seconds}"),
        }
    }

    fn submit(&self, request: &VideoSubmission) -> Result<String, PluginError> {
        if self.plugin.video.is_none() {
            return Err(PluginError::Unsupported("the plugin makes no videos"));
        }
        let Some((frame, format)) = &request.first_frame else {
            return Err(PluginError::Unsupported(
                "a plugin's video starts from an image",
            ));
        };
        let name = format!("first-frame.{}", extension(*format));
        let mut fields = json!({
            "capability": "video.generate",
            "request": {
                "prompt": request.prompt,
                "clip_seconds": request.clip_seconds,
                "lines": request.lines,
                "first_frame": format!("inputs/{name}"),
            },
        });
        let mut inputs = vec![Input {
            name,
            bytes: frame.clone(),
        }];
        if let Some((frame, format)) = &request.last_frame {
            let name = format!("last-frame.{}", extension(*format));
            fields["request"]["last_frame"] = json!(format!("inputs/{name}"));
            inputs.push(Input {
                name,
                bytes: frame.clone(),
            });
        }
        let done = self
            .plugin
            .call("invoke", fields, &inputs, self.plugin.invoke)?;
        match done.answer {
            Answer::Pending { job } if !job.trim().is_empty() && job.len() <= 256 => Ok(job),
            Answer::Failed {
                class,
                accepted,
                message,
            } => Err(PluginError::Failed {
                class,
                accepted,
                message,
            }),
            // It may have been taken and charged: unresolved, not retried.
            _ => Err(PluginError::Unexpected(
                "something other than a job to follow for a video",
            )),
        }
    }

    fn query(&self, task: &ProviderJobId) -> Result<TaskState, PluginError> {
        let done = self.plugin.call(
            "reconcile",
            json!({"capability": "video.generate", "job": task.as_str()}),
            &[],
            self.plugin.invoke,
        )?;
        match done.answer {
            Answer::Pending { .. } => Ok(TaskState::Pending),
            Answer::Completed { .. } => {
                let [file] = done.files.as_slice() else {
                    return Err(PluginError::Unexpected("other than one clip"));
                };
                if !is_mp4(&file.bytes) {
                    return Err(PluginError::Unexpected("a clip that is not an MP4"));
                }
                self.finished()
                    .insert(task.as_str().to_owned(), file.bytes.clone());
                Ok(TaskState::Succeeded {
                    file: task.as_str().to_owned(),
                })
            }
            Answer::Failed {
                class,
                accepted,
                message,
            } => Err(PluginError::Failed {
                class,
                accepted,
                message,
            }),
            Answer::Unknown => Err(PluginError::Unexpected("that it has no record of the job")),
            Answer::Ok { .. } => Err(PluginError::Unexpected("no state of the job")),
        }
    }

    fn fetch(&self, file: &str) -> Result<Vec<u8>, PluginError> {
        if let Some(bytes) = self.finished().remove(file) {
            return Ok(bytes);
        }
        // Found finished by another follower: ask again, which is free.
        let task =
            ProviderJobId::new(file).map_err(|_| PluginError::Receipt("a job ID changed shape"))?;
        match self.query(&task)? {
            TaskState::Succeeded { .. } => self
                .finished()
                .remove(file)
                .ok_or(PluginError::Unexpected("no clip")),
            TaskState::Pending => Err(PluginError::Unexpected("a finished job as unfinished")),
        }
    }

    fn class(&self, error: &PluginError) -> ErrorClass {
        error.class()
    }

    fn certainly_not_accepted(&self, error: &PluginError) -> bool {
        error.certainly_not_accepted()
    }
}
