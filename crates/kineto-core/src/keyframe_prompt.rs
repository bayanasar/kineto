//! Compile a shot's written intent into the prompt for one keyframe.
//!
//! The intent stays expressive; the compiler reports what a still image cannot
//! carry instead of deleting it. Camera motion, dialogue and duration belong
//! to the video step and are dropped as cosmetic. Character identity is
//! described in text only, because no approved reference images exist yet, so
//! every named character is an identity-affecting drop: two takes of the same
//! shot can show different faces.

use std::{error::Error, fmt};

use kineto_project::shot::{SceneIntent, ShotDirection, ShotIntent};
use kineto_prompts::{
    CompiledPrompt, CompiledPromptError, ConstraintId, ConstraintIdError, DegradationLevel,
    DroppedConstraint, PromptHash,
};
use sha2::{Digest, Sha256};

/// Version recorded with every keyframe compiled by this module. Change it
/// whenever the same intent would compile to a different prompt.
pub const KEYFRAME_COMPILER_VERSION: &str = "keyframe-prompt/1";

const INTENT_DOMAIN: &[u8] = b"kineto.keyframe-intent.v1";

/// A compiled keyframe prompt with the hash that ties it to its intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyframePrompt {
    pub compiled: CompiledPrompt,
    pub prompt_hash: PromptHash,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyframePromptError {
    /// The shot has no written intent to compile.
    MissingIntent,
    /// The compiled prompt is longer than the provider accepts. It is never
    /// truncated, because that would drop constraints without reporting them.
    TooLong {
        chars: usize,
        max: usize,
    },
    Constraint(ConstraintIdError),
    Prompt(CompiledPromptError),
}

impl fmt::Display for KeyframePromptError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingIntent => formatter.write_str("the shot has no written intent to compile"),
            Self::TooLong { chars, max } => write!(
                formatter,
                "compiled keyframe prompt is {chars} characters; the provider accepts {max}"
            ),
            Self::Constraint(error) => error.fmt(formatter),
            Self::Prompt(error) => error.fmt(formatter),
        }
    }
}

impl Error for KeyframePromptError {}

impl From<ConstraintIdError> for KeyframePromptError {
    fn from(error: ConstraintIdError) -> Self {
        Self::Constraint(error)
    }
}

impl From<CompiledPromptError> for KeyframePromptError {
    fn from(error: CompiledPromptError) -> Self {
        Self::Prompt(error)
    }
}

