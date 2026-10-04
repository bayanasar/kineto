//! Workflow recipes: a filmmaking workflow as data.
//!
//! A recipe lists the steps a project goes through, what each produces,
//! which steps it follows and whether the director approves what it made.
//! `project.toml` names the recipe by ID and version; this build runs the
//! recipes it ships and refuses any other, rather than falling back to the
//! default and running a workflow the project did not ask for.
//!
//! A step may name conditions under which it does not apply, such as
//! casting a film whose locked bible has no main character. The engine
//! reads each condition from the project's files; a recipe can only name
//! the ones this build knows.

use std::{collections::BTreeSet, fmt};

use serde::{Deserialize, Serialize};

use crate::manifest::CanonicalProject;

/// `schemas/recipe.schema.json`.
pub const RECIPE_SCHEMA: &str = include_str!("../../../schemas/recipe.schema.json");

/// The recipe a new project follows.
pub const DEFAULT_RECIPE_ID: &str = "default-film";
pub const DEFAULT_RECIPE_VERSION: u32 = 1;

const RECIPE_SCHEMA_VERSION: u32 = 1;

/// The recipes this build ships, as `(id, version, json)`.
const BUILTIN: [(&str, u32, &str); 1] = [(
    DEFAULT_RECIPE_ID,
    DEFAULT_RECIPE_VERSION,
    include_str!("../../../recipes/default-film-1.json"),
)];

/// What a step makes. The engine knows how to tell, from the project's
/// files, whether each kind has been made, approved, or has gone stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Source,
    StoryAnalysis,
    Screenplay,
    Bible,
    Style,
    Cast,
    ShotPlan,
    Keyframe,
    Video,
}

impl ArtifactKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::StoryAnalysis => "story_analysis",
            Self::Screenplay => "screenplay",
            Self::Bible => "bible",
            Self::Style => "style",
            Self::Cast => "cast",
            Self::ShotPlan => "shot_plan",
            Self::Keyframe => "keyframe",
            Self::Video => "video",
        }
    }
}

/// When a step does not apply to a project. Each reads something a step
/// before it makes, and holds only once that is locked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipCondition {
    /// The locked bible has no main character, so there is no one to cast.
    NoMainCharacters,
}

impl SkipCondition {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NoMainCharacters => "no_main_characters",
        }
    }

    /// What the condition reads, which a step before the one it skips must
    /// make.
    #[must_use]
    pub const fn reads(self) -> ArtifactKind {
        match self {
            Self::NoMainCharacters => ArtifactKind::Bible,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    pub id: String,
    pub produces: ArtifactKind,
    /// The steps that must be done first.
    pub after: Vec<String>,
    /// Whether the director approves (locks) what this step made.
    pub approval: bool,
    pub description: String,
    /// The step is skipped once the steps it follows are done and any of
    /// these holds; it then counts as done for the steps after it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skip_when: Vec<SkipCondition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub recipe_id: String,
    pub recipe_version: u32,
    pub name: String,
    /// In an order where every step comes after the steps it follows.
    pub steps: Vec<Step>,
}

#[derive(Debug)]
pub enum RecipeError {
    /// This build does not ship the recipe the project names.
    Unknown {
        id: String,
        version: u32,
    },
    Invalid(String),
    Json(serde_json::Error),
}

impl fmt::Display for RecipeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { id, version } => write!(
                formatter,
                "this Kineto build has no workflow recipe {id} version {version}"
            ),
            Self::Invalid(reason) => write!(formatter, "the workflow recipe is invalid: {reason}"),
            Self::Json(error) => {
                write!(formatter, "the workflow recipe is not valid JSON: {error}")
            }
        }
    }
}

impl std::error::Error for RecipeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            Self::Unknown { .. } | Self::Invalid(_) => None,
        }
    }
}

