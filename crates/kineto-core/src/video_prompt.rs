//! Compile a shot's written intent into the prompt for its video, which
//! starts from the locked keyframe.
//!
//! The first frame carries framing, lens, setting and the characters' look,
//! so those are applied through it rather than restated. Camera motion and
//! clip length, which the keyframe compiler dropped, are applied here. A clip
//! length the provider cannot render becomes the nearest one it can, and
//! that is reported as a cosmetic drop. The video model generates no speech,
//! so dialogue is dropped.

use kineto_project::shot::{ShotDirection, ShotIntent};
use kineto_prompts::{
    CompiledPrompt, ConstraintId, ConstraintIdError, DegradationLevel, DroppedConstraint,
    PromptHash,
};
use sha2::{Digest, Sha256};

use crate::keyframe_prompt::KeyframePromptError;

/// Version recorded with every video prompt compiled by this module.
pub const VIDEO_COMPILER_VERSION: &str = "video-prompt/1";

const INTENT_DOMAIN: &[u8] = b"kineto.video-intent.v1";

/// A compiled video prompt, its hash, and the clip length it asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPrompt {
    pub compiled: CompiledPrompt,
    pub prompt_hash: PromptHash,
    pub duration_seconds: u8,
}

/// Compile the video prompt for a shot whose locked keyframe has
/// `keyframe_hash`. `durations` are the clip lengths the provider renders,
/// in seconds, shortest first.
pub fn compile(
    shot: Option<&ShotIntent>,
    direction: ShotDirection,
    keyframe_hash: &str,
    durations: &[u8],
    max_prompt_chars: usize,
) -> Result<VideoPrompt, KeyframePromptError> {
    let shot = shot.ok_or(KeyframePromptError::MissingIntent)?;
    let mut applied = vec![constraint("keyframe.first_frame")?];
    let mut dropped = Vec::new();
    let mut sentences = vec![sentence_case(&shot.action)];
    applied.push(constraint("shot.action")?);

    if let Some(motion) = &shot.camera_motion {
        sentences.push(camera(motion));
        applied.push(constraint("shot.camera_motion")?);
    }
    if let Some(emotion) = &shot.emotion {
        sentences.push(format!("Mood: {emotion}"));
        applied.push(constraint("shot.emotion")?);
    }
    sentences.push(emphasis(direction).to_owned());
    applied.push(constraint("shot.direction")?);
    sentences.push(
        "Keep the subject, framing and light of the first frame; natural, restrained motion"
            .to_owned(),
    );
    // Carried by the first frame rather than restated.
    for carried in ["shot.framing", "shot.lens"] {
        applied.push(constraint(carried)?);
    }
    for character in &shot.characters {
        applied.push(constraint(&format!("identity.{character}"))?);
    }

    if shot.dialogue.is_some() {
        dropped.push(DroppedConstraint {
            constraint: constraint("shot.dialogue")?,
            reason: "the video model generates no speech".to_owned(),
            degradation: DegradationLevel::Cosmetic,
        });
    }
    let duration_seconds = nearest(shot.duration_target, durations);
    match shot.duration_target {
        Some(target) if (f64::from(duration_seconds) - target).abs() > f64::EPSILON => {
            dropped.push(DroppedConstraint {
                constraint: constraint("shot.duration_target")?,
                reason: format!(
                    "{target} s was asked for; the provider renders {duration_seconds} s"
                ),
                degradation: DegradationLevel::Cosmetic,
            });
        }
        Some(_) => applied.push(constraint("shot.duration_target")?),
        None => {}
    }

    let prompt = sentences.join(". ") + ".";
    let chars = prompt.chars().count();
    if chars > max_prompt_chars {
        return Err(KeyframePromptError::TooLong {
            chars,
            max: max_prompt_chars,
        });
    }
    let compiled = CompiledPrompt::new(prompt, applied, dropped, VIDEO_COMPILER_VERSION)?;
    let prompt_hash = compiled.prompt_hash(&canonical_intent(
        shot,
        direction,
        keyframe_hash,
        duration_seconds,
    ));
    Ok(VideoPrompt {
        compiled,
        prompt_hash,
        duration_seconds,
    })
}

/// The supported clip length closest to `target`; ties and no target go to
/// the shorter, cheaper clip.
fn nearest(target: Option<f64>, durations: &[u8]) -> u8 {
    let shortest = durations.first().copied().unwrap_or(6);
    let Some(target) = target else {
        return shortest;
    };
    durations
        .iter()
        .copied()
        .min_by(|left, right| {
            (f64::from(*left) - target)
                .abs()
                .total_cmp(&(f64::from(*right) - target).abs())
                .then(left.cmp(right))
        })
        .unwrap_or(shortest)
}