/// Compile the keyframe prompt for a shot.
pub fn compile(
    scene: Option<&SceneIntent>,
    shot: Option<&ShotIntent>,
    direction: ShotDirection,
    max_prompt_chars: usize,
) -> Result<KeyframePrompt, KeyframePromptError> {
    let shot = shot.ok_or(KeyframePromptError::MissingIntent)?;
    let mut applied = Vec::new();
    let mut dropped = Vec::new();
    let mut sentences = Vec::new();

    if let Some(framing) = &shot.framing {
        sentences.push(format!("{} shot", sentence_case(&words(framing))));
        applied.push(constraint("shot.framing")?);
    }
    sentences.push(sentence_case(&shot.action));
    applied.push(constraint("shot.action")?);

    if let Some(emotion) = &shot.emotion {
        sentences.push(format!("Mood: {emotion}"));
        applied.push(constraint("shot.emotion")?);
    }
    if let Some(scene) = scene {
        let setting = [&scene.location, &scene.time]
            .into_iter()
            .flatten()
            .map(|value| words(value))
            .collect::<Vec<_>>();
        if !setting.is_empty() {
            sentences.push(format!("Setting: {}", setting.join(", ")));
        }
        if scene.location.is_some() {
            applied.push(constraint("scene.location")?);
        }
        if scene.time.is_some() {
            applied.push(constraint("scene.time")?);
        }
    }
    if let Some(lens) = &shot.lens {
        sentences.push(format!("Shot on a {lens} lens"));
        applied.push(constraint("shot.lens")?);
    }
    sentences.push(emphasis(direction).to_owned());
    applied.push(constraint("shot.direction")?);
    sentences.push(
        "Photorealistic cinematic film still, natural light, no text, no watermark".to_owned(),
    );

    if shot.camera_motion.is_some() {
        dropped.push(drop(
            "shot.camera_motion",
            "a still keyframe cannot show camera motion; it is kept for the video step",
            DegradationLevel::Cosmetic,
        )?);
    }
    if shot.dialogue.is_some() {
        dropped.push(drop(
            "shot.dialogue",
            "a still keyframe carries no dialogue",
            DegradationLevel::Cosmetic,
        )?);
    }
    if shot.duration_target.is_some() {
        dropped.push(drop(
            "shot.duration_target",
            "a still keyframe has no duration; it is kept for the video step",
            DegradationLevel::Cosmetic,
        )?);
    }
    for character in &shot.characters {
        dropped.push(DroppedConstraint {
            constraint: constraint(&format!("identity.{character}"))?,
            reason: format!(
                "no approved reference images exist for {character}; identity is described in \
                 text only and can drift between takes and shots"
            ),
            degradation: DegradationLevel::IdentityAffecting,
        });
    }

    let prompt = sentences.join(". ") + ".";
    let chars = prompt.chars().count();
    if chars > max_prompt_chars {
        return Err(KeyframePromptError::TooLong {
            chars,
            max: max_prompt_chars,
        });
    }
    let compiled = CompiledPrompt::new(prompt, applied, dropped, KEYFRAME_COMPILER_VERSION)?;
    let prompt_hash = compiled.prompt_hash(&canonical_intent(scene, shot, direction));
    Ok(KeyframePrompt {
        compiled,
        prompt_hash,
    })
}

fn emphasis(direction: ShotDirection) -> &'static str {
    match direction {
        ShotDirection::Reaction => "Favor the listener's reaction over the speaker",
        ShotDirection::SpatialClarity => {
            "Keep the geography of the space and where each person stands readable"
        }
        ShotDirection::Intimacy => "Stay close and intimate with the subject",
        ShotDirection::Tension => "Compose for mounting tension",
    }
}

fn constraint(value: &str) -> Result<ConstraintId, ConstraintIdError> {
    ConstraintId::new(value)
}

fn drop(
    id: &str,
    reason: &str,
    degradation: DegradationLevel,
) -> Result<DroppedConstraint, ConstraintIdError> {
    Ok(DroppedConstraint {
        constraint: constraint(id)?,
        reason: reason.to_owned(),
        degradation,
    })
}

/// `medium_close_up` reads as `medium close up`.
fn words(value: &str) -> String {
    value.replace('_', " ")
}

