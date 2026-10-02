//! Compile a shot's written intent, and what production has locked, into
//! the prompt for one keyframe.
//!
//! The intent stays expressive; the compiler reports what a still image cannot
//! carry instead of deleting it. Camera motion, dialogue and duration belong
//! to the video step and are dropped as cosmetic.
//!
//! The locked style leads the prompt. A character whose cast is locked is
//! shown from their canonical portrait, sent as the provider's identity
//! reference, when the provider takes one; one reference is sent per image,
//! to the first such character of the shot. Every other named character is
//! described in text only, an identity-affecting drop: two takes of the same
//! shot can show different faces. A cast character's forbidden changes are
//! said as long as the prompt has room, and dropping them is
//! identity-affecting too. Nothing is truncated.

use std::{error::Error, fmt};

use kineto_project::{
    ArtifactDependency,
    cast::{self, CharacterLock, LockedImage},
    shot::{SceneIntent, ShotDirection, ShotIntent},
    style::{self, StyleLock},
};
use kineto_prompts::{
    CompiledPrompt, CompiledPromptError, ConstraintId, ConstraintIdError, DegradationLevel,
    DroppedConstraint, PromptHash,
};
use kineto_providers::{ImageCapabilities, ReferenceRole};
use sha2::{Digest, Sha256};

use crate::style_prompt::{Part, fit, sentence};

/// Version recorded with every keyframe compiled by this module. Change it
/// whenever the same intent would compile to a different prompt.
pub const KEYFRAME_COMPILER_VERSION: &str = "keyframe-prompt/2";

const INTENT_DOMAIN: &[u8] = b"kineto.keyframe-intent.v2";

/// What production has locked that a keyframe follows.
#[derive(Debug, Clone, Copy, Default)]
pub struct Locks<'a> {
    pub style: Option<&'a StyleLock>,
    /// The locked casts of characters, in any order.
    pub characters: &'a [CharacterLock],
}

/// A compiled keyframe prompt with the hash that ties it to its intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyframePrompt {
    pub compiled: CompiledPrompt,
    pub prompt_hash: PromptHash,
    /// The locks the prompt follows, which a keyframe made from it depends
    /// on.
    pub dependencies: Vec<ArtifactDependency>,
    /// The canonical portrait to send as the identity reference, when one
    /// is.
    pub identity: Option<LockedImage>,
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

