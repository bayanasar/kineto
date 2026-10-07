//! Compile a shot's written intent, and what production has locked, into
//! the prompt for one keyframe.
//!
//! The intent stays expressive; the compiler reports what a still image cannot
//! carry instead of deleting it. Camera motion, dialogue and duration belong
//! to the video step and are dropped as cosmetic.
//!
//! The locked style leads the prompt. A character whose cast is locked is
//! shown from their canonical portrait, sent as an identity reference, up to
//! as many references as the provider declares it takes in one image, in the
//! shot's order. Every other named character is described in text only, an
//! identity-affecting drop: two takes of the same shot can show different
//! faces. A cast character's forbidden changes are
//! said as long as the prompt has room, and dropping them is
//! identity-affecting too.
//!
//! Continuity comes from the locked bible and the shot's plan: what each
//! character wears in this scene, how the location looks and what never
//! changes there, the props the scene holds, the shot's own continuity
//! notes, and the shot it continues from. Each is said as long as the prompt
//! has room; a cut one is reported, props first, then the location, then
//! wardrobe and notes, and a cast character's forbidden changes last.
//! Nothing is truncated.
//!
//! The locked keyframe of the shot it continues from is sent as a
//! composition reference to a provider that declares it takes one, when a
//! reference image is left after the cast's identities, and the keyframe
//! then depends on it. Otherwise the shot follows it in words only, a
//! cosmetic drop that says why.

use std::{error::Error, fmt};

use kineto_project::{
    ArtifactDependency, ArtifactId, ContentHash, DependencyImpact,
    bible::Bible,
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

use crate::{
    preferences::{InForce, KEYFRAME_DIMENSIONS},
    style_prompt::{Part, fit, sentence},
};

/// Version recorded with every keyframe compiled by this module. Change it
/// whenever the same intent would compile to a different prompt.
pub const KEYFRAME_COMPILER_VERSION: &str = "keyframe-prompt/4";

const INTENT_DOMAIN: &[u8] = b"kineto.keyframe-intent.v3";

/// What production has locked that a keyframe follows.
#[derive(Debug, Clone, Copy, Default)]
pub struct Locks<'a> {
    pub style: Option<&'a StyleLock>,
    /// The locked casts of characters, in any order.
    pub characters: &'a [CharacterLock],
    /// The locked bible, whose entries say what carries over.
    pub bible: Option<&'a Bible>,
    /// The director's note for this generation only, such as "warmer light,
    /// tighter on his hands".
    pub note: Option<&'a str>,
    /// The director's preferences in force, read for what a still can show.
    pub preferences: Option<&'a InForce>,
    /// The locked keyframe of the shot this one continues from, if it has
    /// one.
    pub previous: Option<&'a PreviousFrame>,
}

/// The dependency field a keyframe records on the locked keyframe it was
/// sent as a composition reference.
pub const PREVIOUS_FRAME_FIELD: &str = "continuity.previous_frame";

/// The locked keyframe of the shot a shot continues from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviousFrame {
    /// The shot whose keyframe it is.
    pub shot_id: String,
    pub artifact_id: ArtifactId,
    pub content_hash: ContentHash,
    /// Its image, relative to the project.
    pub media_path: String,
}

impl PreviousFrame {
    /// What a keyframe sent it as a reference records.
    #[must_use]
    pub fn dependency(&self) -> ArtifactDependency {
        ArtifactDependency {
            artifact_id: self.artifact_id.clone(),
            content_hash: self.content_hash.clone(),
            impact: DependencyImpact::Semantic,
            field: Some(PREVIOUS_FRAME_FIELD.to_owned()),
        }
    }
}

/// The bible entries a shot's keyframe reads for continuity: the entry of
/// each character it shows, of its scene's location, and of each prop the
/// scene holds. A keyframe depends on each.
#[must_use]
pub fn continuity_dependencies(
    scene: Option<&SceneIntent>,
    shot: Option<&ShotIntent>,
    bible: Option<&Bible>,
) -> Vec<ArtifactDependency> {
    let Some(bible) = bible else {
        return Vec::new();
    };
    let mut dependencies: Vec<ArtifactDependency> = shot
        .map_or(&[][..], |shot| shot.characters.as_slice())
        .iter()
        .filter_map(|character| bible.character_dependency(character))
        .collect();
    if let Some(location) = scene.and_then(|scene| scene.location.as_deref()) {
        dependencies.extend(bible.location_dependency(location));
    }
    for prop in props_in(bible, scene) {
        dependencies.extend(bible.prop_dependency(&prop.id));
    }
    dependencies
}