fn sentence_case(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A deterministic, length-framed encoding of everything the compiler read.
fn canonical_intent(
    scene: Option<&SceneIntent>,
    shot: &ShotIntent,
    direction: ShotDirection,
) -> Vec<u8> {
    let mut hasher = Sha256::new();
    let mut field = |bytes: &[u8]| {
        hasher.update(
            u64::try_from(bytes.len())
                .expect("length fits in u64")
                .to_be_bytes(),
        );
        hasher.update(bytes);
    };
    let optional = |value: &Option<String>| match value {
        None => vec![0],
        Some(value) => [&[1], value.as_bytes()].concat(),
    };
    field(INTENT_DOMAIN);
    field(&optional(&scene.and_then(|scene| scene.location.clone())));
    field(&optional(&scene.and_then(|scene| scene.time.clone())));
    field(shot.action.as_bytes());
    field(&optional(&shot.framing));
    field(&optional(&shot.lens));
    field(&optional(&shot.camera_motion));
    field(&optional(&shot.emotion));
    field(&optional(&shot.dialogue));
    field(&(shot.characters.len() as u64).to_be_bytes());
    for character in &shot.characters {
        field(character.as_bytes());
    }
    field(
        &shot
            .duration_target
            .map_or_else(Vec::new, |seconds| seconds.to_bits().to_be_bytes().to_vec()),
    );
    field(&[direction.code()]);
    // The prompt hash frames this again; a digest keeps it fixed-size.
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot() -> ShotIntent {
        ShotIntent {
            action: "A filmmaker hears the final line across the table".to_owned(),
            framing: Some("medium_close_up".to_owned()),
            lens: Some("50mm".to_owned()),
            camera_motion: Some("slow_dolly_in".to_owned()),
            emotion: Some("a withheld reaction".to_owned()),
            dialogue: None,
            characters: vec!["filmmaker".to_owned()],
            duration_target: Some(4.0),
        }
    }

    fn scene() -> SceneIntent {
        SceneIntent {
            location: Some("small_office".to_owned()),
            time: Some("night".to_owned()),
            dramatic_goal: None,
            characters: vec!["filmmaker".to_owned()],
        }
    }

    #[test]
    fn a_shot_compiles_to_a_stable_prompt_with_its_losses_reported() {
        let first = compile(Some(&scene()), Some(&shot()), ShotDirection::Reaction, 1500).unwrap();
        assert_eq!(
            first.compiled.prompt(),
            "Medium close up shot. A filmmaker hears the final line across the table. \
             Mood: a withheld reaction. Setting: small office, night. Shot on a 50mm lens. \
             Favor the listener's reaction over the speaker. Photorealistic cinematic film \
             still, natural light, no text, no watermark."
        );
        assert_eq!(first.compiled.compiler_version(), KEYFRAME_COMPILER_VERSION);
        assert_eq!(
            first
                .compiled
                .dropped_constraints()
                .iter()
                .map(|dropped| dropped.constraint.as_str())
                .collect::<Vec<_>>(),
            [
                "shot.camera_motion",
                "shot.duration_target",
                "identity.filmmaker"
            ]
        );
        assert_eq!(
            first.compiled.degradation(),
            DegradationLevel::IdentityAffecting
        );
        assert_eq!(
            first,
            compile(Some(&scene()), Some(&shot()), ShotDirection::Reaction, 1500).unwrap()
        );
    }

    #[test]
    fn every_intent_field_changes_the_prompt_hash() {
        let base = compile(Some(&scene()), Some(&shot()), ShotDirection::Reaction, 1500)
            .unwrap()
            .prompt_hash;
        let mut variants = vec![
            compile(None, Some(&shot()), ShotDirection::Reaction, 1500),
            compile(Some(&scene()), Some(&shot()), ShotDirection::Tension, 1500),
        ];
        let edits: [fn(&mut ShotIntent); 4] = [
            |shot| shot.camera_motion = None,
            |shot| shot.dialogue = Some("Goodbye.".to_owned()),
            |shot| shot.characters.push("interviewer".to_owned()),
            |shot| shot.duration_target = Some(5.0),
        ];
        for edit in edits {
            let mut shot = shot();
            edit(&mut shot);
            variants.push(compile(
                Some(&scene()),
                Some(&shot),
                ShotDirection::Reaction,
                1500,
            ));
        }
        for variant in variants {
            assert_ne!(variant.unwrap().prompt_hash, base);
        }
    }

    #[test]
    fn a_shot_without_intent_or_too_long_a_prompt_is_refused() {
        assert_eq!(
            compile(Some(&scene()), None, ShotDirection::Reaction, 1500),
            Err(KeyframePromptError::MissingIntent)
        );
        assert!(matches!(
            compile(Some(&scene()), Some(&shot()), ShotDirection::Reaction, 40),
            Err(KeyframePromptError::TooLong { max: 40, .. })
        ));
    }

    #[test]
    fn a_minimal_intent_names_only_what_was_written() {
        let shot = ShotIntent {
            action: "Rain on an empty street".to_owned(),
            ..ShotIntent::default()
        };
        let compiled = compile(None, Some(&shot), ShotDirection::SpatialClarity, 1500).unwrap();
        assert_eq!(
            compiled.compiled.prompt(),
            "Rain on an empty street. Keep the geography of the space and where each person \
             stands readable. Photorealistic cinematic film still, natural light, no text, no \
             watermark."
        );
        assert!(compiled.compiled.dropped_constraints().is_empty());
    }
}
