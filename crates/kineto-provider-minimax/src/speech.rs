//! MiniMax text-to-speech (`t2a_v2`) and its voice catalogue.
//!
//! Speech is synchronous: the paid call answers with the audio itself, hex
//! encoded inside the authenticated response, and its measured length. When
//! asked for, MiniMax also writes sentence timings to a file on its content
//! host, fetched without the API key; timings are a convenience, so a
//! failure to fetch them never costs the audio already paid for. Listing the
//! system voices is free.

use kineto_providers::{
    AudioFormat, Emotion, Price, SpeechCapabilities, SpeechTiming, VoiceSource,
};
use serde::{Deserialize, Serialize};

use crate::{MinimaxClient, MinimaxError, Transport, WireBaseResp, api_error};

/// The speech model this adapter calls.
pub const SPEECH_MODEL: &str = "speech-2.8-hd";
/// Published pay-as-you-go price of `speech-2.8-hd`: $100 for a million
/// characters, in USD micros a character.
pub const SPEECH_PRICE_MICROS_PER_CHARACTER: u64 = 100;
/// MiniMax takes fewer than 10,000 characters a request.
pub const MAX_SPEECH_CHARS: usize = 9_999;
/// Sentence timings for the longest text are far smaller than this.
const MAX_SUBTITLE_BYTES: u64 = 1024 * 1024;
const SAMPLE_RATE: u32 = 32_000;
const BITRATE: u32 = 128_000;

/// ISO 639-1 codes and the names MiniMax's `language_boost` takes for them.
const LANGUAGES: [(&str, &str); 39] = [
    ("af", "Afrikaans"),
    ("ar", "Arabic"),
    ("bg", "Bulgarian"),
    ("ca", "Catalan"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("hu", "Hungarian"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("ms", "Malay"),
    ("nl", "Dutch"),
    ("nn", "Nynorsk"),
    ("no", "Norwegian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sv", "Swedish"),
    ("ta", "Tamil"),
    ("th", "Thai"),
    ("tl", "Filipino"),
    ("tr", "Turkish"),
    ("uk", "Ukrainian"),
    ("vi", "Vietnamese"),
    ("zh", "Chinese"),
];

/// The deliveries `speech-2.8-hd` takes. MiniMax's `fluent` and `whisper`
/// belong to its 2.6 models only.
const EMOTIONS: [Emotion; 7] = [
    Emotion::Happy,
    Emotion::Sad,
    Emotion::Angry,
    Emotion::Fearful,
    Emotion::Disgusted,
    Emotion::Surprised,
    Emotion::Calm,
];

/// What `speech-2.8-hd` does through this adapter, as MiniMax documents it:
/// voices from its catalogue, about 40 languages, seven deliveries, rates of
/// 0.5–2× and pitch of ±12 semitones, sentence timings, MP3, WAV or FLAC in
/// the answer, fewer than 10,000 characters a call, priced by the
/// character. MiniMax can also clone and design voices, at a price per
/// voice; this adapter does neither, so neither is declared.
#[must_use]
pub fn speech_capabilities() -> SpeechCapabilities {
    SpeechCapabilities {
        voice_sources: [VoiceSource::Catalog].into_iter().collect(),
        languages: LANGUAGES
            .iter()
            .map(|(code, _)| (*code).to_owned())
            .collect(),
        emotions: EMOTIONS.into_iter().collect(),
        speed_permille: Some((500, 2000)),
        pitch_semitones: Some((-12, 12)),
        timing: SpeechTiming::Sentence,
        async_jobs: false,
        output_formats: [AudioFormat::Mp3, AudioFormat::Wav, AudioFormat::Flac]
            .into_iter()
            .collect(),
        max_text_chars: Some(MAX_SPEECH_CHARS),
        price_per_character: Some(Price::usd(SPEECH_PRICE_MICROS_PER_CHARACTER)),
    }
}

/// The name `language_boost` takes for an ISO 639-1 code.
#[must_use]
pub fn language_boost(code: &str) -> Option<&'static str> {
    LANGUAGES
        .iter()
        .find(|(known, _)| *known == code)
        .map(|(_, name)| *name)
}

/// One line asked of `speech-2.8-hd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechRequest {
    pub text: String,
    pub voice_id: String,
    /// ISO 639-1 code of the language it is in.
    pub language: String,
    pub emotion: Option<Emotion>,
    /// In thousandths of the normal rate, 500 to 2000.
    pub speed_permille: u16,
    /// In semitones, -12 to 12.
    pub pitch_semitones: i8,
    pub format: AudioFormat,
}

