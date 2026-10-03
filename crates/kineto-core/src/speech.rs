//! Speech: text-to-speech providers chosen by what they declare they can
//! do, and paid synthesis as durable write-ahead jobs.
//!
//! Providers are [`SpeechGenerator`]s: what they declare
//! ([`SpeechCapabilities`]), the voices they offer (free) and synthesis (the
//! paid call). Speech arrives in the provider's answer, so a call runs as a
//! paid image does, in a job partition of its provider and kind of work (a
//! voice audition, a line):
//!
//! ```text
//! prepare -> receipt (plan) -> speak (paid) -> audio + receipt -> invoked
//!         -> attach -> acknowledge -> drop receipt
//! ```
//!
//! A restart finds a receipt that names the audio on disk and records the
//! speech from it instead of paying again. A call that was attempted but left
//! no audio is made again only when its receipt proves the provider certainly
//! did not accept that attempt. Otherwise it may have been charged, and
//! speech in its numbering is refused until the director chooses to risk
//! paying twice.
//!
//! The local demo speaks every line as a tone, free and the same every time:
//! a WAV whose sentences last about as long as a person would take to say
//! them. A project without a key goes through voice casting, dubbing,
//! locking and export with it.

use std::{convert::Infallible, error::Error, fmt, marker::PhantomData, num::NonZeroU16};

use kineto_job_runtime::{
    DispatchOutcome, Invocation, InvokePreparedError, JobRuntime, JobRuntimePolicy, PaidJobAdapter,
    PrepareOutcome, ProviderKey, Reconciliation, RuntimeIntent, StartupOutcome,
};
use kineto_jobs::{
    BackoffPolicy, CostEstimate, CurrencyCode, ErrorClass, FallbackPolicy, IdempotencyKey,
    IntentState, JobId, PaidFallback, ProviderExecutionPolicy, ProviderJobId,
};
use kineto_project::{
    ArtifactId, ContentHash, InputHash,
    fs::{ProjectFsError, ProjectRelativePath, ProjectRoot},
    manifest::CanonicalProject,
    shot::DEMO_PROVIDER,
};
use kineto_providers::{
    AudioFormat, Emotion, GapSeverity, Price, SpeechCapabilities, SpeechRequirements, SpeechTiming,
    VoiceSource, evaluate_speech, secret::SecretSource,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    generation::{
        GenerationError, NoHooks, ProviderSelectionError, invoke_when_permitted, parse_job_key,
    },
    paid_image::Submission,
};

/// The capability speech is made with.
pub const SPEECH_OPERATION: &str = "speech.synthesize";
/// Provider key the local demo's speech is recorded under.
pub const DEMO_SPEECH_PROVIDER: &str = "local-demo-speech";
/// The model the local demo names: a tone for every voice.
pub const DEMO_SPEECH_MODEL: &str = "demo-tone";
const DEMO_SAMPLE_RATE: u32 = 16_000;
/// The demo's voices: an ID, a name, a description and the tone's pitch in
/// hertz.
const DEMO_VOICES: [(&str, &str, &str, u32); 3] = [
    ("demo_low", "Low", "A low tone, for a deep voice.", 110),
    ("demo_middle", "Middle", "A middle tone.", 165),
    ("demo_high", "High", "A high tone, for a light voice.", 220),
];
/// Languages the demo speaks, which is every one: it says nothing.
const DEMO_LANGUAGES: [&str; 12] = [
    "ar", "de", "en", "es", "fr", "hi", "it", "ja", "ko", "pt", "ru", "zh",
];
const RECEIPT_SCHEMA_VERSION: u32 = 1;
/// The normal speaking rate, in thousandths.
pub const NORMAL_SPEED: u16 = 1000;

/// A voice a provider offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// How a chosen voice speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSettings {
    pub voice_id: String,
    /// ISO 639-1 code of the language it speaks.
    pub language: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "emotion_name"
    )]
    pub emotion: Option<Emotion>,
    /// In thousandths of the provider's normal rate.
    pub speed_permille: u16,
    pub pitch_semitones: i8,
}

impl VoiceSettings {
    /// A voice from a provider's list, at its normal rate and pitch.
    #[must_use]
    pub fn new(voice_id: impl Into<String>, language: impl Into<String>) -> Self {
        Self {
            voice_id: voice_id.into(),
            language: language.into(),
            emotion: None,
            speed_permille: NORMAL_SPEED,
            pitch_semitones: 0,
        }
    }

    /// What speaking with this voice needs of a provider.
    #[must_use]
    pub fn requirements(&self) -> SpeechRequirements {
        SpeechRequirements {
            voice_source: VoiceSource::Catalog,
            language: self.language.clone(),
            emotion: self.emotion,
            speed_permille: self.speed_permille,
            pitch_semitones: self.pitch_semitones,
            preferred_timing: SpeechTiming::Sentence,
            requires_synchronous: true,
        }
    }

    /// The voice as it enters an input hash.
    #[must_use]
    pub fn canonical(&self) -> String {
        format!(
            "voice={};language={};emotion={};speed={};pitch={}",
            self.voice_id,
            self.language,
            self.emotion.map_or("", Emotion::name),
            self.speed_permille,
            self.pitch_semitones
        )
    }
}

mod emotion_name {
    use kineto_providers::Emotion;
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    #[allow(
        clippy::ref_option,
        reason = "serde's `with` passes the field by reference"
    )]
    pub fn serialize<S: Serializer>(
        emotion: &Option<Emotion>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match emotion {
            Some(emotion) => serializer.serialize_some(emotion.name()),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Emotion>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|name| {
                Emotion::parse(&name)
                    .ok_or_else(|| D::Error::custom(format!("unknown emotion {name:?}")))
            })
            .transpose()
    }
}

/// What one call asks a provider to speak.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechRequest {
    pub text: String,
    pub voice: VoiceSettings,
    /// One of the formats the provider declares.
    pub format: AudioFormat,
}

/// A stretch of what was spoken, and when it was heard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpokenSpan {
    pub text: String,
    pub start_ms: u32,
    pub end_ms: u32,
}

/// What a provider answered.
#[derive(Clone, PartialEq, Eq)]
pub struct Spoken {
    pub audio: Vec<u8>,
    pub format: AudioFormat,
    /// The audio's length, as the provider measured it.
    pub duration_ms: u32,
    /// Each sentence's timing, when the provider reports it.
    pub sentences: Vec<SpokenSpan>,
}

impl fmt::Debug for Spoken {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Spoken")
            .field("audio", &self.audio.len())
            .field("format", &self.format)
            .field("duration_ms", &self.duration_ms)
            .field("sentences", &self.sentences)
            .finish()
    }
}

/// A text-to-speech provider: what it can do, its voices and its paid
/// synthesis.
pub trait SpeechGenerator {
    type Error: Error + Send + Sync + 'static;

    /// The key its speech is recorded under, which the audio it makes names
    /// as its provider.
    fn provider(&self) -> &'static str;
    fn model(&self) -> &'static str;
    fn capabilities(&self) -> SpeechCapabilities;
    /// The voices it offers. Free.
    fn voices(&self) -> Result<Vec<Voice>, Self::Error>;
    /// The parameters a call with `voice` in `format` is made with, as they
    /// enter the input hash.
    fn canonical_params(&self, voice: &VoiceSettings, format: AudioFormat) -> String {
        format!("{};format={}", voice.canonical(), format.extension())
    }
    /// Speak: the paid call.
    fn synthesize(&self, request: &SpeechRequest) -> Result<Spoken, Self::Error>;
    fn class(&self, error: &Self::Error) -> ErrorClass;
    /// Whether `error` proves the provider did not accept the call, so it
    /// cannot have been charged.
    fn certainly_not_accepted(&self, error: &Self::Error) -> bool;
}

