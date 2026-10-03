//! What a line is asked to sound like: `speech-prompt/1`.
//!
//! A speech provider is given the line's words and a voice, not a written
//! direction. So a line is spoken with its character's locked voice, and
//! what else the screenplay and the director ask for is carried as far as
//! the provider's controls go:
//!
//! - the screenplay's parenthetical, as a delivery when a word of it names
//!   one the provider has (`quietly` is calm, `whispering` a whisper), and
//!   reported as dropped otherwise;
//! - the director's chosen delivery and rate, which replace the voice's;
//! - the director's note, kept with the take; the provider never hears it.
//!
//! The provenance records what was applied and what was dropped, as a
//! prompt's does.

use kineto_project::{
    ContentHash,
    shot::{Degradation, DroppedConstraintRecord, PromptProvenance},
};
use kineto_providers::{Emotion, SpeechCapabilities};

use crate::speech::VoiceSettings;

/// Version of the line compiler.
pub const SPEECH_COMPILER_VERSION: &str = "speech-prompt/1";

/// Words of a parenthetical, by their stem, and the delivery each names.
/// Earlier entries win, so `softly whispering` is a whisper.
const DELIVERIES: [(&str, Emotion); 30] = [
    ("whisper", Emotion::Whisper),
    ("hush", Emotion::Whisper),
    ("murmur", Emotion::Whisper),
    ("angr", Emotion::Angry),
    ("shout", Emotion::Angry),
    ("furious", Emotion::Angry),
    ("yell", Emotion::Angry),
    ("snap", Emotion::Angry),
    ("sad", Emotion::Sad),
    ("tear", Emotion::Sad),
    ("cry", Emotion::Sad),
    ("grie", Emotion::Sad),
    ("sorrow", Emotion::Sad),
    ("happ", Emotion::Happy),
    ("laugh", Emotion::Happy),
    ("cheer", Emotion::Happy),
    ("delight", Emotion::Happy),
    ("afraid", Emotion::Fearful),
    ("fear", Emotion::Fearful),
    ("scared", Emotion::Fearful),
    ("nervous", Emotion::Fearful),
    ("trembl", Emotion::Fearful),
    ("disgust", Emotion::Disgusted),
    ("surpris", Emotion::Surprised),
    ("shock", Emotion::Surprised),
    ("astonish", Emotion::Surprised),
    ("calm", Emotion::Calm),
    ("quiet", Emotion::Calm),
    ("soft", Emotion::Calm),
    ("gentl", Emotion::Calm),
];

/// What a line is asked for.
#[derive(Debug, Clone, Copy)]
pub struct LineDirection<'a> {
    pub text: &'a str,
    pub parenthetical: Option<&'a str>,
    /// The character's locked voice.
    pub voice: &'a VoiceSettings,
    /// The director's chosen delivery, which replaces any other.
    pub delivery: Option<Emotion>,
    /// The director's chosen rate, in thousandths of normal.
    pub speed_permille: Option<u16>,
    pub note: Option<&'a str>,
    pub capabilities: &'a SpeechCapabilities,
}

/// A line compiled for a speech provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinePrompt {
    /// The voice as it is asked to speak the line.
    pub voice: VoiceSettings,
    pub provenance: PromptProvenance,
}

/// The delivery a parenthetical names, if a word of it names one.
#[must_use]
pub fn delivery_named(parenthetical: &str) -> Option<Emotion> {
    let words: Vec<String> = parenthetical
        .split(|character: char| !character.is_alphabetic())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    DELIVERIES
        .iter()
        .find(|(stem, _)| words.iter().any(|word| word.starts_with(stem)))
        .map(|(_, emotion)| *emotion)
}

