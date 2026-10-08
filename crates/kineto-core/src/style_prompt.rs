//! Style presets, and the prompt a style sample is generated from.
//!
//! A preset is a tested description of how every image of a film looks. A
//! sample shows one preset, with an optional note from the director, on a
//! scene of the locked screenplay, described from the bible when there is
//! one, so the director judges the style on their own film. The scene is
//! the material; the style is what is being explored.
//!
//! Nothing is truncated. What the provider cannot take is reported as
//! dropped: the preset's negative description when the provider takes no
//! negative prompt, and, when the prompt would be too long, the least
//! important descriptions first: the world's rules, the action, the
//! details of each person (from the last), the place, and last the people
//! themselves (from the last), since a style is best judged on the film's
//! own people. A prompt still too long is refused.

use kineto_project::{
    bible::Bible,
    screenplay::{Scene, Screenplay},
};
use kineto_prompts::{CompiledPrompt, ConstraintId, DegradationLevel, DroppedConstraint};
use kineto_providers::ImageCapabilities;
use sha2::{Digest, Sha256};

use crate::keyframe_prompt::{KeyframePrompt, KeyframePromptError};

/// Version recorded with every sample compiled by this module. Change it
/// whenever the same intent would compile to a different prompt.
pub const STYLE_COMPILER_VERSION: &str = "style-sample/1";

const INTENT_DOMAIN: &[u8] = b"kineto.style-sample-intent.v1";

/// A tested style direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StylePreset {
    pub id: &'static str,
    pub name: &'static str,
    /// Changes whenever the directives do.
    pub version: u32,
    /// How every image in this style looks, as the prompt says it.
    pub directives: &'static str,
    /// What the style must not look like, for providers that take a
    /// negative prompt.
    pub avoid: &'static str,
}

/// The presets a director chooses directions from.
pub const PRESETS: [StylePreset; 4] = [
    StylePreset {
        id: "film_realism",
        name: "Realist film",
        version: 1,
        directives: "Photorealistic film still shot on 35mm colour negative film: natural \
                     motivated light, fine grain, true-to-life muted colour, gentle contrast, \
                     shallow depth of field",
        avoid: "illustration, cartoon, painterly texture, oversaturated colour, plastic skin",
    },
    StylePreset {
        id: "film_noir",
        name: "Film noir",
        version: 1,
        directives: "Black-and-white film noir still: hard low-key light from a single source, \
                     deep black shadows, high contrast, wet reflective surfaces, 1940s \
                     crime-film composition",
        avoid: "colour, soft even light, cheerful mood, modern digital look",
    },
    StylePreset {
        id: "soft_fairy_tale",
        name: "Soft fairy tale",
        version: 1,
        directives: "Soft storybook illustration: hand-painted gouache texture, warm diffuse \
                     glow, gentle bloom on highlights, pastel palette, rounded shapes, a quiet \
                     magical atmosphere",
        avoid: "photorealism, harsh shadows, gore, hard-edged digital rendering",
    },
    StylePreset {
        id: "anime_cel",
        name: "Hand-drawn animation",
        version: 1,
        directives: "Hand-drawn 2D animation cel: clean confident line art, flat colour with \
                     soft two-tone shading, painted watercolour backgrounds, cinematic framing, \
                     expressive faces",
        avoid: "3D render, photorealism, heavy grain, muddy colour",
    },
];

/// The preset with `id`.
#[must_use]
pub fn preset(id: &str) -> Option<&'static StylePreset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// The scene a sample shows when the director names none: the one with the
/// most people in it, the earliest of those.
#[must_use]
pub fn representative_scene(screenplay: &Screenplay) -> Option<&Scene> {
    screenplay
        .scenes
        .iter()
        .enumerate()
        .max_by_key(|(index, scene)| (scene.characters.len(), std::cmp::Reverse(*index)))
        .map(|(_, scene)| scene)
}

/// One part of a prompt and the constraint it carries.
pub(crate) struct Part {
    pub(crate) constraint: String,
    pub(crate) text: String,
    /// Parts of lower rank are dropped first, later parts of one rank
    /// before earlier ones.
    pub(crate) rank: u8,
    /// What dropping it costs.
    pub(crate) degradation: DegradationLevel,
}

const RANK_WORLD: u8 = 0;
const RANK_ACTION: u8 = 1;
const RANK_PERSON_DETAIL: u8 = 2;
const RANK_LOCATION: u8 = 3;
const RANK_PERSON: u8 = 4;

/// The constraint a person's part carries: `character.<id>`, or, for an ID
/// too long for a constraint, the person's place in the scene.
fn person_constraint(id: &str, number: usize, suffix: &str) -> String {
    let named = format!("character.{id}{suffix}");
    if ConstraintId::new(named.as_str()).is_ok() {
        named
    } else {
        format!("character.{number}{suffix}")
    }
}