/// A sentence MiniMax spoke, and when it was heard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpokenSentence {
    pub text: String,
    pub start_ms: u32,
    pub end_ms: u32,
}

/// What MiniMax answered.
#[derive(Clone, PartialEq, Eq)]
pub struct SynthesizedSpeech {
    pub audio: Vec<u8>,
    pub format: AudioFormat,
    /// As MiniMax measured it.
    pub duration_ms: u32,
    /// Empty when the timings could not be fetched.
    pub sentences: Vec<SpokenSentence>,
    /// The characters MiniMax charged for.
    pub billed_characters: u64,
}

impl std::fmt::Debug for SynthesizedSpeech {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SynthesizedSpeech")
            .field("audio", &self.audio.len())
            .field("format", &self.format)
            .field("duration_ms", &self.duration_ms)
            .field("sentences", &self.sentences)
            .field("billed_characters", &self.billed_characters)
            .finish()
    }
}

/// A voice in MiniMax's system catalogue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogVoice {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Serialize)]
struct WireSpeechRequest<'a> {
    model: &'a str,
    text: &'a str,
    stream: bool,
    language_boost: &'a str,
    output_format: &'a str,
    voice_setting: WireVoiceSetting<'a>,
    audio_setting: WireAudioSetting<'a>,
    subtitle_enable: bool,
}

#[derive(Serialize)]
struct WireVoiceSetting<'a> {
    voice_id: &'a str,
    speed: f64,
    vol: f64,
    pitch: i8,
    #[serde(skip_serializing_if = "Option::is_none")]
    emotion: Option<&'a str>,
}

#[derive(Serialize)]
struct WireAudioSetting<'a> {
    sample_rate: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    bitrate: Option<u32>,
    format: &'a str,
    channel: u8,
}

#[derive(Deserialize)]
struct WireSpeechResponse {
    #[serde(default)]
    data: Option<WireSpeechData>,
    #[serde(default)]
    extra_info: Option<WireExtraInfo>,
    base_resp: WireBaseResp,
}

#[derive(Deserialize)]
struct WireSpeechData {
    #[serde(default)]
    audio: String,
    #[serde(default)]
    subtitle_file: Option<String>,
}

#[derive(Deserialize)]
struct WireExtraInfo {
    #[serde(default)]
    audio_length: u64,
    #[serde(default)]
    usage_characters: u64,
}

#[derive(Deserialize)]
struct WireSubtitle {
    #[serde(default)]
    text: String,
    time_begin: f64,
    time_end: f64,
}

#[derive(Serialize)]
struct WireVoiceQuery<'a> {
    voice_type: &'a str,
}

#[derive(Deserialize)]
struct WireVoices {
    #[serde(default)]
    system_voice: Option<Vec<WireVoice>>,
    base_resp: WireBaseResp,
}

#[derive(Deserialize)]
struct WireVoice {
    voice_id: String,
    #[serde(default)]
    voice_name: Option<String>,
    #[serde(default)]
    description: Option<Vec<String>>,
}

impl<T: Transport> MinimaxClient<T> {
    /// What speaking `characters` characters costs, in USD micros.
    #[must_use]
    pub const fn estimate_speech_cost(characters: u64) -> u64 {
        characters.saturating_mul(SPEECH_PRICE_MICROS_PER_CHARACTER)
    }