impl Recipe {
    /// Read and check a recipe.
    pub fn parse(json: &str) -> Result<Self, RecipeError> {
        let recipe: Self = serde_json::from_str(json).map_err(RecipeError::Json)?;
        recipe.validate().map_err(RecipeError::Invalid)?;
        Ok(recipe)
    }

    /// What the schema cannot say: one version of the shape, unique steps,
    /// each kind made by one step, every step after only the steps listed
    /// before it, so the workflow has no cycle, and each skip condition
    /// reading what a step it follows, directly or not, makes.
    fn validate(&self) -> Result<(), String> {
        if self.schema_version != RECIPE_SCHEMA_VERSION {
            return Err(format!(
                "schema version {} is not {RECIPE_SCHEMA_VERSION}",
                self.schema_version
            ));
        }
        if self.steps.is_empty() {
            return Err("there are no steps".to_owned());
        }
        let mut seen = BTreeSet::new();
        let mut kinds = BTreeSet::new();
        // What each step listed so far follows, directly or not, by kind.
        let mut upstream: Vec<(&str, BTreeSet<ArtifactKind>)> = Vec::new();
        for step in &self.steps {
            if step.id.is_empty() || step.description.trim().is_empty() {
                return Err(format!("step {:?} has no id or description", step.id));
            }
            if !kinds.insert(step.produces) {
                return Err(format!("two steps produce {}", step.produces.as_str()));
            }
            let mut after = BTreeSet::new();
            for parent in &step.after {
                if !after.insert(parent.as_str()) {
                    return Err(format!("step {:?} follows {parent:?} twice", step.id));
                }
                if !seen.contains(parent.as_str()) {
                    return Err(format!(
                        "step {:?} follows {parent:?}, which is not listed before it",
                        step.id
                    ));
                }
            }
            if !seen.insert(step.id.as_str()) {
                return Err(format!("step {:?} is listed twice", step.id));
            }
            let mut reached = BTreeSet::new();
            for (id, kinds) in &upstream {
                if step.after.iter().any(|parent| parent == id) {
                    reached.extend(kinds.iter().copied());
                }
            }
            let mut conditions = BTreeSet::new();
            for condition in &step.skip_when {
                if !conditions.insert(*condition) {
                    return Err(format!(
                        "step {:?} names {} twice",
                        step.id,
                        condition.as_str()
                    ));
                }
                if !reached.contains(&condition.reads()) {
                    return Err(format!(
                        "step {:?} is skipped when {}, which reads the {} no step it follows makes",
                        step.id,
                        condition.as_str(),
                        condition.reads().as_str()
                    ));
                }
            }
            reached.insert(step.produces);
            upstream.push((step.id.as_str(), reached));
        }
        Ok(())
    }

    #[must_use]
    pub fn step(&self, id: &str) -> Option<&Step> {
        self.steps.iter().find(|step| step.id == id)
    }
}

/// The recipe `id` at `version`, as this build ships it.
pub fn builtin(id: &str, version: u32) -> Result<Recipe, RecipeError> {
    BUILTIN
        .iter()
        .find(|(known, known_version, _)| *known == id && *known_version == version)
        .ok_or_else(|| RecipeError::Unknown {
            id: id.to_owned(),
            version,
        })
        .and_then(|(_, _, json)| Recipe::parse(json))
}