/// The props the bible places in `scene`.
fn props_in<'a>(
    bible: &'a Bible,
    scene: Option<&'a SceneIntent>,
) -> impl Iterator<Item = &'a kineto_project::bible::Prop> {
    let scene_id = scene.map(|scene| scene.id.as_str()).unwrap_or_default();
    bible
        .props
        .iter()
        .filter(move |prop| !scene_id.is_empty() && prop.scenes.iter().any(|id| id == scene_id))
}

/// A compiled keyframe prompt with the hash that ties it to its intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyframePrompt {
    pub compiled: CompiledPrompt,
    pub prompt_hash: PromptHash,
    /// The locks the prompt follows, which a keyframe made from it depends
    /// on.
    pub dependencies: Vec<ArtifactDependency>,
    /// The canonical portraits to send as identity references, at most as
    /// many as the provider takes, in the shot's order.
    pub identities: Vec<LockedImage>,
    /// The previous shot's locked keyframe, to send as a composition
    /// reference.
    pub composition: Option<PreviousFrame>,
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

    let limit = usize::from(capabilities.reference_limit(ReferenceRole::Identity));
    let takes_identity = limit > 0;
    let mut identities: Vec<LockedImage> = Vec::new();
    for character in &shot.characters {
        let lock = locks
            .characters
            .iter()
            .find(|lock| &lock.character_id == character);
        if let Some(lock) = lock {
            dependencies.extend(cast::lock_dependency(lock));
        }
        let reason = match lock {
            Some(lock) if identities.len() < limit => {
                identities.push(lock.portrait.clone());
                needed(
                    &mut required,
                    &format!("identity.{character}"),
                    format!("{} is the person in the reference image", lock.name),
                )?;
                None
            }
            Some(_) if takes_identity => Some(format!(
                "the provider takes {limit} identity reference{} per image, given to characters \
                 before {character} in the shot; {character} is described in text only and can \
                 drift between takes and shots",
                if limit == 1 { "" } else { "s" }
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
                // Identity outlasts every continuity part when room runs out.
                rank: 3,
                degradation: DegradationLevel::IdentityAffecting,
            });
        }
    }
    if let Some(note) = locks.note.map(str::trim).filter(|note| !note.is_empty()) {
        needed(
            &mut required,
            "director.note",
            format!("Director's note: {}", clause(note)),
        )?;
    }
    let mut composition = None;
    if let Some(from) = &shot.continuity_from {
        needed(
            &mut required,
            "continuity.from",
            format!(
                "It continues directly from shot {}: keep its light, framing and wardrobe",
                words(from)
            ),
        )?;
        let room = usize::from(capabilities.max_reference_images).saturating_sub(identities.len());
        let previous = locks.previous.filter(|frame| &frame.shot_id == from);
        let reason = match previous {
            None => Some(format!(
                "shot {from} has no locked keyframe yet; the shot follows it in words only"
            )),
            Some(_) if capabilities.reference_limit(ReferenceRole::Composition) == 0 => {
                Some(format!(
                    "the provider takes no composition reference; shot {from}'s keyframe is not \
                     sent and the shot follows it in words only"
                ))
            }
            Some(_) if room == 0 => Some(format!(
                "the provider takes {} reference image{} per image, all given to the cast's \
                 identities; shot {from}'s keyframe is not sent and the shot follows it in \
                 words only",
                capabilities.max_reference_images,
                if capabilities.max_reference_images == 1 {
                    ""
                } else {
                    "s"
                }
            )),
            Some(frame) => {
                needed(
                    &mut required,
                    PREVIOUS_FRAME_FIELD,
                    format!(
                        "The composition reference image is the keyframe of shot {}",
                        words(from)
                    ),
                )?;
                dependencies.push(frame.dependency());
                composition = Some(frame.clone());
                None
            }
        };
        if let Some(reason) = reason {
            dropped.push(DroppedConstraint {
                constraint: constraint(PREVIOUS_FRAME_FIELD)?,
                reason,
                degradation: DegradationLevel::Cosmetic,
            });
        }
    }
    if let Some(bible) = locks.bible {
        let scene_id = scene.map(|scene| scene.id.as_str()).unwrap_or_default();
        for character in &shot.characters {
            let Some(entry) = bible.character(character) else {
                continue;
            };
            let wardrobe = if scene_id.is_empty() {
                entry.wardrobe.as_str()
            } else {
                entry.wardrobe_in(scene_id)
            };
            optional.push(Part {
                constraint: format!("wardrobe.{character}"),
                text: sentence(&format!("{} wears {}", entry.name, clause(wardrobe))),
                rank: 2,
                degradation: DegradationLevel::Cosmetic,
            });
        }
        if let Some(location) = scene
            .and_then(|scene| scene.location.as_deref())
            .and_then(|id| bible.location(id))
        {
            optional.push(Part {
                constraint: format!("location.{}", location.id),
                text: sentence(&format!(
                    "{}: {}",
                    location.name,
                    clause(&location.appearance)
                )),
                rank: 1,
                degradation: DegradationLevel::Cosmetic,
            });
            if !location.forbidden_changes.is_empty() {
                optional.push(Part {
                    constraint: format!("location_forbidden.{}", location.id),
                    text: sentence(&format!(
                        "{}, always: {}",
                        location.name,
                        location
                            .forbidden_changes
                            .iter()
                            .map(|change| clause(change))
                            .collect::<Vec<_>>()
                            .join("; ")
                    )),
                    rank: 1,
                    degradation: DegradationLevel::Cosmetic,
                });
            }
        }
        for prop in props_in(bible, scene) {
            optional.push(Part {
                constraint: format!("prop.{}", prop.id),
                text: sentence(&format!("{}: {}", prop.name, clause(&prop.appearance))),
                rank: 0,
                degradation: DegradationLevel::Cosmetic,
            });
        }
        dependencies.extend(continuity_dependencies(scene, Some(shot), Some(bible)));
    }
    if !shot.continuity.is_empty() {
        optional.push(Part {
            constraint: "shot.continuity".to_owned(),
            text: sentence(&format!(
                "Keep: {}",
                shot.continuity
                    .iter()
                    .map(|note| clause(note))
                    .collect::<Vec<_>>()
                    .join("; ")
            )),
            rank: 2,
            degradation: DegradationLevel::Cosmetic,
        });
    }
    // The director's preferences give way first, and to the locked style.
    if let Some(in_force) = locks.preferences
        && let Some(text) = in_force.sentence(&KEYFRAME_DIMENSIONS)
    {
        optional.push(Part {
            constraint: "director.preferences".to_owned(),
            text: sentence(&text),
            rank: 0,
            degradation: DegradationLevel::Cosmetic,
        });
        dependencies.extend(kineto_project::preferences::dependency(&in_force.record));
    }
    for part in &optional {
        ConstraintId::new(part.constraint.as_str())?;
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
        &canonical_intent(scene, shot, direction, locks, composition.as_ref()),
    )?;
    Ok(KeyframePrompt {
        dependencies,
        identities,
        composition,
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

/// A phrase to embed in a sentence: without its closing stop.
fn clause(value: &str) -> &str {
    value.trim().trim_end_matches(['.', ' '])
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
    composition: Option<&PreviousFrame>,
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
    field(scene.map_or(&b""[..], |scene| scene.id.as_bytes()));
    field(&(shot.continuity.len() as u64).to_be_bytes());
    for note in &shot.continuity {
        field(note.as_bytes());
    }
    field(&optional(&shot.continuity_from));
    field(&optional(&locks.note.map(str::to_owned)));
    field(&optional(
        &locks
            .preferences
            .and_then(|in_force| in_force.sentence(&KEYFRAME_DIMENSIONS)),
    ));
    // The entries continuity reads, as they say it now.
    for dependency in continuity_dependencies(scene, Some(shot), locks.bible) {
        field(dependency.artifact_id.as_str().as_bytes());
        field(dependency.content_hash.as_str().as_bytes());
    }
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
    // The frame sent with it, by the bytes it holds. Last and only when
    // sent, so every prompt without one hashes as it always has.
    if let Some(frame) = composition {
        field(frame.content_hash.as_str().as_bytes());
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
            continuity: Vec::new(),
            continuity_from: None,
        }
    }

    fn scene() -> SceneIntent {
        SceneIntent {
            id: "scene_001".to_owned(),
            location: Some("small_office".to_owned()),
            time: Some("night".to_owned()),
            dramatic_goal: None,
            characters: vec!["filmmaker".to_owned()],
        }
    }

    fn minimax() -> ImageCapabilities {
        kineto_provider_minimax::image_capabilities()
    }

    fn bible() -> Bible {
        serde_json::from_str(kineto_project::bible::SAMPLE_BIBLE_JSON).unwrap()
    }

    /// The sample bible, the filmmaker in a wardrobe change for this scene.
    fn dressed_bible() -> Bible {
        let mut bible = bible();
        bible.characters[0].wardrobe_changes = vec![kineto_project::bible::WardrobeChange {
            scenes: vec!["office_night".to_owned()],
            wardrobe: "Shirtsleeves, the coat over the chair.".to_owned(),
        }];
        bible
    }

    fn night() -> SceneIntent {
        SceneIntent {
            id: "office_night".to_owned(),
            ..scene()
        }
    }

    fn continuing() -> ShotIntent {
        ShotIntent {
            continuity: vec!["The lamp is still on.".to_owned()],
            continuity_from: Some("office_night_01".to_owned()),
            ..shot()
        }
    }

    #[test]
    fn continuity_from_the_bible_and_the_plan_reaches_the_prompt() {
        let bible = dressed_bible();
        let prompt = compile(
            Some(&night()),
            Some(&continuing()),
            ShotDirection::Reaction,
            Locks {
                bible: Some(&bible),
                ..Locks::default()
            },
            &minimax(),
        )
        .unwrap();
        let text = prompt.compiled.prompt();
        for said in [
            "It continues directly from shot office night 01: keep its light, framing and wardrobe.",
            "The Filmmaker wears Shirtsleeves, the coat over the chair.",
            "A small office: One table, two chairs, a rain-streaked window, a single desk lamp.",
            "A small office, always: The window is behind the interviewer.",
            "The notebook: A small black notebook with a red elastic band.",
            "Keep: The lamp is still on.",
        ] {
            assert!(text.contains(said), "{said:?} in {text:?}");
        }
        let applied: Vec<_> = prompt
            .compiled
            .applied_constraints()
            .iter()
            .map(ConstraintId::as_str)
            .collect();
        for constraint in [
            "continuity.from",
            "wardrobe.filmmaker",
            "location.small_office",
            "location_forbidden.small_office",
            "prop.notebook",
            "shot.continuity",
        ] {
            assert!(applied.contains(&constraint), "{constraint}");
        }
        assert!(prompt.compiled.dropped_constraints().iter().any(|dropped| {
            dropped.constraint.as_str() == "continuity.previous_frame"
                && dropped.degradation == DegradationLevel::Cosmetic
        }));
        let depends_on: Vec<_> = prompt
            .dependencies
            .iter()
            .map(|dependency| dependency.artifact_id.as_str())
            .collect();
        assert_eq!(
            depends_on,
            [
                "bible_character.filmmaker",
                "bible_location.small_office",
                "bible_prop.notebook"
            ]
        );
        assert_eq!(
            prompt.dependencies,
            continuity_dependencies(Some(&night()), Some(&continuing()), Some(&bible)),
            "staleness reads what compilation read"
        );

        // A change to an entry the shot reads changes the prompt's hash.
        let mut changed = bible.clone();
        changed.locations[0].appearance = "A bare office.".to_owned();
        let again = compile(
            Some(&night()),
            Some(&continuing()),
            ShotDirection::Reaction,
            Locks {
                bible: Some(&changed),
                ..Locks::default()
            },
            &minimax(),
        )
        .unwrap();
        assert_ne!(again.prompt_hash, prompt.prompt_hash);
        // Another scene's wardrobe change does not apply here.
        let usual = compile(
            Some(&SceneIntent {
                id: "office_dawn".to_owned(),
                ..scene()
            }),
            Some(&continuing()),
            ShotDirection::Reaction,
            Locks {
                bible: Some(&bible),
                ..Locks::default()
            },
            &minimax(),
        )
        .unwrap();
        assert!(
            usual
                .compiled
                .prompt()
                .contains("The Filmmaker wears A dark wool coat over a black sweater.")
        );
        assert!(
            !usual.compiled.prompt().contains("notebook"),
            "the notebook is in another scene"
        );
    }

    #[test]
    fn the_directors_note_is_said_for_its_generation_and_changes_the_hash() {
        let plain = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks::default(),
            &minimax(),
        )
        .unwrap();
        let noted = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks {
                note: Some(" Warmer light, tighter on his hands. "),
                ..Locks::default()
            },
            &minimax(),
        )
        .unwrap();
        assert!(
            noted
                .compiled
                .prompt()
                .contains("Director's note: Warmer light, tighter on his hands.")
        );
        assert!(
            noted
                .compiled
                .applied_constraints()
                .iter()
                .any(|applied| applied.as_str() == "director.note")
        );
        assert_ne!(noted.prompt_hash, plain.prompt_hash);
        let blank = compile(
            Some(&scene()),
            Some(&shot()),
            ShotDirection::Reaction,
            Locks {
                note: Some("  "),
                ..Locks::default()
            },
            &minimax(),
        )
        .unwrap();
        assert_eq!(blank.compiled.prompt(), plain.compiled.prompt());
    }

    #[test]
    fn identity_is_cut_after_every_continuity_part() {
        let bible = dressed_bible();
        let casts = [cast("filmmaker", "The Filmmaker")];
        let locks = Locks {
            characters: &casts,
            bible: Some(&bible),
            ..Locks::default()
        };
        let full = compile(
            Some(&night()),
            Some(&continuing()),
            ShotDirection::Reaction,
            locks,
            &minimax(),
        )
        .unwrap();
        let forbidden = "The Filmmaker, always: Grey at the temples.";
        assert!(full.compiled.prompt().contains(forbidden));
        // Room for everything but the prop and one more part: the second
        // to go is a continuity part, not the forbidden change.
        let prop = "The notebook: A small black notebook with a red elastic band.";
        assert!(full.compiled.prompt().contains(prop));
        let tight = ImageCapabilities {
            max_prompt_chars: Some(
                full.compiled.prompt().chars().count() - prop.chars().count() - 2,
            ),
            ..minimax()
        };
        let cut = compile(
            Some(&night()),
            Some(&continuing()),
            ShotDirection::Reaction,
            locks,
            &tight,
        )
        .unwrap();
        assert!(cut.compiled.prompt().contains(forbidden));
        assert!(!cut.compiled.prompt().contains(prop));
        assert_eq!(
            cut.compiled.dropped_constraints().len(),
            full.compiled.dropped_constraints().len() + 2
        );
        let identity_cut = cut
            .compiled
            .dropped_constraints()
            .iter()
            .filter(|dropped| dropped.degradation == DegradationLevel::IdentityAffecting)
            .count();
        assert_eq!(identity_cut, 0);
    }

    #[test]
    fn continuity_is_cut_props_first_when_the_prompt_has_no_room() {
        let bible = dressed_bible();
        let full = compile(
            Some(&night()),
            Some(&continuing()),
            ShotDirection::Reaction,
            Locks {
                bible: Some(&bible),
                ..Locks::default()
            },
            &minimax(),
        )
        .unwrap();
        let length = full.compiled.prompt().chars().count();
        let tight = ImageCapabilities {
            max_prompt_chars: Some(length - 10),
            ..minimax()
        };
        let cut = compile(
            Some(&night()),
            Some(&continuing()),
            ShotDirection::Reaction,
            Locks {
                bible: Some(&bible),
                ..Locks::default()
            },
            &tight,
        )
        .unwrap();
        let dropped: Vec<_> = cut
            .compiled
            .dropped_constraints()
            .iter()
            .map(|dropped| dropped.constraint.as_str())
            .collect();
        assert!(dropped.contains(&"prop.notebook"), "{dropped:?}");
        assert!(!dropped.contains(&"wardrobe.filmmaker"), "{dropped:?}");
        // What was cut was still read: the keyframe depends on it.
        assert!(
            cut.dependencies
                .iter()
                .any(|dependency| dependency.artifact_id.as_str() == "bible_prop.notebook")
        );
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
                bible: None,
                note: None,
                preferences: None,
                previous: None,
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
        assert!(prompt.identities.is_empty());
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
                bible: None,
                note: None,
                preferences: None,
                previous: None,
            },
            &minimax(),
        )
        .unwrap();
        let text = prompt.compiled.prompt();
        assert!(text.contains("The Filmmaker is the person in the reference image."));
        assert!(text.contains("The Filmmaker, always: Grey at the temples."));
        assert_eq!(prompt.identities, [locks[0].portrait.clone()]);
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
                bible: None,
                note: None,
                preferences: None,
                previous: None,
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
                bible: None,
                note: None,
                preferences: None,
                previous: None,
            },
            &minimax(),
        )
        .unwrap();
        assert_eq!(prompt.identities, [locks[0].portrait.clone()]);
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
                .starts_with("the provider takes 1 identity reference per image")
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
                bible: None,
                note: None,
                preferences: None,
                previous: None,
            },
            &text_only,
        )
        .unwrap();
        assert!(prompt.identities.is_empty());
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
    fn a_provider_that_takes_more_references_gets_them_in_shot_order() {
        let two = ImageCapabilities {
            max_reference_images: 2,
            ..minimax()
        };
        let locks = [
            cast("waiter", "The Waiter"),
            cast("interviewer", "The Interviewer"),
            cast("filmmaker", "The Filmmaker"),
        ];
        let prompt = compile(
            Some(&scene()),
            Some(&two_hander()),
            ShotDirection::Reaction,
            Locks {
                style: None,
                characters: &locks,
                bible: None,
                note: None,
                preferences: None,
                previous: None,
            },
            &two,
        )
        .unwrap();
        assert_eq!(
            prompt.identities,
            [locks[2].portrait.clone(), locks[1].portrait.clone()],
            "the first two of the shot, not of the locks"
        );
        let text = prompt.compiled.prompt();
        assert!(text.contains("The Filmmaker is the person in the reference image."));
        assert!(text.contains("The Interviewer is the person in the reference image."));
        let identity_drops: Vec<_> = prompt
            .compiled
            .dropped_constraints()
            .iter()
            .filter(|dropped| dropped.constraint.as_str().starts_with("identity."))
            .map(|dropped| (dropped.constraint.as_str(), dropped.reason.as_str()))
            .collect();
        assert_eq!(identity_drops.len(), 1);
        assert_eq!(identity_drops[0].0, "identity.waiter");
        assert!(
            identity_drops[0]
                .1
                .starts_with("the provider takes 2 identity references per image")
        );

        // Room for more references than cast characters: nobody is dropped
        // for the limit, only the one who is not cast.
        let three = ImageCapabilities {
            max_reference_images: 3,
            ..minimax()
        };
        let prompt = compile(
            Some(&scene()),
            Some(&two_hander()),
            ShotDirection::Reaction,
            Locks {
                style: None,
                characters: &locks[1..],
                bible: None,
                note: None,
                preferences: None,
                previous: None,
            },
            &three,
        )
        .unwrap();
        assert_eq!(prompt.identities.len(), 2);
        assert!(
            prompt
                .compiled
                .dropped_constraints()
                .iter()
                .filter(|dropped| dropped.constraint.as_str().starts_with("identity."))
                .all(|dropped| dropped
                    .reason
                    .starts_with("no approved reference images exist for waiter"))
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
                bible: None,
                note: None,
                preferences: None,
                previous: None,
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
        assert_eq!(prompt.identities, [locks[0].portrait.clone()]);
    }

    fn previous_frame(shot_id: &str) -> PreviousFrame {
        PreviousFrame {
            shot_id: shot_id.to_owned(),
            artifact_id: ArtifactId::new("office_night_01_g0001_candidate_b").unwrap(),
            content_hash: ContentHash::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
            media_path: "scenes/office_night/shots/office_night_01/candidates/b.png".to_owned(),
        }
    }

    /// A provider that takes `max` references, of identity and composition.
    fn composing(max: u16) -> ImageCapabilities {
        ImageCapabilities {
            max_reference_images: max,
            reference_roles: [ReferenceRole::Identity, ReferenceRole::Composition]
                .into_iter()
                .collect(),
            ..minimax()
        }
    }

    fn previous_frame_drop(prompt: &KeyframePrompt) -> Option<&str> {
        prompt
            .compiled
            .dropped_constraints()
            .iter()
            .find(|dropped| dropped.constraint.as_str() == PREVIOUS_FRAME_FIELD)
            .map(|dropped| {
                assert_eq!(dropped.degradation, DegradationLevel::Cosmetic);
                dropped.reason.as_str()
            })
    }

    #[test]
    fn the_previous_locked_frame_is_sent_where_the_provider_takes_it() {
        let frame = previous_frame("office_night_01");
        let locks = [cast("filmmaker", "The Filmmaker")];
        let with = |previous: Option<&PreviousFrame>, capabilities: &ImageCapabilities| {
            compile(
                Some(&night()),
                Some(&continuing()),
                ShotDirection::Reaction,
                Locks {
                    characters: &locks,
                    previous,
                    ..Locks::default()
                },
                capabilities,
            )
            .unwrap()
        };

        // Room after the cast's identity: the frame goes as composition,
        // and the keyframe depends on it.
        let sent = with(Some(&frame), &composing(2));
        assert_eq!(sent.identities, [locks[0].portrait.clone()]);
        assert_eq!(sent.composition.as_ref(), Some(&frame));
        assert!(previous_frame_drop(&sent).is_none());
        assert!(
            sent.compiled
                .applied_constraints()
                .iter()
                .any(|applied| applied.as_str() == PREVIOUS_FRAME_FIELD)
        );
        assert!(
            sent.compiled.prompt().contains(
                "The composition reference image is the keyframe of shot office night 01."
            )
        );
        assert_eq!(sent.dependencies.last(), Some(&frame.dependency()));

        // The frame's bytes are part of what the prompt is for.
        let other = PreviousFrame {
            content_hash: ContentHash::new(format!("sha256:{}", "c".repeat(64))).unwrap(),
            ..frame.clone()
        };
        assert_ne!(
            with(Some(&other), &composing(2)).prompt_hash,
            sent.prompt_hash
        );

        // Otherwise it is not sent, nothing depends on it, and the drop says
        // why.
        for (previous, capabilities, why) in [
            (
                None,
                composing(2),
                "shot office_night_01 has no locked keyframe yet",
            ),
            (
                Some(previous_frame("office_night_07")),
                composing(2),
                "shot office_night_01 has no locked keyframe yet",
            ),
            (
                Some(frame.clone()),
                minimax(),
                "the provider takes no composition reference",
            ),
            (
                Some(frame.clone()),
                composing(1),
                "the provider takes 1 reference image per image, all given to the cast's identities",
            ),
        ] {
            let prompt = with(previous.as_ref(), &capabilities);
            assert!(prompt.composition.is_none(), "{why}");
            assert!(
                previous_frame_drop(&prompt).is_some_and(|reason| reason.starts_with(why)),
                "{why}: {:?}",
                previous_frame_drop(&prompt)
            );
            assert!(
                prompt
                    .dependencies
                    .iter()
                    .all(|dependency| dependency.field.as_deref() != Some(PREVIOUS_FRAME_FIELD))
            );
            assert_ne!(prompt.prompt_hash, sent.prompt_hash, "{why}");
        }
    }
}