/// Compile `direction`.
#[must_use]
pub fn compile(direction: LineDirection<'_>) -> LinePrompt {
    let mut voice = direction.voice.clone();
    let mut applied = vec!["line.text".to_owned(), "voice.locked".to_owned()];
    let mut dropped = Vec::new();
    let mut drop = |constraint: &str, reason: String| {
        dropped.push(DroppedConstraintRecord {
            constraint: constraint.to_owned(),
            reason,
            degradation: Degradation::Cosmetic,
        });
    };

    let parenthetical = direction
        .parenthetical
        .map(str::trim)
        .filter(|text| !text.is_empty());
    if let Some(delivery) = direction.delivery {
        voice.emotion = Some(delivery);
        applied.push("director.delivery".to_owned());
        if let Some(parenthetical) = parenthetical {
            drop(
                "line.parenthetical",
                format!("the director chose the delivery instead of {parenthetical:?}"),
            );
        }
    } else if let Some(parenthetical) = parenthetical {
        match delivery_named(parenthetical) {
            Some(emotion) if direction.capabilities.emotions.contains(&emotion) => {
                voice.emotion = Some(emotion);
                applied.push("line.parenthetical".to_owned());
            }
            Some(emotion) => drop(
                "line.parenthetical",
                format!(
                    "{parenthetical:?} is a {} delivery, which the provider does not take",
                    emotion.name()
                ),
            ),
            None => drop(
                "line.parenthetical",
                format!("no delivery the provider takes matches {parenthetical:?}"),
            ),
        }
    }
    if let Some(speed) = direction.speed_permille {
        voice.speed_permille = speed;
        applied.push("director.rate".to_owned());
    }
    if let Some(note) = direction
        .note
        .map(str::trim)
        .filter(|note| !note.is_empty())
    {
        drop(
            "director.note",
            format!("kept with the take; the provider takes no written direction: {note}"),
        );
    }

    let mut material = Vec::new();
    for field in [
        SPEECH_COMPILER_VERSION,
        direction.text,
        &voice.canonical(),
        parenthetical.unwrap_or(""),
    ] {
        material.extend_from_slice(
            &u64::try_from(field.len())
                .expect("length fits in u64")
                .to_be_bytes(),
        );
        material.extend_from_slice(field.as_bytes());
    }
    LinePrompt {
        provenance: PromptProvenance {
            compiler_version: SPEECH_COMPILER_VERSION.to_owned(),
            compiled_prompt: direction.text.to_owned(),
            prompt_hash: ContentHash::of(&material).as_str().to_owned(),
            applied_constraints: applied,
            dropped_constraints: dropped,
        },
        voice,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::speech::demo_speech_capabilities;
    use kineto_provider_minimax::speech_capabilities as speech_capabilities_of_minimax;

    fn direction<'a>(
        voice: &'a VoiceSettings,
        capabilities: &'a SpeechCapabilities,
        parenthetical: Option<&'a str>,
    ) -> LineDirection<'a> {
        LineDirection {
            text: "Was it worth it?",
            parenthetical,
            voice,
            delivery: None,
            speed_permille: None,
            note: None,
            capabilities,
        }
    }

    #[test]
    fn a_parenthetical_becomes_a_delivery_the_provider_has_or_is_reported() {
        assert_eq!(delivery_named("quietly"), Some(Emotion::Calm));
        assert_eq!(delivery_named("softly, whispering"), Some(Emotion::Whisper));
        assert_eq!(delivery_named("Through tears"), Some(Emotion::Sad));
        assert_eq!(delivery_named("beat"), None);

        let voice = VoiceSettings::new("demo_low", "en");
        let demo = demo_speech_capabilities();
        let quiet = compile(direction(&voice, &demo, Some("quietly")));
        assert_eq!(quiet.voice.emotion, Some(Emotion::Calm));
        assert_eq!(
            quiet.provenance.applied_constraints,
            ["line.text", "voice.locked", "line.parenthetical"]
        );
        assert_eq!(quiet.provenance.compiled_prompt, "Was it worth it?");

        // MiniMax's 2.8 model has no whisper: the line keeps the voice's
        // own delivery and says so.
        let minimax = speech_capabilities_of_minimax();
        let whispered = compile(direction(&voice, &minimax, Some("whispering")));
        assert_eq!(whispered.voice.emotion, None);
        assert_eq!(
            whispered.provenance.dropped_constraints[0].reason,
            "\"whispering\" is a whisper delivery, which the provider does not take"
        );
        let beat = compile(direction(&voice, &demo, Some("a beat")));
        assert_eq!(
            beat.provenance.dropped_constraints[0].constraint,
            "line.parenthetical"
        );
        assert_ne!(quiet.provenance.prompt_hash, beat.provenance.prompt_hash);
    }

    #[test]
    fn the_directors_delivery_and_rate_replace_the_voices_and_the_note_is_kept_aside() {
        let voice = VoiceSettings {
            emotion: Some(Emotion::Calm),
            ..VoiceSettings::new("demo_low", "en")
        };
        let demo = demo_speech_capabilities();
        let refined = compile(LineDirection {
            delivery: Some(Emotion::Angry),
            speed_permille: Some(1200),
            note: Some("Harder, faster."),
            ..direction(&voice, &demo, Some("quietly"))
        });
        assert_eq!(refined.voice.emotion, Some(Emotion::Angry));
        assert_eq!(refined.voice.speed_permille, 1200);
        assert_eq!(
            refined.provenance.applied_constraints,
            [
                "line.text",
                "voice.locked",
                "director.delivery",
                "director.rate"
            ]
        );
        let dropped: Vec<_> = refined
            .provenance
            .dropped_constraints
            .iter()
            .map(|dropped| dropped.constraint.as_str())
            .collect();
        assert_eq!(dropped, ["line.parenthetical", "director.note"]);
        // Without a parenthetical, the voice keeps its own delivery.
        assert_eq!(
            compile(direction(&voice, &demo, None)).voice.emotion,
            Some(Emotion::Calm)
        );
    }
}