    /// Speak one line. This is a paid call.
    pub fn synthesize(&self, request: &SpeechRequest) -> Result<SynthesizedSpeech, MinimaxError> {
        let characters = request.text.chars().count();
        if request.text.trim().is_empty() || characters > MAX_SPEECH_CHARS {
            return Err(MinimaxError::InvalidPrompt);
        }
        if request.voice_id.trim().is_empty() {
            return Err(MinimaxError::InvalidRequest("a voice must be named"));
        }
        if !(500..=2000).contains(&request.speed_permille)
            || !(-12..=12).contains(&request.pitch_semitones)
        {
            return Err(MinimaxError::InvalidRequest(
                "rate or pitch is outside what MiniMax takes",
            ));
        }
        let emotion = match request.emotion {
            Some(emotion) if EMOTIONS.contains(&emotion) => Some(emotion.name()),
            Some(_) => {
                return Err(MinimaxError::InvalidRequest(
                    "speech-2.8-hd does not take that delivery",
                ));
            }
            None => None,
        };
        let language = language_boost(&request.language).ok_or(MinimaxError::InvalidRequest(
            "MiniMax does not speak that language",
        ))?;
        let body = serde_json::to_vec(&WireSpeechRequest {
            model: SPEECH_MODEL,
            text: &request.text,
            stream: false,
            language_boost: language,
            // The audio arrives in the TLS answer itself, so it needs no
            // second, unauthenticated download.
            output_format: "hex",
            voice_setting: WireVoiceSetting {
                voice_id: &request.voice_id,
                speed: f64::from(request.speed_permille) / 1000.0,
                vol: 1.0,
                pitch: request.pitch_semitones,
                emotion,
            },
            audio_setting: WireAudioSetting {
                sample_rate: SAMPLE_RATE,
                bitrate: (request.format == AudioFormat::Mp3).then_some(BITRATE),
                format: request.format.extension(),
                channel: 1,
            },
            subtitle_enable: true,
        })
        .map_err(|_| MinimaxError::InvalidRequest("request did not encode"))?;
        let response = self
            .transport
            .post_json(
                &format!("{}/v1/t2a_v2", self.base_url),
                &self.api_key,
                &body,
            )
            .map_err(MinimaxError::Transport)?;
        let parsed = serde_json::from_slice::<WireSpeechResponse>(&response.body);
        if !(200..300).contains(&response.status) {
            return Err(match parsed {
                Ok(wire) if wire.base_resp.status_code != 0 => api_error(&wire.base_resp),
                _ => MinimaxError::Http {
                    status: response.status,
                },
            });
        }
        let wire = parsed.map_err(|_| MinimaxError::Malformed("body is not a speech response"))?;
        if wire.base_resp.status_code != 0 {
            return Err(api_error(&wire.base_resp));
        }
        let data = wire
            .data
            .ok_or(MinimaxError::Malformed("speech response has no audio"))?;
        let audio = decode_hex(&data.audio).ok_or(MinimaxError::Malformed("audio is not hex"))?;
        if audio.is_empty() {
            return Err(MinimaxError::Malformed("speech response has no audio"));
        }
        if audio_format(&audio) != Some(request.format) {
            return Err(MinimaxError::Malformed(
                "audio is not in the format asked for",
            ));
        }
        let extra = wire
            .extra_info
            .ok_or(MinimaxError::Malformed("speech response has no length"))?;
        let duration_ms = u32::try_from(extra.audio_length)
            .map_err(|_| MinimaxError::Malformed("speech is impossibly long"))?;
        Ok(SynthesizedSpeech {
            audio,
            format: request.format,
            duration_ms,
            sentences: data
                .subtitle_file
                .as_deref()
                .map(|url| self.sentences(url))
                .unwrap_or_default(),
            billed_characters: extra.usage_characters,
        })
    }