/// Compile the keyframe prompt for a shot following `locks`, for a provider
/// with `capabilities`.
pub fn compile(
    scene: Option<&SceneIntent>,
    shot: Option<&ShotIntent>,
    direction: ShotDirection,
    locks: Locks<'_>,
    capabilities: &ImageCapabilities,
) -> Result<KeyframePrompt, KeyframePromptError> {
    let shot = shot.ok_or(KeyframePromptError::MissingIntent)?;
    let mut required = Vec::new();
    let mut optional = Vec::new();
    let mut dropped = Vec::new();
    let mut dependencies = Vec::new();

    if let Some(style) = locks.style {
        needed(&mut required, "style.locked", style.directives.clone())?;
        if let Some(note) = &style.note {
            needed(&mut required, "style.note", format!("Style note: {note}"))?;
        }
        dependencies.extend(style::lock_dependency(style));
    }
    if let Some(framing) = &shot.framing {
        needed(
            &mut required,
            "shot.framing",
            format!("{} shot", sentence_case(&words(framing))),
        )?;
    }
    needed(&mut required, "shot.action", sentence_case(&shot.action))?;
    if let Some(emotion) = &shot.emotion {
        needed(&mut required, "shot.emotion", format!("Mood: {emotion}"))?;
    }
    if let Some(scene) = scene {
        let setting = [&scene.location, &scene.time]
            .into_iter()
            .flatten()
            .map(|value| words(value))
            .collect::<Vec<_>>();
        // The setting is one sentence; each of its parts is a constraint.
        if !setting.is_empty() {
            let text = format!("Setting: {}", setting.join(", "));
            match (&scene.location, &scene.time) {
                // The time rides in the location's sentence.
                (Some(_), Some(_)) => {
                    needed(&mut required, "scene.location", &text)?;
                    needed(&mut required, "scene.time", "")?;
                }
                (Some(_), None) => needed(&mut required, "scene.location", &text)?,
                _ => needed(&mut required, "scene.time", &text)?,
            }
        }
    }
    if let Some(lens) = &shot.lens {
        needed(&mut required, "shot.lens", format!("Shot on a {lens} lens"))?;
    }
    needed(&mut required, "shot.direction", emphasis(direction))?;

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

    let takes_identity = capabilities.reference_limit(ReferenceRole::Identity) > 0;
    let mut identity: Option<LockedImage> = None;
    for character in &shot.characters {
        let lock = locks
            .characters
            .iter()
            .find(|lock| &lock.character_id == character);
        if let Some(lock) = lock {
            dependencies.extend(cast::lock_dependency(lock));
        }
        let reason = match lock {
            Some(lock) if takes_identity && identity.is_none() => {
                identity = Some(lock.portrait.clone());
                needed(
                    &mut required,
                    &format!("identity.{character}"),
                    format!("{} is the person in the reference image", lock.name),
                )?;
                None
            }
            Some(_) if takes_identity => Some(format!(
                "one identity reference is sent per image; {character} is described in text \
                 only and can drift between takes and shots"
            )),
            _ if takes_identity => Some(format!(
                "no approved reference images exist for {character}; identity is described in \
                 text only and can drift between takes and shots"
            )),
            _ => Some(format!(
                "the provider takes no identity reference image; {character} is described in \
                 text only and can drift between takes and shots"
            )),
        };
        if let Some(reason) = reason {
            dropped.push(DroppedConstraint {
                constraint: constraint(&format!("identity.{character}"))?,
                reason,
                degradation: DegradationLevel::IdentityAffecting,
            });
        }
        if let Some(lock) = lock
            && !lock.forbidden_changes.is_empty()
        {
            let forbidden = format!("forbidden.{character}");
            ConstraintId::new(forbidden.as_str())?;
            optional.push(Part {
                constraint: forbidden,
                text: sentence(&format!(
                    "{}, always: {}",
                    lock.name,
                    lock.forbidden_changes
                        .iter()
                        .map(|change| change.trim_end_matches(['.', ' ']))
                        .collect::<Vec<_>>()
                        .join("; ")
                )),
                rank: 0,
                degradation: DegradationLevel::IdentityAffecting,
            });
        }
    }
    // A locked style says how the image looks; only the default names a
    // photograph.
    let look = if locks.style.is_some() {
        "No text, no watermark"
    } else {
        "Photorealistic cinematic film still, natural light, no text, no watermark"
    };
    needed(&mut required, "shot.still", look)?;

    let prompt = fit(
        &required,
        &optional,
        dropped,
        capabilities.max_prompt_chars,
        KEYFRAME_COMPILER_VERSION,
        &canonical_intent(scene, shot, direction, locks),
    )?;
    Ok(KeyframePrompt {
        dependencies,
        identity,
        ..prompt
    })
}