/// The local demo: free, offline, and the same for the same line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalDemoVoices;

/// What the local demo can do: every delivery, rate and pitch a real
/// provider might, so a voice chosen with it is kept as chosen.
#[must_use]
pub fn demo_speech_capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        voice_sources: [VoiceSource::Catalog].into_iter().collect(),
        languages: DEMO_LANGUAGES.into_iter().map(str::to_owned).collect(),
        emotions: Emotion::ALL.into_iter().collect(),
        speed_permille: Some((500, 2000)),
        pitch_semitones: Some((-12, 12)),
        timing: SpeechTiming::Sentence,
        async_jobs: false,
        output_formats: [AudioFormat::Wav].into_iter().collect(),
        max_text_chars: Some(9_999),
        price_per_character: Some(Price::usd(0)),
    }
}

impl SpeechGenerator for LocalDemoVoices {
    type Error = Infallible;

    fn provider(&self) -> &'static str {
        DEMO_SPEECH_PROVIDER
    }

    fn model(&self) -> &'static str {
        DEMO_SPEECH_MODEL
    }

    fn capabilities(&self) -> SpeechCapabilities {
        demo_speech_capabilities()
    }

    fn voices(&self) -> Result<Vec<Voice>, Infallible> {
        Ok(DEMO_VOICES
            .iter()
            .map(|(id, name, description, _)| Voice {
                id: (*id).to_owned(),
                name: (*name).to_owned(),
                description: (*description).to_owned(),
            })
            .collect())
    }

    fn synthesize(&self, request: &SpeechRequest) -> Result<Spoken, Infallible> {
        let voice = &request.voice;
        let hertz = DEMO_VOICES
            .iter()
            .find(|(id, ..)| *id == voice.voice_id)
            .map_or(DEMO_VOICES[1].3, |(.., hertz)| *hertz);
        let sentences = sentence_timing(&request.text, voice.speed_permille);
        let duration_ms = sentences.last().map_or(0, |span| span.end_ms);
        Ok(Spoken {
            audio: demo_wav(
                &sentences,
                duration_ms,
                pitched_millihertz(hertz, voice.pitch_semitones),
                loudness(voice.emotion),
            ),
            format: AudioFormat::Wav,
            duration_ms,
            sentences,
        })
    }

    fn class(&self, error: &Infallible) -> ErrorClass {
        match *error {}
    }

    fn certainly_not_accepted(&self, error: &Infallible) -> bool {
        match *error {}
    }
}

/// How loud the demo's tone is for a delivery, in thousandths of full
/// scale.
const fn loudness(emotion: Option<Emotion>) -> u32 {
    match emotion {
        Some(Emotion::Whisper) => 80,
        Some(Emotion::Sad | Emotion::Fearful | Emotion::Disgusted) => 250,
        None | Some(Emotion::Calm | Emotion::Fluent) => 300,
        Some(Emotion::Happy | Emotion::Surprised) => 450,
        Some(Emotion::Angry) => 600,
    }
}

/// `hertz` shifted by `semitones`, in millihertz, by equal temperament in
/// integers so the demo's audio is the same on every platform.
fn pitched_millihertz(hertz: u32, semitones: i8) -> u64 {
    // 2^(n/12) for n = 0..12, in thousandths.
    const STEPS: [u64; 12] = [
        1000, 1059, 1122, 1189, 1260, 1335, 1414, 1498, 1587, 1682, 1782, 1888,
    ];
    let semitones = i32::from(semitones);
    let step = STEPS[usize::try_from(semitones.rem_euclid(12)).expect("in 0..12")];
    let millihertz = u64::from(hertz) * step;
    match semitones.div_euclid(12) {
        octaves @ 0.. => millihertz << octaves.unsigned_abs(),
        octaves => millihertz >> octaves.unsigned_abs(),
    }
}

/// A mono 16-bit PCM WAV: a triangle tone at `millihertz` during each
/// sentence, with 10 ms fades, and silence between them.
fn demo_wav(sentences: &[SpokenSpan], duration_ms: u32, millihertz: u64, loudness: u32) -> Vec<u8> {
    let samples_per_ms = DEMO_SAMPLE_RATE / 1000;
    let total = duration_ms * samples_per_ms;
    let fade = 10 * samples_per_ms;
    let step = (millihertz << 32) / (u64::from(DEMO_SAMPLE_RATE) * 1000);
    let mut pcm = Vec::with_capacity(usize::try_from(total).unwrap_or(0) * 2);
    let mut phase: u64 = 0;
    let mut spans = sentences.iter().peekable();
    for index in 0..total {
        while spans
            .peek()
            .is_some_and(|span| span.end_ms * samples_per_ms <= index)
        {
            spans.next();
        }
        let level = spans.peek().map_or(0, |span| {
            let (start, end) = (span.start_ms * samples_per_ms, span.end_ms * samples_per_ms);
            if index < start {
                0
            } else {
                let edge = (index - start).min(end - index).min(fade);
                i64::from(loudness) * i64::from(edge) / i64::from(fade.max(1))
            }
        });
        let position = i64::try_from((phase >> 16) & 0xffff).expect("16 bits");
        let triangle = if position < 32_768 {
            position * 2 - 32_768
        } else {
            (65_535 - position) * 2 - 32_768
        };
        let sample = i16::try_from(triangle * level / 1000).expect("within full scale");
        pcm.extend_from_slice(&sample.to_le_bytes());
        phase = (phase + step) & 0xffff_ffff;
    }
    let data = u32::try_from(pcm.len()).expect("a line fits a WAV");
    let mut wav = Vec::with_capacity(pcm.len() + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1_u16.to_le_bytes()); // mono
    wav.extend_from_slice(&DEMO_SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(DEMO_SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2_u16.to_le_bytes());
    wav.extend_from_slice(&16_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data.to_le_bytes());
    wav.extend_from_slice(&pcm);
    wav
}

/// Time a word takes at the normal rate.
const WORD_MS: u32 = 380;
/// Time a Chinese or Japanese character takes at the normal rate.
const CJK_CHARACTER_MS: u32 = 250;
/// The breath between two sentences.
const PAUSE_MS: u32 = 250;
/// No sentence is said faster than this.
const SHORTEST_SENTENCE_MS: u32 = 300;

fn is_cjk(character: char) -> bool {
    matches!(character,
        '\u{3040}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}'
        | '\u{f900}'..='\u{faff}')
}

/// How long a sentence takes to say at the normal rate: about 2.6 words a
/// second, or 4 Chinese or Japanese characters.
fn sentence_ms(sentence: &str) -> u32 {
    let mut words = 0;
    let mut cjk = 0;
    let mut in_word = false;
    for character in sentence.chars() {
        if is_cjk(character) {
            cjk += 1;
            in_word = false;
        } else if character.is_alphanumeric() {
            if !in_word {
                words += 1;
            }
            in_word = true;
        } else {
            in_word = character == '\'' || character == '’';
        }
    }
    (words * WORD_MS + cjk * CJK_CHARACTER_MS).max(SHORTEST_SENTENCE_MS)
}

/// The text's sentences, each with its ending punctuation and any closing
/// quote after it.
#[must_use]
pub fn sentences(text: &str) -> Vec<String> {
    const ENDINGS: [char; 7] = ['.', '!', '?', '…', '。', '！', '？'];
    const CLOSINGS: [char; 7] = ['"', '\'', '”', '’', ')', '」', '』'];
    let mut found = Vec::new();
    let mut current = String::new();
    let mut characters = text.chars().peekable();
    while let Some(character) = characters.next() {
        current.push(character);
        if !ENDINGS.contains(&character) {
            continue;
        }
        let mut full_width = matches!(character, '。' | '！' | '？');
        while let Some(&next) = characters.peek() {
            if ENDINGS.contains(&next) || CLOSINGS.contains(&next) {
                full_width |= matches!(next, '。' | '！' | '？');
                current.push(next);
                characters.next();
            } else {
                break;
            }
        }
        if full_width || characters.peek().is_none_or(|next| next.is_whitespace()) {
            let sentence = current.trim();
            if !sentence.is_empty() {
                found.push(sentence.to_owned());
            }
            current.clear();
        }
    }
    let rest = current.trim();
    if !rest.is_empty() {
        found.push(rest.to_owned());
    }
    found
}

/// When each sentence of `text` would be heard at `speed_permille`, before
/// it is spoken.
#[must_use]
pub fn sentence_timing(text: &str, speed_permille: u16) -> Vec<SpokenSpan> {
    let speed = u32::from(speed_permille.max(1));
    let scaled = |ms: u32| ms * NORMAL_SPEED as u32 / speed;
    let mut start = 0;
    sentences(text)
        .into_iter()
        .enumerate()
        .map(|(index, text)| {
            if index > 0 {
                start += scaled(PAUSE_MS);
            }
            let end = start + scaled(sentence_ms(&text));
            let span = SpokenSpan {
                text,
                start_ms: start,
                end_ms: end,
            };
            start = end;
            span
        })
        .collect()
}

/// How long saying `text` takes at `speed_permille`, before it is spoken.
#[must_use]
pub fn estimate_duration_ms(text: &str, speed_permille: u16) -> u32 {
    sentence_timing(text, speed_permille)
        .last()
        .map_or(0, |span| span.end_ms)
}

/// Which provider a session speaks with.
#[derive(Debug, Clone, Default)]
pub enum SpeechProvider {
    /// A tone for every line. Free.
    #[default]
    LocalDemo,
}

/// Which speech provider a session speaks with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechProviderKind {
    LocalDemo,
}