    /// The sentence timings MiniMax wrote for a call, or none: the audio is
    /// already paid for, so a timing file that cannot be had is left out.
    fn sentences(&self, url: &str) -> Vec<SpokenSentence> {
        if !url.starts_with("https://") {
            return Vec::new();
        }
        let Ok(response) = self.transport.download(url, MAX_SUBTITLE_BYTES) else {
            return Vec::new();
        };
        if !(200..300).contains(&response.status) {
            return Vec::new();
        }
        let Ok(subtitles) = serde_json::from_slice::<Vec<WireSubtitle>>(&response.body) else {
            return Vec::new();
        };
        let milliseconds = |value: f64| {
            if value.is_finite() && value >= 0.0 && value <= f64::from(u32::MAX) {
                // In range and rounded, so the cast is exact.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                Some(value.round() as u32)
            } else {
                None
            }
        };
        subtitles
            .into_iter()
            .map(|subtitle| {
                Some(SpokenSentence {
                    text: subtitle.text.trim().to_owned(),
                    start_ms: milliseconds(subtitle.time_begin)?,
                    end_ms: milliseconds(subtitle.time_end)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .filter(|sentences| {
                sentences
                    .iter()
                    .all(|sentence| sentence.start_ms <= sentence.end_ms)
            })
            .unwrap_or_default()
    }

    /// MiniMax's system voices. Free.
    pub fn speech_voices(&self) -> Result<Vec<CatalogVoice>, MinimaxError> {
        let body = serde_json::to_vec(&WireVoiceQuery {
            voice_type: "system",
        })
        .map_err(|_| MinimaxError::InvalidRequest("request did not encode"))?;
        let response = self
            .transport
            .post_json(
                &format!("{}/v1/get_voice", self.base_url),
                &self.api_key,
                &body,
            )
            .map_err(MinimaxError::Transport)?;
        let parsed = serde_json::from_slice::<WireVoices>(&response.body);
        if !(200..300).contains(&response.status) {
            return Err(match parsed {
                Ok(wire) if wire.base_resp.status_code != 0 => api_error(&wire.base_resp),
                _ => MinimaxError::Http {
                    status: response.status,
                },
            });
        }
        let wire = parsed.map_err(|_| MinimaxError::Malformed("body is not a voice list"))?;
        if wire.base_resp.status_code != 0 {
            return Err(api_error(&wire.base_resp));
        }
        Ok(wire
            .system_voice
            .unwrap_or_default()
            .into_iter()
            .filter(|voice| !voice.voice_id.trim().is_empty())
            .map(|voice| CatalogVoice {
                name: voice
                    .voice_name
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| voice.voice_id.clone()),
                description: voice.description.unwrap_or_default().join(" "),
                id: voice.voice_id,
            })
            .collect())
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let digits = text.trim().as_bytes();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let value = |digit: u8| match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        b'A'..=b'F' => Some(digit - b'A' + 10),
        _ => None,
    };
    digits
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[high, low]| Some(value(*high)? << 4 | value(*low)?))
        .collect()
}

/// The audio format from its leading bytes.
#[must_use]
pub fn audio_format(bytes: &[u8]) -> Option<AudioFormat> {
    match bytes {
        [
            b'R',
            b'I',
            b'F',
            b'F',
            _,
            _,
            _,
            _,
            b'W',
            b'A',
            b'V',
            b'E',
            ..,
        ] => Some(AudioFormat::Wav),
        [b'f', b'L', b'a', b'C', ..] => Some(AudioFormat::Flac),
        [b'I', b'D', b'3', ..] => Some(AudioFormat::Mp3),
        [0xff, second, ..] if second & 0xe0 == 0xe0 => Some(AudioFormat::Mp3),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpResponse, TransportFailure};
    use kineto_providers::secret::SecretString;
    use std::{collections::VecDeque, sync::Mutex};

    const KEY: &str = "sk-test-not-a-real-key";

    #[derive(Default)]
    struct Recorded {
        answers: Mutex<VecDeque<Result<HttpResponse, TransportFailure>>>,
        posts: Mutex<Vec<(String, serde_json::Value)>>,
        downloads: Mutex<Vec<(String, u64)>>,
    }

    impl Recorded {
        fn next(&self) -> Result<HttpResponse, TransportFailure> {
            self.answers
                .lock()
                .unwrap()
                .pop_front()
                .expect("a recorded answer")
        }
    }

    impl Transport for Recorded {
        fn post_json(
            &self,
            url: &str,
            api_key: &SecretString,
            body: &[u8],
        ) -> Result<HttpResponse, TransportFailure> {
            assert_eq!(api_key.expose(), KEY);
            self.posts
                .lock()
                .unwrap()
                .push((url.to_owned(), serde_json::from_slice(body).unwrap()));
            self.next()
        }

        fn get(
            &self,
            _url: &str,
            _api_key: &SecretString,
        ) -> Result<HttpResponse, TransportFailure> {
            panic!("speech never GETs the API");
        }

        fn download(&self, url: &str, max_bytes: u64) -> Result<HttpResponse, TransportFailure> {
            self.downloads
                .lock()
                .unwrap()
                .push((url.to_owned(), max_bytes));
            self.next()
        }
    }

    fn client(
        answers: impl IntoIterator<Item = Result<HttpResponse, TransportFailure>>,
    ) -> MinimaxClient<Recorded> {
        MinimaxClient::with_transport(
            Recorded {
                answers: Mutex::new(answers.into_iter().collect()),
                ..Recorded::default()
            },
            SecretString::new(KEY.to_owned()).unwrap(),
            "https://example.test",
        )
    }

    fn json(status: u16, body: &str) -> Result<HttpResponse, TransportFailure> {
        Ok(HttpResponse {
            status,
            body: body.as_bytes().to_vec(),
        })
    }

    /// An MP3 frame header and a few bytes, as MiniMax's hex.
    const MP3_HEX: &str = "fffb9064000000";

    fn spoken(subtitle_file: &str) -> Result<HttpResponse, TransportFailure> {
        json(
            200,
            &format!(
                r#"{{"data":{{"audio":"{MP3_HEX}","status":2{subtitle_file}}},
                "extra_info":{{"audio_length":2530,"audio_sample_rate":32000,"audio_size":7,
                "bitrate":128000,"word_count":6,"usage_characters":28,"audio_format":"mp3",
                "audio_channel":1}},"trace_id":"t","base_resp":{{"status_code":0,"status_msg":"success"}}}}"#
            ),
        )
    }