/// The recipe `project` names in `project.toml`.
pub fn of(project: &CanonicalProject) -> Result<Recipe, RecipeError> {
    let workflow = &project.manifest().workflow;
    builtin(&workflow.recipe_id, workflow.recipe_version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_json() -> serde_json::Value {
        serde_json::from_str(BUILTIN[0].2).unwrap()
    }

    fn parse(value: &serde_json::Value) -> Result<Recipe, String> {
        Recipe::parse(&value.to_string()).map_err(|error| error.to_string())
    }

    #[test]
    fn the_default_recipe_is_the_documented_pipeline() {
        let recipe = builtin(DEFAULT_RECIPE_ID, DEFAULT_RECIPE_VERSION).unwrap();
        assert_eq!(recipe.name, "Default film");
        let steps: Vec<_> = recipe
            .steps
            .iter()
            .map(|step| (step.id.as_str(), step.approval))
            .collect();
        assert_eq!(
            steps,
            [
                ("source", false),
                ("story_analysis", false),
                ("screenplay", true),
                ("bible", true),
                ("style", true),
                ("casting", true),
                ("shot_plan", true),
                ("keyframes", true),
                ("video", true),
            ]
        );
        assert_eq!(recipe.step("casting").unwrap().after, ["bible", "style"]);
        assert_eq!(
            recipe.step("keyframes").unwrap().produces,
            ArtifactKind::Keyframe
        );
        let skippable: Vec<_> = recipe
            .steps
            .iter()
            .filter(|step| !step.skip_when.is_empty())
            .map(|step| (step.id.as_str(), step.skip_when.as_slice()))
            .collect();
        assert_eq!(
            skippable,
            [("casting", &[SkipCondition::NoMainCharacters][..])]
        );
    }

    #[test]
    fn a_recipe_this_build_does_not_ship_is_refused() {
        assert!(matches!(
            builtin(DEFAULT_RECIPE_ID, 2),
            Err(RecipeError::Unknown { version: 2, .. })
        ));
        assert!(matches!(
            builtin("documentary", 1),
            Err(RecipeError::Unknown { .. })
        ));
    }

    #[test]
    fn a_recipe_with_a_cycle_a_stranger_or_a_repeat_is_refused() {
        type Change = fn(&mut serde_json::Value);
        let cases: [(&str, Change); 9] = [
            ("follows a later step", |v| {
                v["steps"][1]["after"] = serde_json::json!(["screenplay"]);
            }),
            ("follows itself", |v| {
                v["steps"][1]["after"] = serde_json::json!(["story_analysis"]);
            }),
            ("follows a step that does not exist", |v| {
                v["steps"][2]["after"] = serde_json::json!(["outline"]);
            }),
            ("two steps make one kind", |v| {
                v["steps"][4]["produces"] = "bible".into();
            }),
            ("a step listed twice", |v| {
                v["steps"][4]["id"] = "bible".into();
            }),
            ("an unknown kind", |v| {
                v["steps"][4]["produces"] = "music".into();
            }),
            ("an unknown skip condition", |v| {
                v["steps"][5]["skip_when"] = serde_json::json!(["no_budget"]);
            }),
            ("a skip condition named twice", |v| {
                v["steps"][5]["skip_when"] =
                    serde_json::json!(["no_main_characters", "no_main_characters"]);
            }),
            (
                "a skip condition reading what no step before it makes",
                |v| {
                    // Style follows only the screenplay; the bible is beside it.
                    v["steps"][4]["skip_when"] = serde_json::json!(["no_main_characters"]);
                },
            ),
        ];
        for (case, change) in cases {
            let mut recipe = default_json();
            change(&mut recipe);
            assert!(parse(&recipe).is_err(), "{case}");
        }
        let mut recipe = default_json();
        recipe["steps"] = serde_json::json!([]);
        assert!(parse(&recipe).is_err());
        assert!(parse(&default_json()).is_ok());
    }

    #[test]
    fn a_skip_condition_may_read_what_a_step_reached_through_others_makes() {
        let mut recipe = default_json();
        // Keyframes follow casting, which follows the bible.
        recipe["steps"][7]["skip_when"] = serde_json::json!(["no_main_characters"]);
        let recipe = parse(&recipe).unwrap();
        assert_eq!(
            recipe.step("keyframes").unwrap().skip_when,
            [SkipCondition::NoMainCharacters]
        );
        // A step with no conditions writes none.
        let json = serde_json::to_value(recipe.step("bible").unwrap()).unwrap();
        assert!(json.get("skip_when").is_none());
    }
}