/// What the director needs to know about the session's speech provider
/// before asking for speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpeechProviderInfo {
    pub kind: SpeechProviderKind,
    /// Whether speech can be asked for: a paid provider needs credentials.
    pub ready: bool,
    /// What a character of text costs, in USD micros; `None` when unknown.
    pub character_cost_micros: Option<u64>,
}

impl SpeechProvider {
    /// The provider named by `selection`: `local-demo` for the free demo.
    /// `None` when nothing is named, so the session follows its image
    /// provider.
    pub fn select(
        selection: Option<&str>,
        _secrets: &impl SecretSource,
    ) -> Result<Option<Self>, ProviderSelectionError> {
        match selection.map(str::trim) {
            None | Some("") => Ok(None),
            Some(DEMO_PROVIDER) => Ok(Some(Self::LocalDemo)),
            Some(other) => Err(ProviderSelectionError::Unknown(other.to_owned())),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> SpeechProviderKind {
        match self {
            Self::LocalDemo => SpeechProviderKind::LocalDemo,
        }
    }

    #[must_use]
    pub fn capabilities(&self) -> SpeechCapabilities {
        match self {
            Self::LocalDemo => demo_speech_capabilities(),
        }
    }

    #[must_use]
    pub fn info(&self) -> SpeechProviderInfo {
        SpeechProviderInfo {
            kind: self.kind(),
            ready: true,
            character_cost_micros: self
                .capabilities()
                .price_per_character
                .map(|price| price.amount_micros),
        }
    }

    /// What speaking `text` costs, in USD micros; `None` when unknown.
    #[must_use]
    pub fn estimate(&self, text: &str) -> Option<u64> {
        self.capabilities().cost_micros(text.chars().count())
    }
}

/// Whether `capabilities` can speak `text` with `voice` as it was chosen,
/// and the format to ask for. A provider that would change the voice, or
/// cannot take the text, is refused before anything is written.
pub fn check_speech(
    capabilities: &SpeechCapabilities,
    voice: &VoiceSettings,
    text: &str,
) -> Result<AudioFormat, GenerationError> {
    let mut missing: Vec<&'static str> = evaluate_speech(&voice.requirements(), capabilities)
        .gaps
        .into_iter()
        .filter(|gap| gap.severity == GapSeverity::Incompatible)
        .map(|gap| gap.capability)
        .collect();
    if capabilities
        .max_text_chars
        .is_some_and(|longest| text.chars().count() > longest)
    {
        missing.push("speech.max_text_chars");
    }
    let format = capabilities.preferred_format();
    if format.is_none() {
        missing.push("speech.output_formats");
    }
    match format {
        Some(format) if missing.is_empty() => Ok(format),
        _ => Err(GenerationError::Incapable(missing)),
    }
}

/// Where a durable step of speech was reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechPoint {
    /// The write-ahead intent and the receipt's plan are on disk.
    Prepared,
    /// The provider answered; nothing of its answer is written yet.
    Answered,
    /// The audio and the receipt naming it are on disk; the intent does not
    /// say so yet.
    Written,
    /// The intent is invoked; canonical state has not changed.
    Invoked,
    /// The speech is in canonical state; its intent is not acknowledged yet.
    Attached,
}

pub trait SpeechHooks {
    fn reached(&mut self, point: SpeechPoint);
}

impl SpeechHooks for NoHooks {
    fn reached(&mut self, _point: SpeechPoint) {}
}

/// Speech on disk, as its record keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenOutput {
    pub artifact_id: ArtifactId,
    pub input_hash: InputHash,
    pub provider: String,
    pub model: String,
    /// The parameters it was asked with.
    pub parameters: String,
    pub media_path: ProjectRelativePath,
    pub content_hash: ContentHash,
    pub format: AudioFormat,
    pub duration_ms: u32,
    pub sentences: Vec<SpokenSpan>,
}

/// Where one kind of speech is recorded.
pub trait SpeechLedger {
    /// What the canonical record needs beyond the speech itself.
    type Plan: Serialize + DeserializeOwned;

    /// The kind of work, which with the provider names its job partition,
    /// apart from every other kind.
    const KIND: &'static str;

    fn artifact_id(plan: &Self::Plan) -> &str;

    fn input_hash(plan: &Self::Plan) -> &str;

    /// The project-relative directory its audio is written in.
    fn media_dir(plan: &Self::Plan) -> String;

    /// Record the speech in canonical state. Idempotent.
    fn attach(
        project: &CanonicalProject,
        plan: &Self::Plan,
        spoken: &SpokenOutput,
    ) -> Result<(), GenerationError>;
}

/// A speech job that cannot run or be completed.
#[derive(Debug)]
pub enum SpeechJobError<E> {
    /// The provider refused or failed.
    Provider(E),
    /// No provider is at hand, so no call can be made.
    NotConfigured,
    /// A receipt or its audio is missing, unreadable or does not match.
    /// Nothing is made again; a person needs to look.
    Receipt(&'static str),
    /// Writing down the provider's answer failed.
    Fs(ProjectFsError),
}

impl<E: fmt::Display> fmt::Display for SpeechJobError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider(error) => error.fmt(formatter),
            Self::NotConfigured => formatter.write_str("no speech provider is configured"),
            Self::Receipt(what) => write!(formatter, "speech receipt: {what}"),
            Self::Fs(error) => error.fmt(formatter),
        }
    }
}

impl<E: Error + 'static> Error for SpeechJobError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Provider(error) => Some(error),
            Self::Fs(error) => Some(error),
            Self::NotConfigured | Self::Receipt(_) => None,
        }
    }
}