/// The first sentence of `text`, and the rest.
fn first_sentence(text: &str) -> (&str, &str) {
    match text.find(". ") {
        Some(end) => (&text[..=end], text[end + 2..].trim()),
        None => (text, ""),
    }
}

fn constraint(value: &str) -> Result<ConstraintId, KeyframePromptError> {
    Ok(ConstraintId::new(value)?)
}

pub(crate) fn sentence(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = text.trim_end_matches(['.', ' ']);
    format!("{text}.")
}

/// Compile the prompt of a sample of `preset`, as `note` asks, showing
/// `scene` of `screenplay` as `bible` describes it, for a provider with
/// `capabilities`.
pub fn compile(
    preset: &StylePreset,
    note: Option<&str>,
    scene: &Scene,
    screenplay: &Screenplay,
    bible: Option<&Bible>,
    capabilities: &ImageCapabilities,
) -> Result<KeyframePrompt, KeyframePromptError> {
    let note = note.map(str::trim).filter(|note| !note.is_empty());
    // Kept whatever the length.
    let mut required = vec![Part {
        constraint: "style.preset".to_owned(),
        text: sentence(preset.directives),
        rank: u8::MAX,
        degradation: DegradationLevel::Cosmetic,
    }];
    if let Some(note) = note {
        required.push(Part {
            constraint: "style.note".to_owned(),
            text: sentence(&format!("Director's note: {note}")),
            rank: u8::MAX,
            degradation: DegradationLevel::Cosmetic,
        });
    }
    required.push(Part {
        constraint: "scene.setting".to_owned(),
        text: sentence(&format!("Scene: {}, {}", scene.heading, scene.time)),
        rank: u8::MAX,
        degradation: DegradationLevel::Cosmetic,
    });
    // Dropped by rank when the prompt is too long.
    let mut optional = Vec::new();
    if let Some(location) = bible.and_then(|bible| bible.location(&scene.location)) {
        optional.push(Part {
            constraint: "bible.location".to_owned(),
            text: sentence(&location.appearance),
            rank: RANK_LOCATION,
            degradation: DegradationLevel::Cosmetic,
        });
    }
    if let Some(action) = scene
        .blocks
        .iter()
        .find(|block| block.kind == kineto_project::screenplay::BlockKind::Action)
    {
        optional.push(Part {
            constraint: "scene.action".to_owned(),
            text: sentence(&action.text),
            rank: RANK_ACTION,
            degradation: DegradationLevel::Cosmetic,
        });
    }
    for (number, id) in scene.characters.iter().enumerate() {
        let entry = bible.and_then(|bible| bible.character(id));
        let name = entry.map(|entry| entry.name.as_str()).or_else(|| {
            screenplay
                .cast
                .iter()
                .find(|member| &member.id == id)
                .map(|member| member.name.as_str())
        });
        let Some(name) = name else { continue };
        let number = number + 1;
        let Some(entry) = entry else {
            optional.push(Part {
                constraint: person_constraint(id, number, ""),
                text: sentence(&format!("{name} is in the scene")),
                rank: RANK_PERSON,
                degradation: DegradationLevel::Cosmetic,
            });
            continue;
        };
        let (look, more) = first_sentence(entry.appearance.trim());
        optional.push(Part {
            constraint: person_constraint(id, number, ""),
            text: sentence(&format!("{name}: {look}")),
            rank: RANK_PERSON,
            degradation: DegradationLevel::Cosmetic,
        });
        let wardrobe = entry
            .wardrobe_changes
            .iter()
            .find(|change| change.scenes.contains(&scene.id))
            .map_or(entry.wardrobe.as_str(), |change| change.wardrobe.as_str());
        let wardrobe = sentence(&format!("Wardrobe: {wardrobe}"));
        optional.push(Part {
            constraint: person_constraint(id, number, ".detail"),
            text: if more.is_empty() {
                wardrobe
            } else {
                format!("{} {wardrobe}", sentence(more))
            },
            rank: RANK_PERSON_DETAIL,
            degradation: DegradationLevel::Cosmetic,
        });
    }
    if let Some(bible) = bible
        && !bible.world.visual_rules.is_empty()
    {
        optional.push(Part {
            constraint: "bible.world".to_owned(),
            text: sentence(&bible.world.visual_rules.join(" ")),
            rank: RANK_WORLD,
            degradation: DegradationLevel::Cosmetic,
        });
    }
    let avoid = DroppedConstraint {
        constraint: constraint("style.avoid")?,
        reason: if capabilities.negative_prompt {
            "this build sends no negative prompt".to_owned()
        } else {
            "the provider takes no negative prompt".to_owned()
        },
        degradation: DegradationLevel::Cosmetic,
    };
    fit(
        &required,
        &optional,
        vec![avoid],
        capabilities.max_prompt_chars,
        STYLE_COMPILER_VERSION,
        &canonical_intent(preset, note, scene, bible),
    )
}