    fn request() -> SpeechRequest {
        SpeechRequest {
            text: "I waited for you. All night!".to_owned(),
            voice_id: "English_Wiselady".to_owned(),
            language: "en".to_owned(),
            emotion: Some(Emotion::Sad),
            speed_permille: 900,
            pitch_semitones: -2,
            format: AudioFormat::Mp3,
        }
    }

    #[test]
    fn a_line_is_asked_for_as_declared_and_heard_with_its_timings() {
        let minimax = client([
            spoken(r#","subtitle_file":"https://cdn.example.test/sub.json""#),
            json(
                200,
                r#"[{"text":"I waited for you. ","time_begin":0,"time_end":1480.4,"text_begin":0,"text_end":18},
                    {"text":"All night!","time_begin":1720,"time_end":2530,"text_begin":18,"text_end":28}]"#,
            ),
        ]);
        let speech = minimax.synthesize(&request()).unwrap();
        assert_eq!(speech.audio, [0xff, 0xfb, 0x90, 0x64, 0, 0, 0]);
        assert_eq!(speech.format, AudioFormat::Mp3);
        assert_eq!(speech.duration_ms, 2530);
        assert_eq!(speech.billed_characters, 28);
        assert_eq!(
            speech.sentences,
            [
                SpokenSentence {
                    text: "I waited for you.".to_owned(),
                    start_ms: 0,
                    end_ms: 1480,
                },
                SpokenSentence {
                    text: "All night!".to_owned(),
                    start_ms: 1720,
                    end_ms: 2530,
                },
            ]
        );

        let posts = minimax.transport.posts.lock().unwrap();
        assert_eq!(posts[0].0, "https://example.test/v1/t2a_v2");
        assert_eq!(
            posts[0].1,
            serde_json::json!({
                "model": "speech-2.8-hd",
                "text": "I waited for you. All night!",
                "stream": false,
                "language_boost": "English",
                "output_format": "hex",
                "voice_setting": {
                    "voice_id": "English_Wiselady", "speed": 0.9, "vol": 1.0, "pitch": -2,
                    "emotion": "sad"
                },
                "audio_setting": {
                    "sample_rate": 32000, "bitrate": 128000, "format": "mp3", "channel": 1
                },
                "subtitle_enable": true
            })
        );
        // The timing file is fetched without the key, bounded.
        assert_eq!(
            *minimax.transport.downloads.lock().unwrap(),
            [(
                "https://cdn.example.test/sub.json".to_owned(),
                MAX_SUBTITLE_BYTES
            )]
        );
    }