/// What is kept of a speech call until its speech is acknowledged. Runtime
/// state, never canonical.
#[derive(Debug, Serialize, Deserialize)]
struct Receipt {
    schema_version: u32,
    artifact_id: String,
    input_hash: String,
    provider: String,
    model: String,
    parameters: String,
    /// What the ledger records the speech with.
    plan: serde_json::Value,
    /// The attempt the provider certainly did not accept. Any other attempt
    /// that left no audio may have been charged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    refused_attempt: Option<u16>,
    /// Set once the audio is on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    spoken: Option<SpokenRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SpokenRecord {
    media_path: String,
    content_hash: String,
    format: String,
    duration_ms: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    sentences: Vec<SpokenSpan>,
}

fn receipt_path(artifact_id: &str) -> Result<ProjectRelativePath, ProjectFsError> {
    ProjectRelativePath::new(format!(".kineto/generation/{artifact_id}.json")).map_err(|_| {
        ProjectFsError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a speech ID is not a file name",
        ))
    })
}

/// A project path as its records write it.
fn path_text(path: &ProjectRelativePath) -> String {
    path.as_path().to_string_lossy().into_owned()
}

fn write_receipt(root: &ProjectRoot, receipt: &Receipt) -> Result<(), ProjectFsError> {
    let mut bytes = serde_json::to_vec_pretty(receipt).map_err(|error| {
        ProjectFsError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, error))
    })?;
    bytes.push(b'\n');
    root.write_atomic(&receipt_path(&receipt.artifact_id)?, &bytes)
}

/// The receipt of `artifact_id` for `input_hash`; `None` when there is none
/// or it is for other input.
fn read_receipt(root: &ProjectRoot, artifact_id: &str, input_hash: &str) -> Option<Receipt> {
    let bytes = root.read(&receipt_path(artifact_id).ok()?).ok()?;
    serde_json::from_slice::<Receipt>(&bytes)
        .ok()
        .filter(|receipt| {
            receipt.schema_version == RECEIPT_SCHEMA_VERSION
                && receipt.artifact_id == artifact_id
                && receipt.input_hash == input_hash
        })
}

fn remove_receipt(root: &ProjectRoot, artifact_id: &str) {
    // Best effort: a receipt left behind is never read again once its job
    // is acknowledged or retired.
    if let Ok(path) = receipt_path(artifact_id) {
        let _ = std::fs::remove_file(root.resolve(&path));
    }
}

fn format_named(name: &str) -> Option<AudioFormat> {
    [AudioFormat::Wav, AudioFormat::Mp3, AudioFormat::Flac]
        .into_iter()
        .find(|format| format.extension() == name)
}

/// The speech a receipt names, after proving the audio on disk is the one
/// it names; `None` while no audio was written.
fn spoken_from(
    root: &ProjectRoot,
    receipt: &Receipt,
) -> Result<Option<SpokenOutput>, &'static str> {
    let Some(spoken) = &receipt.spoken else {
        return Ok(None);
    };
    let media_path =
        ProjectRelativePath::new(&spoken.media_path).map_err(|_| "its audio path is invalid")?;
    let content_hash =
        ContentHash::new(spoken.content_hash.clone()).map_err(|_| "its audio hash is invalid")?;
    let bytes = root.read(&media_path).map_err(|_| "its audio is missing")?;
    if ContentHash::of(&bytes) != content_hash {
        return Err("its audio is not what it names");
    }
    Ok(Some(SpokenOutput {
        artifact_id: ArtifactId::new(receipt.artifact_id.clone())
            .map_err(|_| "its artifact ID is invalid")?,
        input_hash: InputHash::new(receipt.input_hash.clone())
            .map_err(|_| "its input hash is invalid")?,
        provider: receipt.provider.clone(),
        model: receipt.model.clone(),
        parameters: receipt.parameters.clone(),
        media_path,
        content_hash,
        format: format_named(&spoken.format).ok_or("its audio format is unknown")?,
        duration_ms: spoken.duration_ms,
        sentences: spoken.sentences.clone(),
    }))
}

/// How the last call of a prepared speech job ended, as far as the project
/// can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LastAttempt {
    /// Nothing was called.
    None,
    /// The provider certainly did not accept it.
    Refused,
    /// The provider may have accepted it.
    Unresolved,
}

fn last_attempt(root: &ProjectRoot, record: &RuntimeIntent) -> LastAttempt {
    if record.attempts() == 0 {
        return LastAttempt::None;
    }
    let intent = record.intent();
    let refused = intent
        .artifact_id()
        .and_then(|artifact_id| {
            read_receipt(root, artifact_id.as_str(), intent.input_hash().as_str())
        })
        .is_some_and(|receipt| receipt.refused_attempt == Some(record.attempts()));
    if refused {
        LastAttempt::Refused
    } else {
        LastAttempt::Unresolved
    }
}

/// One call, as the job runtime hands it to the adapter.
struct SpeechCall {
    artifact_id: ArtifactId,
    input_hash: InputHash,
    request: SpeechRequest,
    media_dir: String,
}

/// Speech of one generator behind the paid-job contract. Without a
/// generator it completes calls from their receipts but makes none.
struct Speaker<'a, G> {
    root: &'a ProjectRoot,
    generator: Option<&'a G>,
    /// The attempt this call is, as the intent will count it.
    attempt: u16,
    hooks: &'a mut dyn SpeechHooks,
}

impl<G: SpeechGenerator> Speaker<'_, G> {
    fn completed(
        &self,
        artifact_id: &str,
        input_hash: &str,
    ) -> Result<Option<SpokenOutput>, SpeechJobError<G::Error>> {
        read_receipt(self.root, artifact_id, input_hash)
            .map_or(Ok(None), |receipt| spoken_from(self.root, &receipt))
            .map_err(SpeechJobError::Receipt)
    }

    fn record(&self, artifact_id: &str, input_hash: &str, change: impl FnOnce(&mut Receipt)) {
        // Best effort for a refusal: if it cannot be written down, the
        // attempt stays unresolved, which fails closed.
        if let Some(mut receipt) = read_receipt(self.root, artifact_id, input_hash) {
            change(&mut receipt);
            let _ = write_receipt(self.root, &receipt);
        }
    }
}