/// A part the prompt cannot do without.
fn needed(
    parts: &mut Vec<Part>,
    constraint: &str,
    text: impl AsRef<str>,
) -> Result<(), KeyframePromptError> {
    ConstraintId::new(constraint)?;
    let text = text.as_ref();
    parts.push(Part {
        constraint: constraint.to_owned(),
        text: if text.is_empty() {
            String::new()
        } else {
            sentence(text)
        },
        rank: u8::MAX,
        degradation: DegradationLevel::Cosmetic,
    });
    Ok(())
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
    locks: Locks<'_>,
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
    field(
        &locks
            .style
            .map(|style| serde_json::to_vec(style).expect("a style lock always encodes"))
            .unwrap_or_default(),
    );
    for character in &shot.characters {
        field(
            &locks
                .characters
                .iter()
                .find(|lock| &lock.character_id == character)
                .map(|lock| serde_json::to_vec(lock).expect("a cast lock always encodes"))
                .unwrap_or_default(),
        );
    }
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

    fn minimax() -> ImageCapabilities {
        kineto_provider_minimax::image_capabilities()
    }

    #[test]
    fn a_shot_compiles_to_a_stable_prompt_with_its_losses_reported() {
        let first = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks::default(),
            &minimax(),
        )
        .unwrap();
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
            compile(
                Some(&scene()),
                Some(&shot()),
                ShotDirection::Reaction,
                Locks::default(),
                &minimax()
            )
            .unwrap()
        );
    }

    #[test]
    fn every_intent_field_changes_the_prompt_hash() {
        let base = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks::default(),
            &minimax(),
        )
        .unwrap()
        .prompt_hash;
        let mut variants = vec![
            compile(
                None,
                Some(&shot()),
                ShotDirection::Reaction,
                Locks::default(),
                &minimax(),
            ),
            compile(
                Some(&scene()),
                Some(&shot()),
                ShotDirection::Tension,
                Locks::default(),
                &minimax(),
            ),
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
                Locks::default(),
                &minimax(),
            ));
        }
        for variant in variants {
            assert_ne!(variant.unwrap().prompt_hash, base);
        }
    }

    #[test]
    fn a_shot_without_intent_or_too_long_a_prompt_is_refused() {
        assert_eq!(
            compile(
                Some(&scene()),
                None,
                ShotDirection::Reaction,
                Locks::default(),
                &minimax()
            ),
            Err(KeyframePromptError::MissingIntent)
        );
        assert!(matches!(
            compile(
                Some(&scene()),
                Some(&shot()),
                ShotDirection::Reaction,
                Locks::default(),
                &ImageCapabilities {
                    max_prompt_chars: Some(40),
                    ..minimax()
                }
            ),
            Err(KeyframePromptError::TooLong { max: 40, .. })
        ));
        // MiniMax's own limit applies: a long action is refused, never cut.
        let long = ShotIntent {
            action: "a".repeat(1500),
            ..shot()
        };
        assert!(matches!(
            compile(
                Some(&scene()),
                Some(&long),
                ShotDirection::Reaction,
                Locks::default(),
                &minimax()
            ),
            Err(KeyframePromptError::TooLong { max: 1500, .. })
        ));
        // A provider without a limit takes any length.
        assert!(
            compile(
                Some(&scene()),
                Some(&long),
                ShotDirection::Reaction,
                Locks::default(),
                &ImageCapabilities::default()
            )
            .is_ok()
        );
    }

    #[test]
    fn identity_drops_say_whether_the_provider_could_take_a_reference() {
        let reason = |capabilities: &ImageCapabilities| {
            compile(
                Some(&scene()),
                Some(&shot()),
                ShotDirection::Reaction,
                Locks::default(),
                capabilities,
            )
            .unwrap()
            .compiled
            .dropped_constraints()
            .iter()
            .find(|dropped| dropped.constraint.as_str() == "identity.filmmaker")
            .map(|dropped| (dropped.reason.clone(), dropped.degradation))
            .unwrap()
        };
        let (with_references, degradation) = reason(&minimax());
        assert!(with_references.starts_with("no approved reference images exist"));
        assert_eq!(degradation, DegradationLevel::IdentityAffecting);
        let (text_only, degradation) = reason(&ImageCapabilities::default());
        assert!(text_only.starts_with("the provider takes no identity reference image"));
        assert_eq!(degradation, DegradationLevel::IdentityAffecting);
    }

    #[test]
    fn a_minimal_intent_names_only_what_was_written() {
        let shot = ShotIntent {
            action: "Rain on an empty street".to_owned(),
            ..ShotIntent::default()
        };
        let compiled = compile(
            None,
            Some(&shot),
            ShotDirection::SpatialClarity,
            Locks::default(),
            &minimax(),
        )
        .unwrap();
        assert_eq!(
            compiled.compiled.prompt(),
            "Rain on an empty street. Keep the geography of the space and where each person \
             stands readable. Photorealistic cinematic film still, natural light, no text, no \
             watermark."
        );
        assert!(compiled.compiled.dropped_constraints().is_empty());
    }

    fn style() -> StyleLock {
        StyleLock {
            schema_version: 1,
            artifact_id: "style_s0002".to_owned(),
            content_hash: format!("sha256:{}", "c".repeat(64)),
            preset: "film_noir".to_owned(),
            preset_version: 1,
            directives: "Black-and-white film noir still".to_owned(),
            note: Some("colder".to_owned()),
        }
    }

    fn cast(id: &str, name: &str) -> CharacterLock {
        let image = |kind, number: u8| LockedImage {
            kind,
            artifact_id: format!("cast.{id}.000{number}"),
            content_hash: format!(
                "sha256:{}",
                char::from(b'0' + number).to_string().repeat(64)
            ),
            media_path: format!("characters/{id}/candidates/cast.{id}.000{number}.jpg"),
        };
        CharacterLock {
            schema_version: 1,
            character_id: id.to_owned(),
            name: name.to_owned(),
            portrait: image(kineto_project::cast::CastImageKind::Portrait, 1),
            references: vec![image(kineto_project::cast::CastImageKind::Front, 2)],
            provider_reference: kineto_project::cast::ProviderReference {
                provider: "minimax".to_owned(),
                mechanism: "subject_reference".to_owned(),
                role: "identity".to_owned(),
                artifact_id: format!("cast.{id}.0001"),
            },
            forbidden_changes: vec!["Grey at the temples.".to_owned()],
            dependencies: Vec::new(),
        }
    }

    fn two_hander() -> ShotIntent {
        ShotIntent {
            characters: vec![
                "filmmaker".to_owned(),
                "interviewer".to_owned(),
                "waiter".to_owned(),
            ],
            ..shot()
        }
    }

    fn dropped(prompt: &KeyframePrompt) -> Vec<(&str, DegradationLevel)> {
        prompt
            .compiled
            .dropped_constraints()
            .iter()
            .map(|dropped| (dropped.constraint.as_str(), dropped.degradation))
            .collect()
    }

    #[test]
    fn the_locked_style_leads_and_a_shot_without_people_loses_no_identity() {
        let empty = ShotIntent {
            characters: Vec::new(),
            ..shot()
        };
        let style = style();
        let prompt = compile(
            Some(&scene()),
            Some(&empty),
            ShotDirection::SpatialClarity,
            Locks {
                style: Some(&style),
                characters: &[],
            },
            &minimax(),
        )
        .unwrap();
        let text = prompt.compiled.prompt();
        assert!(text.starts_with("Black-and-white film noir still. Style note: colder. Medium"));
        assert!(text.ends_with(" readable. No text, no watermark."));
        assert!(!text.contains("Cinematic film still"));
        assert!(!text.contains("Photorealistic"));
        assert_eq!(prompt.compiled.degradation(), DegradationLevel::Cosmetic);
        assert_eq!(prompt.identity, None);
        assert_eq!(
            prompt
                .dependencies
                .iter()
                .map(|dependency| dependency.artifact_id.as_str())
                .collect::<Vec<_>>(),
            ["style_s0002"]
        );
    }

    #[test]
    fn one_cast_character_is_shown_from_their_portrait() {
        let locks = [cast("filmmaker", "The Filmmaker")];
        let prompt = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks {
                style: None,
                characters: &locks,
            },
            &minimax(),
        )
        .unwrap();
        let text = prompt.compiled.prompt();
        assert!(text.contains("The Filmmaker is the person in the reference image."));
        assert!(text.contains("The Filmmaker, always: Grey at the temples."));
        assert_eq!(prompt.identity.as_ref(), Some(&locks[0].portrait));
        let applied: Vec<_> = prompt
            .compiled
            .applied_constraints()
            .iter()
            .map(ConstraintId::as_str)
            .collect();
        assert!(applied.contains(&"identity.filmmaker"));
        assert!(applied.contains(&"forbidden.filmmaker"));
        assert_eq!(prompt.compiled.degradation(), DegradationLevel::Cosmetic);
        assert_eq!(
            prompt.dependencies[0].artifact_id.as_str(),
            "cast.filmmaker.0001"
        );
        assert_eq!(
            prompt.dependencies[0].field.as_deref(),
            Some("character.filmmaker")
        );

        // A lock that changes changes the prompt's hash.
        let mut recast = locks.clone();
        recast[0].forbidden_changes.clear();
        let again = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks {
                style: None,
                characters: &recast,
            },
            &minimax(),
        )
        .unwrap();
        assert_ne!(again.prompt_hash, prompt.prompt_hash);
        assert_ne!(again.dependencies, prompt.dependencies);
    }

    #[test]
    fn beyond_the_references_a_provider_takes_identity_is_text_and_said() {
        let locks = [
            cast("filmmaker", "The Filmmaker"),
            cast("interviewer", "The Interviewer"),
        ];
        let prompt = compile(
            Some(&scene()),
            Some(&two_hander()),
            ShotDirection::Reaction,
            Locks {
                style: None,
                characters: &locks,
            },
            &minimax(),
        )
        .unwrap();
        assert_eq!(prompt.identity.as_ref(), Some(&locks[0].portrait));
        let reasons: Vec<_> = prompt
            .compiled
            .dropped_constraints()
            .iter()
            .filter(|dropped| dropped.constraint.as_str().starts_with("identity."))
            .map(|dropped| (dropped.constraint.as_str(), dropped.reason.as_str()))
            .collect();
        assert_eq!(reasons.len(), 2);
        assert_eq!(reasons[0].0, "identity.interviewer");
        assert!(
            reasons[0]
                .1
                .starts_with("one identity reference is sent per image")
        );
        assert_eq!(reasons[1].0, "identity.waiter");
        assert!(
            reasons[1]
                .1
                .starts_with("no approved reference images exist for waiter")
        );
        assert_eq!(
            prompt.compiled.degradation(),
            DegradationLevel::IdentityAffecting
        );
        assert_eq!(prompt.dependencies.len(), 2, "both casts are followed");

        // A provider that takes no reference shows nobody from a portrait.
        let text_only = ImageCapabilities {
            reference_roles: Default::default(),
            max_reference_images: 0,
            ..minimax()
        };
        let prompt = compile(
            Some(&scene()),
            Some(&two_hander()),
            ShotDirection::Reaction,
            Locks {
                style: None,
                characters: &locks,
            },
            &text_only,
        )
        .unwrap();
        assert_eq!(prompt.identity, None);
        assert!(
            prompt
                .compiled
                .dropped_constraints()
                .iter()
                .filter(|dropped| dropped.constraint.as_str().starts_with("identity."))
                .all(|dropped| dropped
                    .reason
                    .starts_with("the provider takes no identity reference image"))
        );
    }

    #[test]
    fn forbidden_changes_that_do_not_fit_are_dropped_as_identity_affecting() {
        let mut lock = cast("filmmaker", "The Filmmaker");
        lock.forbidden_changes = vec!["Never without the coat. ".repeat(80)];
        let locks = [lock];
        let prompt = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks {
                style: None,
                characters: &locks,
            },
            &minimax(),
        )
        .unwrap();
        assert!(
            dropped(&prompt)
                .contains(&("forbidden.filmmaker", DegradationLevel::IdentityAffecting))
        );
        assert!(
            prompt.compiled.prompt().chars().count() <= kineto_provider_minimax::MAX_PROMPT_CHARS
        );
        assert_eq!(prompt.identity.as_ref(), Some(&locks[0].portrait));
    }
}