    #[test]
    fn audio_already_paid_for_is_kept_without_its_timings() {
        for (subtitle_file, timing_answer) in [
            ("", None),
            (
                r#","subtitle_file":"http://cdn.example.test/sub.json""#,
                None,
            ),
            (
                r#","subtitle_file":"https://cdn.example.test/sub.json""#,
                Some(Err(TransportFailure::Timeout)),
            ),
            (
                r#","subtitle_file":"https://cdn.example.test/sub.json""#,
                Some(json(403, "denied")),
            ),
            (
                r#","subtitle_file":"https://cdn.example.test/sub.json""#,
                Some(json(200, r#"[{"text":"x","time_begin":-5,"time_end":3}]"#)),
            ),
        ] {
            let minimax = client([spoken(subtitle_file)].into_iter().chain(timing_answer));
            let speech = minimax.synthesize(&request()).unwrap();
            assert_eq!(speech.duration_ms, 2530, "{subtitle_file}");
            assert!(speech.sentences.is_empty(), "{subtitle_file}");
        }
    }

    #[test]
    fn what_minimax_cannot_take_is_refused_before_anything_is_sent() {
        let minimax = client([]);
        for (request, why) in [
            (
                SpeechRequest {
                    text: " ".to_owned(),
                    ..request()
                },
                "prompt is empty or longer than the model accepts",
            ),
            (
                SpeechRequest {
                    text: "x".repeat(MAX_SPEECH_CHARS + 1),
                    ..request()
                },
                "prompt is empty or longer than the model accepts",
            ),
            (
                SpeechRequest {
                    emotion: Some(Emotion::Whisper),
                    ..request()
                },
                "MiniMax request is invalid: speech-2.8-hd does not take that delivery",
            ),
            (
                SpeechRequest {
                    language: "xx".to_owned(),
                    ..request()
                },
                "MiniMax request is invalid: MiniMax does not speak that language",
            ),
            (
                SpeechRequest {
                    speed_permille: 2500,
                    ..request()
                },
                "MiniMax request is invalid: rate or pitch is outside what MiniMax takes",
            ),
        ] {
            let error = minimax.synthesize(&request).unwrap_err();
            assert_eq!(error.to_string(), why);
            assert!(error.certainly_not_accepted());
        }
        assert!(minimax.transport.posts.lock().unwrap().is_empty());
    }

    #[test]
    fn refusals_say_why_and_a_spent_plan_is_not_retried() {
        let minimax = client([
            json(
                200,
                r#"{"base_resp":{"status_code":2056,"status_msg":"Token Plan usage limit reached"}}"#,
            ),
            json(
                200,
                r#"{"base_resp":{"status_code":1039,"status_msg":"TPM rate limit exceeded"}}"#,
            ),
            json(500, "upstream"),
            json(
                200,
                r#"{"data":{"audio":"zz"},"extra_info":{"audio_length":1},"base_resp":{"status_code":0}}"#,
            ),
            json(
                200,
                r#"{"data":{"audio":"52494646"},"extra_info":{"audio_length":1},"base_resp":{"status_code":0}}"#,
            ),
        ]);
        let spent = minimax.synthesize(&request()).unwrap_err();
        assert_eq!(
            spent.to_string(),
            "MiniMax refused the request (2056): Token Plan usage limit reached"
        );
        assert_eq!(spent.class(), kineto_jobs::ErrorClass::Terminal);
        assert!(spent.certainly_not_accepted());
        let limited = minimax.synthesize(&request()).unwrap_err();
        assert_eq!(limited.class(), kineto_jobs::ErrorClass::Retryable);
        assert!(limited.certainly_not_accepted());
        let failed = minimax.synthesize(&request()).unwrap_err();
        assert_eq!(failed.to_string(), "MiniMax answered HTTP 500");
        assert!(
            !failed.certainly_not_accepted(),
            "a server error may have been charged"
        );
        assert_eq!(
            minimax.synthesize(&request()).unwrap_err().to_string(),
            "MiniMax response is malformed: audio is not hex"
        );
        assert_eq!(
            minimax.synthesize(&request()).unwrap_err().to_string(),
            "MiniMax response is malformed: audio is not in the format asked for"
        );
    }

    #[test]
    fn the_system_voices_are_listed_free() {
        let minimax = client([json(
            200,
            r#"{"system_voice":[
                {"voice_id":"English_Wiselady","voice_name":"Wise Lady","description":["A calm,","measured woman."],"created_time":"2025-01-01"},
                {"voice_id":"male-qn-qingse","description":[]},
                {"voice_id":" "}
              ],"voice_cloning":[],"base_resp":{"status_code":0,"status_msg":"success"}}"#,
        )]);
        assert_eq!(
            minimax.speech_voices().unwrap(),
            [
                CatalogVoice {
                    id: "English_Wiselady".to_owned(),
                    name: "Wise Lady".to_owned(),
                    description: "A calm, measured woman.".to_owned(),
                },
                CatalogVoice {
                    id: "male-qn-qingse".to_owned(),
                    name: "male-qn-qingse".to_owned(),
                    description: String::new(),
                },
            ]
        );
        let posts = minimax.transport.posts.lock().unwrap();
        assert_eq!(posts[0].0, "https://example.test/v1/get_voice");
        assert_eq!(posts[0].1, serde_json::json!({"voice_type": "system"}));
    }