impl<G: SpeechGenerator> PaidJobAdapter for Speaker<'_, G> {
    type Request = SpeechCall;
    type Output = SpokenOutput;
    type Error = SpeechJobError<G::Error>;

    fn estimate_cost(&self, call: &SpeechCall) -> Result<CostEstimate, Self::Error> {
        let generator = self.generator.ok_or(SpeechJobError::NotConfigured)?;
        let capabilities = generator.capabilities();
        let currency = capabilities
            .price_per_character
            .map_or(*b"USD", |price| price.currency);
        Ok(CostEstimate {
            call_count: 1,
            amount_micros: capabilities
                .cost_micros(call.request.text.chars().count())
                .unwrap_or(0),
            currency: CurrencyCode::new(currency).expect("declared currency is valid"),
            estimated_duration_ms: None,
        })
    }

    fn invoke(
        &mut self,
        call: &SpeechCall,
        _idempotency_key: &IdempotencyKey,
    ) -> Result<Invocation<SpokenOutput>, Self::Error> {
        let (artifact_id, input_hash) = (call.artifact_id.as_str(), call.input_hash.as_str());
        // Answered and written down earlier, but the intent never said so:
        // use that answer instead of paying for another.
        if let Some(spoken) = self.completed(artifact_id, input_hash)? {
            return Ok(Invocation::Completed(spoken));
        }
        let generator = self.generator.ok_or(SpeechJobError::NotConfigured)?;
        let spoken = match generator.synthesize(&call.request) {
            Ok(spoken) => spoken,
            Err(error) => {
                if generator.certainly_not_accepted(&error) {
                    let attempt = self.attempt;
                    self.record(artifact_id, input_hash, |receipt| {
                        receipt.refused_attempt = Some(attempt);
                    });
                }
                return Err(SpeechJobError::Provider(error));
            }
        };
        self.hooks.reached(SpeechPoint::Answered);
        let media_path = ProjectRelativePath::new(format!(
            "{}/{artifact_id}.{}",
            call.media_dir.trim_end_matches('/'),
            spoken.format.extension()
        ))
        .map_err(|_| SpeechJobError::Receipt("the audio path is invalid"))?;
        self.root
            .write_atomic(&media_path, &spoken.audio)
            .map_err(SpeechJobError::Fs)?;
        let mut receipt = read_receipt(self.root, artifact_id, input_hash)
            .ok_or(SpeechJobError::Receipt("the receipt went missing"))?;
        receipt.spoken = Some(SpokenRecord {
            media_path: path_text(&media_path),
            content_hash: ContentHash::of(&spoken.audio).as_str().to_owned(),
            format: spoken.format.extension().to_owned(),
            duration_ms: spoken.duration_ms,
            sentences: spoken.sentences,
        });
        write_receipt(self.root, &receipt).map_err(SpeechJobError::Fs)?;
        self.hooks.reached(SpeechPoint::Written);
        spoken_from(self.root, &receipt)
            .map_err(SpeechJobError::Receipt)?
            .map(Invocation::Completed)
            .ok_or(SpeechJobError::Receipt("the receipt names no audio"))
    }

    fn reconcile(
        &mut self,
        _provider_job_id: &ProviderJobId,
    ) -> Result<Reconciliation<SpokenOutput>, Self::Error> {
        // Speech arrives in the answer; no remote job is ever followed.
        Ok(Reconciliation::NotFound)
    }

    fn reconcile_by_idempotency_key(
        &mut self,
        idempotency_key: &IdempotencyKey,
    ) -> Result<Reconciliation<SpokenOutput>, Self::Error> {
        let (artifact_id, input_hash) = parse_job_key(SPEECH_OPERATION, idempotency_key)
            .map_err(|_| SpeechJobError::Receipt("the job key is not a speech key"))?;
        Ok(
            match self.completed(artifact_id.as_str(), input_hash.as_str())? {
                Some(output) => Reconciliation::Completed {
                    output,
                    provider_job_id: None,
                },
                None => Reconciliation::NotFound,
            },
        )
    }

    fn classify_error(&self, error: &Self::Error) -> ErrorClass {
        match error {
            SpeechJobError::Provider(error) => self
                .generator
                .map_or(ErrorClass::Terminal, |generator| generator.class(error)),
            // Writing the answer down failed; the disk may recover.
            SpeechJobError::Fs(_) => ErrorClass::Retryable,
            SpeechJobError::NotConfigured | SpeechJobError::Receipt(_) => ErrorClass::Terminal,
        }
    }
}

/// How speech jobs of `provider` run: a few calls at once to a paid
/// provider; the free demo answers at once, so it is not held back.
fn policy(provider: &str) -> JobRuntimePolicy {
    let concurrency = if provider == DEMO_SPEECH_PROVIDER {
        64
    } else {
        4
    };
    JobRuntimePolicy {
        execution: ProviderExecutionPolicy::new(
            NonZeroU16::new(concurrency).expect("static concurrency is non-zero"),
            BackoffPolicy::new(1_000, 30_000, 3).expect("static backoff policy is valid"),
        ),
        // Speech is an explicit, chosen spend; nothing falls back to it.
        fallback_policy: FallbackPolicy {
            paid_fallback: PaidFallback::Forbidden,
            max_paid_amount_micros: Some(0),
            require_user_approval: true,
        },
    }
}

fn runtime<'a, L: SpeechLedger>(project: &'a CanonicalProject, provider: &str) -> JobRuntime<'a> {
    let partition = ProviderKey::new(format!("{provider}-{}", L::KIND))
        .expect("static provider and kind make a valid key");
    JobRuntime::new(project, partition, policy(provider))
}

/// What finishing interrupted speech found.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SpeechRecovery {
    /// Speech recorded from its receipts, by ID.
    pub attached: Vec<String>,
    /// Speech whose call may have been accepted without its audio being
    /// kept, by ID.
    pub unresolved: Vec<String>,
    /// Records this build cannot complete; they need a person.
    pub needs_attention: usize,
}

/// Record speech whose audio arrived before an interruption, and sort out
/// the intents that never finished.
fn finish<L: SpeechLedger, G: SpeechGenerator>(
    project: &CanonicalProject,
    runtime: &JobRuntime<'_>,
    generator: Option<&G>,
) -> Result<(SpeechRecovery, Vec<(JobId, LastAttempt)>), GenerationError> {
    let root = project.root();
    let mut hooks = NoHooks;
    let mut speaker = Speaker {
        root,
        generator,
        attempt: 0,
        hooks: &mut hooks,
    };
    let mut recovery = SpeechRecovery::default();
    let mut waiting = Vec::new();
    for outcome in runtime.reconcile_startup(&mut speaker)? {
        match outcome {
            StartupOutcome::ResultReady { job_id, output } => {
                let attached = read_receipt(
                    root,
                    output.artifact_id.as_str(),
                    output.input_hash.as_str(),
                )
                .and_then(|receipt| serde_json::from_value::<L::Plan>(receipt.plan).ok())
                .ok_or(GenerationError::Unexpected("finished speech has no plan"))
                .and_then(|plan| {
                    L::attach(project, &plan, &output)?;
                    let _ = runtime.acknowledge_result(&job_id)?;
                    Ok(())
                });
                match attached {
                    Ok(()) => {
                        remove_receipt(root, output.artifact_id.as_str());
                        recovery.attached.push(job_id.as_str().to_owned());
                    }
                    Err(_) => recovery.needs_attention += 1,
                }
            }
            StartupOutcome::RetryPrepared { job_id, .. } => {
                if let Some(record) = runtime.load(&job_id)? {
                    let last = last_attempt(root, &record);
                    if last == LastAttempt::Unresolved {
                        recovery.unresolved.push(job_id.as_str().to_owned());
                    }
                    waiting.push((job_id, last));
                }
            }
            StartupOutcome::StillPending { .. }
            | StartupOutcome::NeedsAttention { .. }
            | StartupOutcome::UnreadableIntent { .. }
            | StartupOutcome::ProviderError { .. } => recovery.needs_attention += 1,
        }
    }
    Ok((recovery, waiting))
}

/// Record speech of L that an interrupted session left, as a project opens.
/// Never calls a provider.
pub fn recover<L: SpeechLedger>(
    project: &CanonicalProject,
    provider: &str,
) -> Result<SpeechRecovery, GenerationError> {
    finish::<L, LocalDemoVoices>(project, &runtime::<L>(project, provider), None)
        .map(|(recovery, _)| recovery)
}

/// L's speech with one generator, opened for new calls.
pub struct SpeechJobs<'p, L, G> {
    project: &'p CanonicalProject,
    runtime: JobRuntime<'p>,
    generator: &'p G,
    ledger: PhantomData<L>,
}