/// The prompt of `required` and as many of `optional` as fit in
/// `max_chars`, dropping by rank and reporting each drop after `dropped`.
/// Nothing is truncated: a prompt whose required parts do not fit is
/// refused.
pub(crate) fn fit(
    required: &[Part],
    optional: &[Part],
    mut dropped: Vec<DroppedConstraint>,
    max_chars: Option<usize>,
    compiler_version: &str,
    intent: &[u8],
) -> Result<KeyframePrompt, KeyframePromptError> {
    let mut order: Vec<usize> = (0..optional.len()).collect();
    order.sort_by_key(|index| (optional[*index].rank, std::cmp::Reverse(*index)));
    let mut kept = vec![true; optional.len()];
    let text = |kept: &[bool]| {
        required
            .iter()
            .chain(
                optional
                    .iter()
                    .zip(kept)
                    .filter(|(_, kept)| **kept)
                    .map(|(part, _)| part),
            )
            .map(|part| part.text.as_str())
            // A part may carry a constraint said in another part's text.
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    if let Some(max) = max_chars {
        for index in order {
            if text(&kept).chars().count() <= max {
                break;
            }
            kept[index] = false;
            dropped.push(DroppedConstraint {
                constraint: constraint(&optional[index].constraint)?,
                reason: format!(
                    "the prompt would be longer than the {max} characters the provider takes"
                ),
                degradation: optional[index].degradation,
            });
        }
        let chars = text(&kept).chars().count();
        if chars > max {
            return Err(KeyframePromptError::TooLong { chars, max });
        }
    }
    let applied = required
        .iter()
        .chain(
            optional
                .iter()
                .zip(&kept)
                .filter(|(_, kept)| **kept)
                .map(|(part, _)| part),
        )
        .map(|part| constraint(&part.constraint))
        .collect::<Result<Vec<_>, _>>()?;
    let compiled = CompiledPrompt::new(text(&kept), applied, dropped, compiler_version)?;
    let prompt_hash = compiled.prompt_hash(intent);
    Ok(KeyframePrompt {
        compiled,
        prompt_hash,
        dependencies: Vec::new(),
        identities: Vec::new(),
        composition: None,
        style_image: None,
    })
}

/// A deterministic, length-framed encoding of everything the compiler read
/// that decides the sample: the preset, the note, the scene and the bible.
#[must_use]
pub fn canonical_intent(
    preset: &StylePreset,
    note: Option<&str>,
    scene: &Scene,
    bible: Option<&Bible>,
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
    field(INTENT_DOMAIN);
    field(preset.id.as_bytes());
    field(&preset.version.to_be_bytes());
    field(preset.directives.as_bytes());
    field(note.unwrap_or_default().as_bytes());
    field(scene.content_hash().as_str().as_bytes());
    field(
        &bible
            .map(|bible| serde_json::to_vec(bible).expect("a bible always encodes"))
            .unwrap_or_default(),
    );
    hasher.finalize().to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;
    use kineto_project::{bible::SAMPLE_BIBLE_JSON, screenplay::SAMPLE_SCREENPLAY_JSON};

    fn screenplay() -> Screenplay {
        serde_json::from_str(SAMPLE_SCREENPLAY_JSON).unwrap()
    }

    fn bible() -> Bible {
        serde_json::from_str(SAMPLE_BIBLE_JSON).unwrap()
    }

    fn minimax() -> ImageCapabilities {
        kineto_provider_minimax::image_capabilities()
    }

    fn ids(constraints: &[ConstraintId]) -> Vec<&str> {
        constraints.iter().map(ConstraintId::as_str).collect()
    }

    #[test]
    fn a_sample_shows_the_preset_and_the_note_on_the_films_own_scene() {
        let screenplay = screenplay();
        let bible = bible();
        let scene = representative_scene(&screenplay).unwrap();
        let prompt = compile(
            preset("film_noir").unwrap(),
            Some("  keep the rain  "),
            scene,
            &screenplay,
            Some(&bible),
            &minimax(),
        )
        .unwrap();
        let text = prompt.compiled.prompt();
        assert!(text.starts_with("Black-and-white film noir still: hard low-key light"));
        assert!(text.contains("Director's note: keep the rain."));
        assert!(text.contains("Scene: INT. SMALL OFFICE - NIGHT, night."));
        assert!(text.contains("a rain-streaked window, a single desk lamp."));
        assert!(text.contains(
            "The Filmmaker: A man in his late forties, lean, unshaven, grey at the temples, \
             deep-set eyes. Wardrobe: A dark wool coat over a black sweater."
        ));
        assert!(text.contains(
            "The Interviewer: A woman in her thirties, short black hair, round glasses. \
             Wardrobe: A grey blazer, a notebook always in hand."
        ));
        assert!(text.contains("Practical light only"));
        assert_eq!(
            ids(prompt.compiled.applied_constraints()),
            [
                "style.preset",
                "style.note",
                "scene.setting",
                "bible.location",
                "scene.action",
                "character.filmmaker",
                "character.filmmaker.detail",
                "character.interviewer",
                "character.interviewer.detail",
                "bible.world"
            ]
        );
        let dropped = prompt.compiled.dropped_constraints();
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].constraint.as_str(), "style.avoid");
        assert_eq!(dropped[0].reason, "the provider takes no negative prompt");
        assert_eq!(prompt.compiled.compiler_version(), STYLE_COMPILER_VERSION);

        // Without a bible the people are named from the cast.
        let plain = compile(
            preset("film_noir").unwrap(),
            None,
            scene,
            &screenplay,
            None,
            &minimax(),
        )
        .unwrap();
        assert!(
            plain
                .compiled
                .prompt()
                .contains(" Filmmaker is in the scene.")
        );
        assert_ne!(plain.prompt_hash, prompt.prompt_hash);
    }

    #[test]
    fn a_long_prompt_drops_the_least_important_descriptions_and_says_so() {
        let screenplay = screenplay();
        let bible = bible();
        let scene = &screenplay.scenes[0];
        let full = compile(
            preset("film_realism").unwrap(),
            None,
            scene,
            &screenplay,
            Some(&bible),
            &ImageCapabilities::default(),
        )
        .unwrap();
        let length = full.compiled.prompt().chars().count();
        let world = sentence(&bible.world.visual_rules.join(" "))
            .chars()
            .count();
        let tight = ImageCapabilities {
            max_prompt_chars: Some(length - world),
            ..minimax()
        };
        let prompt = compile(
            preset("film_realism").unwrap(),
            None,
            scene,
            &screenplay,
            Some(&bible),
            &tight,
        )
        .unwrap();
        let dropped: Vec<_> = prompt
            .compiled
            .dropped_constraints()
            .iter()
            .map(|dropped| dropped.constraint.as_str())
            .collect();
        assert_eq!(dropped, ["style.avoid", "bible.world"]);
        assert!(!prompt.compiled.prompt().contains("Practical light"));

        // One character fewer than dropping the world's rules (and the space
        // before them) frees.
        let tighter = ImageCapabilities {
            max_prompt_chars: Some(length - world - 2),
            ..minimax()
        };
        let prompt = compile(
            preset("film_realism").unwrap(),
            None,
            scene,
            &screenplay,
            Some(&bible),
            &tighter,
        )
        .unwrap();
        assert_eq!(
            prompt.compiled.dropped_constraints()[2].constraint.as_str(),
            "scene.action",
            "then the action"
        );

        // With room for little more than the style, the place and the
        // people's first lines are what is left.
        let briefs = ImageCapabilities {
            max_prompt_chars: Some(length - 160),
            ..minimax()
        };
        let prompt = compile(
            preset("film_realism").unwrap(),
            None,
            scene,
            &screenplay,
            Some(&bible),
            &briefs,
        )
        .unwrap();
        let dropped: Vec<_> = prompt
            .compiled
            .dropped_constraints()
            .iter()
            .map(|dropped| dropped.constraint.as_str())
            .collect();
        assert_eq!(
            dropped,
            [
                "style.avoid",
                "bible.world",
                "scene.action",
                "character.interviewer.detail",
                "character.filmmaker.detail"
            ]
        );
        let text = prompt.compiled.prompt();
        assert!(text.contains("The Filmmaker: A man in his late forties"));
        assert!(text.contains("The Interviewer: A woman in her thirties"));
        assert!(text.contains("a single desk lamp."), "the place is kept");
        assert!(!text.contains("A grey blazer"));

        let impossible = ImageCapabilities {
            max_prompt_chars: Some(20),
            ..minimax()
        };
        assert!(matches!(
            compile(
                preset("film_realism").unwrap(),
                None,
                scene,
                &screenplay,
                Some(&bible),
                &impossible,
            ),
            Err(KeyframePromptError::TooLong { max: 20, .. })
        ));
    }

    #[test]
    fn the_representative_scene_has_the_most_people() {
        let mut screenplay = screenplay();
        let mut alone = screenplay.scenes[0].clone();
        alone.id = "corridor".to_owned();
        alone.characters = vec!["filmmaker".to_owned()];
        alone.blocks.retain(|block| block.character.is_none());
        screenplay.scenes.insert(0, alone);
        assert_eq!(
            representative_scene(&screenplay).unwrap().id,
            "office_night"
        );
        assert!(
            PRESETS
                .iter()
                .all(|preset| super::preset(preset.id) == Some(preset))
        );
    }
}