/// Camera motion in the provider's bracket commands where one fits, in words
/// otherwise.
fn camera(motion: &str) -> String {
    let command = match motion {
        "dolly_in" | "slow_dolly_in" | "push_in" | "slow_push_in" => Some("Push in"),
        "dolly_out" | "slow_dolly_out" | "pull_out" | "slow_pull_out" => Some("Pull out"),
        "pan_left" => Some("Pan left"),
        "pan_right" => Some("Pan right"),
        "tilt_up" => Some("Tilt up"),
        "tilt_down" => Some("Tilt down"),
        "tracking" | "tracking_shot" => Some("Tracking shot"),
        "static" | "locked_off" | "static_frame" => Some("Static shot"),
        _ => None,
    };
    match command {
        Some(command) => format!("Camera: {} [{command}]", motion.replace('_', " ")),
        None => format!("Camera: {}", motion.replace('_', " ")),
    }
}

fn emphasis(direction: ShotDirection) -> &'static str {
    match direction {
        ShotDirection::Reaction => "Let the listener's reaction carry the moment",
        ShotDirection::SpatialClarity => {
            "Keep where each person stands readable as the camera moves"
        }
        ShotDirection::Intimacy => "Stay close and intimate with the subject",
        ShotDirection::Tension => "Let the tension build through the clip",
    }
}

fn constraint(value: &str) -> Result<ConstraintId, ConstraintIdError> {
    ConstraintId::new(value)
}

fn sentence_case(value: &str) -> String {
    let mut chars = value.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

fn canonical_intent(
    shot: &ShotIntent,
    direction: ShotDirection,
    keyframe_hash: &str,
    duration_seconds: u8,
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
    field(keyframe_hash.as_bytes());
    field(shot.action.as_bytes());
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
    field(&[direction.code(), duration_seconds]);
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYFRAME: &str =
        "sha256:1111111111111111111111111111111111111111111111111111111111111111";

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

    #[test]
    fn motion_is_applied_and_an_unrenderable_length_is_reported() {
        let prompt = compile(
            Some(&shot()),
            ShotDirection::Reaction,
            KEYFRAME,
            &[6, 10],
            2000,
        )
        .unwrap();
        assert_eq!(
            prompt.compiled.prompt(),
            "A filmmaker hears the final line across the table. Camera: slow dolly in \
             [Push in]. Mood: a withheld reaction. Let the listener's reaction carry the \
             moment. Keep the subject, framing and light of the first frame; natural, \
             restrained motion."
        );
        assert_eq!(prompt.duration_seconds, 6);
        let dropped = prompt.compiled.dropped_constraints();
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].constraint.as_str(), "shot.duration_target");
        assert_eq!(
            dropped[0].reason,
            "4 s was asked for; the provider renders 6 s"
        );
        assert_eq!(prompt.compiled.degradation(), DegradationLevel::Cosmetic);
        assert!(
            prompt
                .compiled
                .applied_constraints()
                .iter()
                .any(|applied| applied.as_str() == "identity.filmmaker")
        );
    }

    #[test]
    fn the_hash_follows_the_keyframe_and_the_clip() {
        let base = compile(
            Some(&shot()),
            ShotDirection::Reaction,
            KEYFRAME,
            &[6, 10],
            2000,
        )
        .unwrap()
        .prompt_hash;
        let other_keyframe = format!("sha256:{}", "2".repeat(64));
        assert_ne!(
            compile(
                Some(&shot()),
                ShotDirection::Reaction,
                &other_keyframe,
                &[6, 10],
                2000
            )
            .unwrap()
            .prompt_hash,
            base
        );
        let mut longer = shot();
        longer.duration_target = Some(10.0);
        let ten = compile(
            Some(&longer),
            ShotDirection::Reaction,
            KEYFRAME,
            &[6, 10],
            2000,
        )
        .unwrap();
        assert_eq!(ten.duration_seconds, 10);
        assert!(ten.compiled.dropped_constraints().is_empty());
        assert_ne!(ten.prompt_hash, base);
    }

    #[test]
    fn unknown_motion_is_described_and_missing_intent_refused() {
        assert_eq!(camera("restrained_pan"), "Camera: restrained pan");
        assert_eq!(
            compile(None, ShotDirection::Reaction, KEYFRAME, &[6, 10], 2000),
            Err(KeyframePromptError::MissingIntent)
        );
        assert_eq!(nearest(Some(8.0), &[6, 10]), 6);
        assert_eq!(nearest(Some(8.1), &[6, 10]), 10);
        assert_eq!(nearest(None, &[6, 10]), 6);
    }
}