impl<'p, L: SpeechLedger, G: SpeechGenerator> SpeechJobs<'p, L, G> {
    /// Open L's speech with `generator` for new calls numbered where
    /// `numbering` says an ID belongs. Speech an interruption left is
    /// recorded, intents never called or certainly refused are retired, and
    /// while a call of `numbering` may have been charged without its audio
    /// being kept, opening is refused unless the director risks paying
    /// twice, which retires those calls. An unresolved call outside
    /// `numbering` is left as it is: no risk was accepted for it.
    pub fn open(
        project: &'p CanonicalProject,
        generator: &'p G,
        numbering: impl Fn(&str) -> bool,
        submission: Submission,
    ) -> Result<(Self, SpeechRecovery), GenerationError> {
        let runtime = runtime::<L>(project, generator.provider());
        let (recovery, waiting) = finish::<L, G>(project, &runtime, Some(generator))?;
        if submission == Submission::Once && recovery.unresolved.iter().any(|id| numbering(id)) {
            return Err(GenerationError::UnresolvedSubmission);
        }
        for (job_id, last) in waiting {
            let retired = match last {
                LastAttempt::None => runtime.retire_unattempted(&job_id)?,
                LastAttempt::Refused => runtime.retire_failed(&job_id)?,
                LastAttempt::Unresolved if numbering(job_id.as_str()) => {
                    runtime.retire_unresolved(&job_id)?
                }
                LastAttempt::Unresolved => false,
            };
            if retired {
                remove_receipt(project.root(), job_id.as_str());
            }
        }
        Ok((
            Self {
                project,
                runtime,
                generator,
                ledger: PhantomData,
            },
            recovery,
        ))
    }

    /// Speak `text` with `voice` and record it as `plan` says.
    pub fn speak(
        &mut self,
        plan: &L::Plan,
        text: &str,
        voice: &VoiceSettings,
        hooks: &mut dyn SpeechHooks,
    ) -> Result<SpokenOutput, GenerationError> {
        let format = check_speech(&self.generator.capabilities(), voice, text)?;
        let artifact_id = ArtifactId::new(L::artifact_id(plan))
            .map_err(|_| GenerationError::Unexpected("a speech ID is not an artifact ID"))?;
        let input_hash = InputHash::new(L::input_hash(plan))
            .map_err(|_| GenerationError::Unexpected("a speech input hash is invalid"))?;
        let job_id = JobId::new(artifact_id.as_str())?;
        let record = match self.runtime.prepare(
            job_id.clone(),
            SPEECH_OPERATION,
            input_hash.clone(),
            artifact_id.clone(),
        )? {
            PrepareOutcome::Created(record) | PrepareOutcome::Existing(record) => record,
        };
        if record.intent().state() != IntentState::Prepared || record.attempts() > 0 {
            return Err(GenerationError::Unexpected(
                "speech with this ID is already being made",
            ));
        }
        let root = self.project.root();
        write_receipt(
            root,
            &Receipt {
                schema_version: RECEIPT_SCHEMA_VERSION,
                artifact_id: artifact_id.as_str().to_owned(),
                input_hash: input_hash.as_str().to_owned(),
                provider: self.generator.provider().to_owned(),
                model: self.generator.model().to_owned(),
                parameters: self.generator.canonical_params(voice, format),
                plan: serde_json::to_value(plan)
                    .map_err(|_| GenerationError::Unexpected("a speech plan did not encode"))?,
                refused_attempt: None,
                spoken: None,
            },
        )
        .map_err(|error| GenerationError::Provider {
            source: Box::new(error),
            class: ErrorClass::Retryable,
        })?;
        hooks.reached(SpeechPoint::Prepared);
        let call = SpeechCall {
            artifact_id,
            input_hash,
            request: SpeechRequest {
                text: text.to_owned(),
                voice: voice.clone(),
                format,
            },
            media_dir: L::media_dir(plan),
        };
        let mut speaker = Speaker {
            root,
            generator: Some(self.generator),
            attempt: record.attempts() + 1,
            hooks: &mut *hooks,
        };
        let output = match invoke_when_permitted(&self.runtime, &job_id, &mut speaker, &call) {
            Ok(DispatchOutcome::Completed { output, .. }) => output,
            Ok(DispatchOutcome::Pending { .. }) => {
                return Err(GenerationError::Unexpected("speech was left pending"));
            }
            Err(InvokePreparedError::Runtime(error)) => return Err(error.into()),
            Err(InvokePreparedError::Provider { source, class, .. }) => {
                return Err(GenerationError::Provider {
                    source: Box::new(source),
                    class,
                });
            }
        };
        hooks.reached(SpeechPoint::Invoked);
        L::attach(self.project, plan, &output)?;
        hooks.reached(SpeechPoint::Attached);
        let _ = self.runtime.acknowledge_result(&job_id)?;
        remove_receipt(root, call.artifact_id.as_str());
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use kineto_providers::secret::EnvSecretSource;
    use std::{
        collections::BTreeMap,
        fs,
        path::{Path, PathBuf},
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "kineto-speech-test-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            // The project is created here; the directory must not exist yet.
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(target: &Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "speech_test".to_owned(),
                title: "Speech Test".to_owned(),
                created_at: "2026-10-03T00:00:00Z".to_owned(),
                source: ProjectSource {
                    kind: "text".to_owned(),
                    path: "source/story.txt".to_owned(),
                    extra: BTreeMap::new(),
                },
                defaults: ProjectDefaults {
                    language: "en".to_owned(),
                    extra: BTreeMap::new(),
                },
                workflow: ProjectWorkflow {
                    recipe_id: "default-film".to_owned(),
                    recipe_version: 1,
                    extra: BTreeMap::new(),
                },
                extra: BTreeMap::new(),
            },
            b"source\n",
        )
        .unwrap()
    }

    /// Records each speech it is given as a line of `lines.txt`.
    struct Lines;

    #[derive(Serialize, Deserialize)]
    struct LinePlan {
        artifact_id: String,
        input_hash: String,
    }

    fn plan(artifact_id: &str, text: &str) -> LinePlan {
        LinePlan {
            artifact_id: artifact_id.to_owned(),
            input_hash: InputHash::new(ContentHash::of(text.as_bytes()).as_str())
                .unwrap()
                .as_str()
                .to_owned(),
        }
    }

    fn lines_path() -> ProjectRelativePath {
        ProjectRelativePath::new("voice/lines.txt").unwrap()
    }