    #[test]
    fn the_declaration_matches_what_the_client_sends_and_charges() {
        let declared = speech_capabilities();
        assert_eq!(declared.languages.len(), 39);
        for code in &declared.languages {
            assert!(language_boost(code).is_some(), "{code}");
        }
        assert_eq!(language_boost("zh"), Some("Chinese"));
        assert_eq!(
            declared.emotions,
            EMOTIONS.into_iter().collect(),
            "fluent and whisper are 2.6 deliveries"
        );
        assert_eq!(declared.cost_micros(10_000), Some(1_000_000));
        assert_eq!(
            MinimaxClient::<Recorded>::estimate_speech_cost(10_000),
            1_000_000
        );
        assert_eq!(declared.preferred_format(), Some(AudioFormat::Mp3));
        assert_eq!(audio_format(b"fLaC\0\0"), Some(AudioFormat::Flac));
        assert_eq!(audio_format(b"ID3\x04"), Some(AudioFormat::Mp3));
        assert_eq!(
            audio_format(b"RIFF\0\0\0\0WAVEfmt "),
            Some(AudioFormat::Wav)
        );
        assert_eq!(audio_format(b"OggS"), None);
        assert_eq!(decode_hex("0aFf"), Some(vec![0x0a, 0xff]));
        assert_eq!(decode_hex("abc"), None);
    }
}