    fn attached(project: &CanonicalProject) -> Vec<String> {
        project
            .root()
            .read(&lines_path())
            .map(|bytes| {
                String::from_utf8(bytes)
                    .unwrap()
                    .lines()
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    impl SpeechLedger for Lines {
        type Plan = LinePlan;
        const KIND: &'static str = "line";

        fn artifact_id(plan: &LinePlan) -> &str {
            &plan.artifact_id
        }

        fn input_hash(plan: &LinePlan) -> &str {
            &plan.input_hash
        }

        fn media_dir(_plan: &LinePlan) -> String {
            "voice".to_owned()
        }

        fn attach(
            project: &CanonicalProject,
            plan: &LinePlan,
            spoken: &SpokenOutput,
        ) -> Result<(), GenerationError> {
            let mut lines = attached(project);
            let line = format!(
                "{} {} {} {}",
                plan.artifact_id,
                path_text(&spoken.media_path),
                spoken.duration_ms,
                spoken.parameters
            );
            if !lines.contains(&line) {
                lines.push(line);
            }
            project
                .root()
                .write_atomic(&lines_path(), lines.join("\n").as_bytes())
                .map_err(|error| GenerationError::Provider {
                    source: Box::new(error),
                    class: ErrorClass::Retryable,
                })
        }
    }

    #[derive(Debug)]
    struct Refused;

    impl fmt::Display for Refused {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("refused (1026): input sensitive")
        }
    }

    impl Error for Refused {}

    /// Speaks as the demo does, counting its paid calls; refuses the first
    /// `refuse` of them.
    struct Counting {
        calls: AtomicUsize,
        refuse: usize,
        capabilities: SpeechCapabilities,
    }

    impl Counting {
        fn new(refuse: usize) -> Self {
            Self {
                calls: AtomicUsize::new(0),
                refuse,
                capabilities: SpeechCapabilities {
                    price_per_character: Some(Price::usd(100)),
                    ..demo_speech_capabilities()
                },
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl SpeechGenerator for Counting {
        type Error = Refused;

        fn provider(&self) -> &'static str {
            "counting-speech"
        }

        fn model(&self) -> &'static str {
            "counting"
        }

        fn capabilities(&self) -> SpeechCapabilities {
            self.capabilities.clone()
        }

        fn voices(&self) -> Result<Vec<Voice>, Refused> {
            Ok(Vec::new())
        }

        fn synthesize(&self, request: &SpeechRequest) -> Result<Spoken, Refused> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            if call < self.refuse {
                return Err(Refused);
            }
            Ok(LocalDemoVoices.synthesize(request).unwrap())
        }

        fn class(&self, _error: &Refused) -> ErrorClass {
            ErrorClass::Terminal
        }

        fn certainly_not_accepted(&self, _error: &Refused) -> bool {
            true
        }
    }

    struct StopAt(SpeechPoint);

    impl SpeechHooks for StopAt {
        fn reached(&mut self, point: SpeechPoint) {
            assert_ne!(point, self.0, "stopped at {point:?}");
        }
    }

    const LINE: &str = "I waited for you. All night!";

    fn english() -> VoiceSettings {
        VoiceSettings::new("demo_low", "en")
    }

    fn speak_stopping_at(
        project: &CanonicalProject,
        generator: &Counting,
        artifact_id: &str,
        point: SpeechPoint,
    ) {
        let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let (mut jobs, _) =
                SpeechJobs::<Lines, _>::open(project, generator, |_| true, Submission::Once)
                    .unwrap();
            let _ = jobs.speak(
                &plan(artifact_id, LINE),
                LINE,
                &english(),
                &mut StopAt(point),
            );
        }));
        assert!(stopped.is_err(), "speech must stop at {point:?}");
    }

    #[test]
    fn sentences_end_where_a_reader_breathes() {
        assert_eq!(
            sentences("Wait... Is it you?! \"Yes.\" Then go"),
            ["Wait...", "Is it you?!", "\"Yes.\"", "Then go"]
        );
        assert_eq!(sentences("你好。我很好！"), ["你好。", "我很好！"]);
        assert_eq!(sentences("3.5 metres away."), ["3.5 metres away."]);
        assert!(sentences("  ").is_empty());
    }

    #[test]
    fn a_line_lasts_about_as_long_as_saying_it_takes() {
        // Two sentences of four and two words, a breath between them.
        let timing = sentence_timing(LINE, NORMAL_SPEED);
        assert_eq!(
            timing,
            [
                SpokenSpan {
                    text: "I waited for you.".to_owned(),
                    start_ms: 0,
                    end_ms: 1520,
                },
                SpokenSpan {
                    text: "All night!".to_owned(),
                    start_ms: 1770,
                    end_ms: 2530,
                },
            ]
        );
        assert_eq!(estimate_duration_ms(LINE, 2000), 1265);
        assert_eq!(estimate_duration_ms("还没来。", NORMAL_SPEED), 750);
        assert_eq!(estimate_duration_ms("Oh.", NORMAL_SPEED), WORD_MS);
        assert_eq!(
            estimate_duration_ms("...", NORMAL_SPEED),
            SHORTEST_SENTENCE_MS
        );
        assert_eq!(estimate_duration_ms("", NORMAL_SPEED), 0);
    }

    #[test]
    fn the_demo_speaks_a_tone_as_long_as_the_line_the_same_every_time() {
        let demo = LocalDemoVoices;
        assert_eq!(
            demo.voices()
                .unwrap()
                .iter()
                .map(|voice| voice.id.as_str())
                .collect::<Vec<_>>(),
            ["demo_low", "demo_middle", "demo_high"]
        );
        let request = SpeechRequest {
            text: LINE.to_owned(),
            voice: english(),
            format: AudioFormat::Wav,
        };
        let spoken = demo.synthesize(&request).unwrap();
        assert_eq!(spoken, demo.synthesize(&request).unwrap());
        assert_eq!(spoken.duration_ms, 2530);
        assert_eq!(spoken.sentences, sentence_timing(LINE, NORMAL_SPEED));
        assert_eq!(&spoken.audio[..4], b"RIFF");
        assert_eq!(&spoken.audio[8..16], b"WAVEfmt ");
        // 16 kHz, 16-bit, mono: 32 bytes a millisecond, after the header.
        assert_eq!(spoken.audio.len(), 44 + 2530 * 32);

        let peak = |spoken: &Spoken| {
            spoken.audio[44..]
                .chunks(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]]).unsigned_abs())
                .max()
                .unwrap()
        };
        let whispered = demo
            .synthesize(&SpeechRequest {
                voice: VoiceSettings {
                    emotion: Some(Emotion::Whisper),
                    ..english()
                },
                ..request.clone()
            })
            .unwrap();
        assert!(peak(&whispered) < peak(&spoken) / 2);
        let higher = demo
            .synthesize(&SpeechRequest {
                voice: VoiceSettings {
                    pitch_semitones: 12,
                    ..english()
                },
                ..request.clone()
            })
            .unwrap();
        assert_eq!(higher.duration_ms, spoken.duration_ms);
        assert_ne!(higher.audio, spoken.audio);
        assert_eq!(pitched_millihertz(110, 12), 220_000);
        assert_eq!(pitched_millihertz(220, -12), 110_000);
        assert_eq!(pitched_millihertz(220, -3), 185_020);
    }

    #[test]
    fn a_voice_is_kept_as_chosen_or_refused_by_capability() {
        let demo = demo_speech_capabilities();
        assert_eq!(
            check_speech(&demo, &english(), LINE).unwrap(),
            AudioFormat::Wav
        );

        let basic = SpeechCapabilities {
            voice_sources: [VoiceSource::Catalog].into_iter().collect(),
            languages: ["en".to_owned()].into_iter().collect(),
            speed_permille: Some((500, 2000)),
            output_formats: [AudioFormat::Wav].into_iter().collect(),
            ..SpeechCapabilities::default()
        };
        let calm_japanese = VoiceSettings {
            emotion: Some(Emotion::Calm),
            ..VoiceSettings::new("v", "ja")
        };
        let Err(GenerationError::Incapable(missing)) = check_speech(&basic, &calm_japanese, LINE)
        else {
            panic!("a basic provider cannot keep a calm Japanese voice");
        };
        assert_eq!(missing, ["speech.language", "speech.emotion"]);

        let long = "word ".repeat(2_000);
        let Err(GenerationError::Incapable(missing)) = check_speech(&demo, &english(), &long)
        else {
            panic!("a line longer than the provider takes is refused");
        };
        assert_eq!(missing, ["speech.max_text_chars"]);

        let silent = SpeechCapabilities {
            output_formats: std::collections::BTreeSet::new(),
            ..demo
        };
        assert!(matches!(
            check_speech(&silent, &english(), LINE),
            Err(GenerationError::Incapable(missing)) if missing == ["speech.output_formats"]
        ));
    }

    #[test]
    fn a_voice_round_trips_through_its_record() {
        let voice = VoiceSettings {
            emotion: Some(Emotion::Whisper),
            speed_permille: 900,
            pitch_semitones: -2,
            ..english()
        };
        let encoded = serde_json::to_value(&voice).unwrap();
        assert_eq!(encoded["emotion"], "whisper");
        assert_eq!(
            serde_json::from_value::<VoiceSettings>(encoded).unwrap(),
            voice
        );
        let plain = serde_json::to_value(english()).unwrap();
        assert!(plain.get("emotion").is_none());
        assert!(
            serde_json::from_value::<VoiceSettings>(serde_json::json!({
                "voice_id": "v", "language": "en", "emotion": "wistful",
                "speed_permille": 1000, "pitch_semitones": 0
            }))
            .is_err()
        );
        assert_eq!(
            english().canonical(),
            "voice=demo_low;language=en;emotion=;speed=1000;pitch=0"
        );
    }

    #[test]
    fn the_session_names_its_speech_provider() {
        let secrets = EnvSecretSource::process();
        assert!(SpeechProvider::select(None, &secrets).unwrap().is_none());
        assert!(
            SpeechProvider::select(Some(" "), &secrets)
                .unwrap()
                .is_none()
        );
        let demo = SpeechProvider::select(Some("local-demo"), &secrets)
            .unwrap()
            .unwrap();
        assert_eq!(
            demo.info(),
            SpeechProviderInfo {
                kind: SpeechProviderKind::LocalDemo,
                ready: true,
                character_cost_micros: Some(0),
            }
        );
        assert_eq!(demo.estimate(LINE), Some(0));
        assert!(matches!(
            SpeechProvider::select(Some("elevenlabs"), &secrets),
            Err(ProviderSelectionError::Unknown(name)) if name == "elevenlabs"
        ));
    }

    #[test]
    fn speech_is_written_recorded_and_its_receipt_dropped() {
        let dir = TempDir::new();
        let project = project(&dir.0);
        let (mut jobs, recovery) =
            SpeechJobs::<Lines, _>::open(&project, &LocalDemoVoices, |_| true, Submission::Once)
                .unwrap();
        assert_eq!(recovery, SpeechRecovery::default());
        let spoken = jobs
            .speak(&plan("line_0001", LINE), LINE, &english(), &mut NoHooks)
            .unwrap();
        assert_eq!(path_text(&spoken.media_path), "voice/line_0001.wav");
        assert_eq!(spoken.provider, DEMO_SPEECH_PROVIDER);
        assert_eq!(spoken.duration_ms, 2530);
        assert_eq!(
            spoken.parameters,
            "voice=demo_low;language=en;emotion=;speed=1000;pitch=0;format=wav"
        );
        let audio = project.root().read(&spoken.media_path).unwrap();
        assert_eq!(ContentHash::of(&audio), spoken.content_hash);
        assert_eq!(
            attached(&project),
            [
                "line_0001 voice/line_0001.wav 2530 voice=demo_low;language=en;emotion=;speed=1000;pitch=0;format=wav"
            ]
        );
        assert!(!dir.0.join(".kineto/generation/line_0001.json").exists());
    }

    #[test]
    fn speech_written_before_an_interruption_is_recorded_without_paying_again() {
        for point in [
            SpeechPoint::Written,
            SpeechPoint::Invoked,
            SpeechPoint::Attached,
        ] {
            let dir = TempDir::new();
            let project = project(&dir.0);
            let generator = Counting::new(0);
            speak_stopping_at(&project, &generator, "line_0001", point);
            assert_eq!(generator.calls(), 1);

            let recovery = recover::<Lines>(&project, generator.provider()).unwrap();
            assert_eq!(recovery.attached, ["line_0001"], "after {point:?}");
            assert_eq!(attached(&project).len(), 1);
            assert_eq!(
                generator.calls(),
                1,
                "nothing is paid again after {point:?}"
            );
            assert!(!dir.0.join(".kineto/generation/line_0001.json").exists());
        }
    }

    #[test]
    fn a_call_that_may_have_been_charged_is_made_again_only_at_the_directors_risk() {
        let dir = TempDir::new();
        let project = project(&dir.0);
        let generator = Counting::new(0);
        // Answered, but nothing of the answer was kept.
        speak_stopping_at(&project, &generator, "line_0001", SpeechPoint::Answered);

        let refused = SpeechJobs::<Lines, _>::open(
            &project,
            &generator,
            |id| id.starts_with("line_"),
            Submission::Once,
        );
        assert!(matches!(
            refused,
            Err(GenerationError::UnresolvedSubmission)
        ));
        // Another numbering is not held up, and leaves the call unresolved.
        let (_, recovery) = SpeechJobs::<Lines, _>::open(
            &project,
            &generator,
            |id| id.starts_with("audition_"),
            Submission::Once,
        )
        .unwrap();
        assert_eq!(recovery.unresolved, ["line_0001"]);

        let (mut jobs, _) = SpeechJobs::<Lines, _>::open(
            &project,
            &generator,
            |id| id.starts_with("line_"),
            Submission::RiskPayingTwice,
        )
        .unwrap();
        jobs.speak(&plan("line_0001", LINE), LINE, &english(), &mut NoHooks)
            .unwrap();
        assert_eq!(generator.calls(), 2);
        assert_eq!(attached(&project).len(), 1);
    }

    #[test]
    fn a_call_never_made_or_certainly_refused_is_asked_again_freely() {
        let dir = TempDir::new();
        let project = project(&dir.0);
        let generator = Counting::new(1);
        speak_stopping_at(&project, &generator, "line_0001", SpeechPoint::Prepared);
        assert_eq!(generator.calls(), 0);

        let (mut jobs, recovery) =
            SpeechJobs::<Lines, _>::open(&project, &generator, |_| true, Submission::Once).unwrap();
        assert_eq!(recovery, SpeechRecovery::default());
        let error = jobs
            .speak(&plan("line_0001", LINE), LINE, &english(), &mut NoHooks)
            .unwrap_err();
        assert_eq!(error.to_string(), "refused (1026): input sensitive");
        assert!(matches!(
            error,
            GenerationError::Provider {
                class: ErrorClass::Terminal,
                ..
            }
        ));
        drop(jobs);

        // The receipt proves the provider took nothing, so asking again
        // needs no risk.
        let (mut jobs, recovery) =
            SpeechJobs::<Lines, _>::open(&project, &generator, |_| true, Submission::Once).unwrap();
        assert!(recovery.unresolved.is_empty());
        jobs.speak(&plan("line_0001", LINE), LINE, &english(), &mut NoHooks)
            .unwrap();
        assert_eq!(generator.calls(), 2);
        assert_eq!(attached(&project).len(), 1);
    }

    #[test]
    fn a_provider_that_would_change_the_voice_is_refused_before_anything_is_written() {
        let dir = TempDir::new();
        let project = project(&dir.0);
        let generator = Counting {
            capabilities: SpeechCapabilities {
                emotions: std::collections::BTreeSet::new(),
                ..demo_speech_capabilities()
            },
            ..Counting::new(0)
        };
        let (mut jobs, _) =
            SpeechJobs::<Lines, _>::open(&project, &generator, |_| true, Submission::Once).unwrap();
        let angry = VoiceSettings {
            emotion: Some(Emotion::Angry),
            ..english()
        };
        assert!(matches!(
            jobs.speak(&plan("line_0001", LINE), LINE, &angry, &mut NoHooks),
            Err(GenerationError::Incapable(missing)) if missing == ["speech.emotion"]
        ));
        assert_eq!(generator.calls(), 0);
        assert!(!dir.0.join(".kineto/jobs").exists());
    }
}
