//! The shot plan: the shots of every scene of the locked screenplay, as
//! structured state.
//!
//! `schemas/shot-plan.schema.json` is the contract, and these types enforce
//! it on the way in. A plan is made from the locked screenplay and the
//! locked bible: [`ShotPlan::check_against`] holds it to their scenes and
//! cast, and [`staleness`] says what has changed since. A shot's location is
//! its scene's.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactDependency, DependencyImpact,
    bible::Bible,
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, RevisionError, RevisionRecord, RevisionStatus},
    screenplay::{SCENE_ARTIFACT_PREFIX, SceneFreshness, Screenplay},
    shot::ShotDirection,
    story::is_portable_id,
};

/// The committed schemas, as the model is shown them.
pub const SHOT_PLAN_SCHEMA: &str = include_str!("../../../schemas/shot-plan.schema.json");
pub const SCENE_PLAN_SCHEMA: &str = include_str!("../../../schemas/shot-plan-scene.schema.json");

/// Shot IDs leave room for the directories and artifact IDs made from them.
const MAX_SHOT_ID: usize = 64;

/// Target durations a shot may ask for, in seconds.
const DURATION: std::ops::RangeInclusive<f64> = 1.0..=30.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Framing {
    ExtremeWide,
    Wide,
    Full,
    MediumWide,
    Medium,
    MediumCloseUp,
    CloseUp,
    ExtremeCloseUp,
    OverTheShoulder,
    Insert,
}

impl Framing {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ExtremeWide => "extreme_wide",
            Self::Wide => "wide",
            Self::Full => "full",
            Self::MediumWide => "medium_wide",
            Self::Medium => "medium",
            Self::MediumCloseUp => "medium_close_up",
            Self::CloseUp => "close_up",
            Self::ExtremeCloseUp => "extreme_close_up",
            Self::OverTheShoulder => "over_the_shoulder",
            Self::Insert => "insert",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMotion {
    Static,
    SlowDollyIn,
    DollyIn,
    SlowDollyOut,
    DollyOut,
    PanLeft,
    PanRight,
    TiltUp,
    TiltDown,
    Tracking,
    Handheld,
}

impl CameraMotion {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Static => "static",
            Self::SlowDollyIn => "slow_dolly_in",
            Self::DollyIn => "dolly_in",
            Self::SlowDollyOut => "slow_dolly_out",
            Self::DollyOut => "dolly_out",
            Self::PanLeft => "pan_left",
            Self::PanRight => "pan_right",
            Self::TiltUp => "tilt_up",
            Self::TiltDown => "tilt_down",
            Self::Tracking => "tracking",
            Self::Handheld => "handheld",
        }
    }
}

/// One planned shot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlannedShot {
    /// Unique in the plan.
    pub id: String,
    /// What the shot is for in the scene.
    pub purpose: String,
    pub direction: ShotDirection,
    pub framing: Framing,
    /// Focal length, such as `50mm`.
    pub lens: String,
    pub camera_motion: CameraMotion,
    pub action: String,
    /// What is said in the shot; empty when nothing is.
    pub dialogue: String,
    pub emotion: String,
    /// Seconds.
    pub duration_target: f64,
    /// Cast IDs of the scene, of those seen in the shot.
    pub characters: Vec<String>,
    /// What must stay as it was, in words.
    pub continuity: Vec<String>,
    /// The earlier shot this one continues from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub continuity_from: Option<String>,
}

/// The shots of one scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScenePlan {
    pub scene_id: String,
    pub shots: Vec<PlannedShot>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawShotPlan {
    scenes: Vec<ScenePlan>,
}

/// The shots of every scene, in screenplay order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawShotPlan")]
pub struct ShotPlan {
    pub scenes: Vec<ScenePlan>,
}

impl TryFrom<RawShotPlan> for ShotPlan {
    type Error = String;

    fn try_from(raw: RawShotPlan) -> Result<Self, String> {
        let plan = Self { scenes: raw.scenes };
        plan.validate()?;
        Ok(plan)
    }
}

fn filled(value: &str, what: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        Err(format!("{what} is empty"))
    } else {
        Ok(())
    }
}

impl ScenePlan {
    fn validate(&self, earlier: &mut BTreeSet<String>) -> Result<(), String> {
        if !is_portable_id(&self.scene_id) {
            return Err(format!("{:?} is not a scene id", self.scene_id));
        }
        if self.shots.is_empty() {
            return Err(format!("scene {:?} has no shots", self.scene_id));
        }
        for shot in &self.shots {
            let owner = format!("shot {:?}", shot.id);
            if !is_portable_id(&shot.id) || shot.id.len() > MAX_SHOT_ID {
                return Err(format!(
                    "{owner} id is not a portable identifier of at most {MAX_SHOT_ID} characters"
                ));
            }
            filled(&shot.purpose, &format!("{owner} purpose"))?;
            filled(&shot.action, &format!("{owner} action"))?;
            filled(&shot.emotion, &format!("{owner} emotion"))?;
            if !is_portable_id(&shot.lens) {
                return Err(format!("{owner} lens {:?} is not like 50mm", shot.lens));
            }
            if !DURATION.contains(&shot.duration_target) {
                return Err(format!(
                    "{owner} runs {} seconds, outside {}–{}",
                    shot.duration_target,
                    DURATION.start(),
                    DURATION.end()
                ));
            }
            let mut seen = BTreeSet::new();
            for character in &shot.characters {
                if !seen.insert(character.as_str()) {
                    return Err(format!("{owner} names {character:?} twice"));
                }
            }
            for note in &shot.continuity {
                filled(note, &format!("a continuity note of {owner}"))?;
            }
            if let Some(from) = &shot.continuity_from
                && !earlier.contains(from)
            {
                return Err(format!(
                    "{owner} continues from {from:?}, which is not an earlier shot"
                ));
            }
            if !earlier.insert(shot.id.clone()) {
                return Err(format!("{owner} appears twice"));
            }
        }
        Ok(())
    }

    /// Check this scene's shots against `scene` of the screenplay: only its
    /// cast is seen.
    fn check_against(&self, scene: &crate::screenplay::Scene) -> Result<(), String> {
        for shot in &self.shots {
            for character in &shot.characters {
                if !scene.characters.contains(character) {
                    return Err(format!(
                        "shot {:?} shows {character:?}, who is not in scene {:?}",
                        shot.id, scene.id
                    ));
                }
            }
        }
        Ok(())
    }
}

impl ShotPlan {
    fn validate(&self) -> Result<(), String> {
        if self.scenes.is_empty() {
            return Err("there are no scenes".to_owned());
        }
        let mut scenes = BTreeSet::new();
        let mut earlier = BTreeSet::new();
        for scene in &self.scenes {
            if !scenes.insert(scene.scene_id.as_str()) {
                return Err(format!("scene {:?} is planned twice", scene.scene_id));
            }
            scene.validate(&mut earlier)?;
        }
        Ok(())
    }

    /// Check that the plan has every scene of `screenplay`, in its order,
    /// shows only each scene's cast, and shows no one the bible has no entry
    /// for.
    pub fn check_against(&self, screenplay: &Screenplay, bible: &Bible) -> Result<(), String> {
        let planned: Vec<&str> = self.scenes.iter().map(|s| s.scene_id.as_str()).collect();
        let written: Vec<&str> = screenplay.scenes.iter().map(|s| s.id.as_str()).collect();
        if planned != written {
            return Err(format!(
                "the plan covers scenes {planned:?}; the screenplay has {written:?}, in that order"
            ));
        }
        for (plan, scene) in self.scenes.iter().zip(&screenplay.scenes) {
            plan.check_against(scene)?;
            for shot in &plan.shots {
                for character in &shot.characters {
                    if bible.character(character).is_none() {
                        return Err(format!(
                            "shot {:?} shows {character:?}, who has no bible entry",
                            shot.id
                        ));
                    }
                }
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn scene(&self, scene_id: &str) -> Option<&ScenePlan> {
        self.scenes.iter().find(|scene| scene.scene_id == scene_id)
    }

    /// This plan with the shots of `scene` replaced.
    pub fn with_scene(&self, scene: ScenePlan) -> Result<Self, String> {
        let mut plan = self.clone();
        let slot = plan
            .scenes
            .iter_mut()
            .find(|planned| planned.scene_id == scene.scene_id)
            .ok_or_else(|| format!("the plan has no scene {:?}", scene.scene_id))?;
        *slot = scene;
        plan.validate()?;
        Ok(plan)
    }

    /// Every shot, in plan order, with the scene it belongs to.
    pub fn shots(&self) -> impl Iterator<Item = (&str, &PlannedShot)> {
        self.scenes.iter().flat_map(|scene| {
            scene
                .shots
                .iter()
                .map(move |shot| (scene.scene_id.as_str(), shot))
        })
    }
}

/// The locked revision of `kind` and what it says, or `None` when none is
/// locked.
fn locked<T: serde::de::DeserializeOwned>(
    project: &CanonicalProject,
    kind: DocumentKind,
) -> Result<Option<(RevisionRecord, T)>, RevisionError> {
    let Some(record) = revisions::revisions(project, kind)?
        .into_iter()
        .find(|record| record.status == RevisionStatus::Locked)
    else {
        return Ok(None);
    };
    let document = revisions::read(project, &record)?;
    Ok(Some((record, document)))
}

/// What a plan is made from: the locked screenplay and the locked bible,
/// each with its revision; `None` until both are locked.
#[allow(clippy::type_complexity)]
pub fn material(
    project: &CanonicalProject,
) -> Result<Option<((RevisionRecord, Screenplay), (RevisionRecord, Bible))>, RevisionError> {
    let Some(screenplay) = locked::<Screenplay>(project, DocumentKind::Screenplay)? else {
        return Ok(None);
    };
    let Some(bible) = locked::<Bible>(project, DocumentKind::Bible)? else {
        return Ok(None);
    };
    Ok(Some((screenplay, bible)))
}

fn check_references(project: &CanonicalProject, plan: &ShotPlan) -> Result<(), RevisionError> {
    let ((_, screenplay), (_, bible)) = material(project)?.ok_or_else(|| {
        RevisionError::InvalidDocument("lock the screenplay and the bible first".to_owned())
    })?;
    plan.check_against(&screenplay, &bible)
        .map_err(RevisionError::InvalidDocument)
}

/// What a plan made from `screenplay` and `bible` depends on: each scene;
/// each cast member's entry and each scene location's entry; and the two
/// revisions themselves, which are cosmetic: they name where the entries
/// and scenes were read, while those decide freshness.
pub fn dependencies(
    screenplay: &(RevisionRecord, Screenplay),
    bible: &(RevisionRecord, Bible),
) -> Result<Vec<ArtifactDependency>, RevisionError> {
    let (screenplay_record, screenplay) = screenplay;
    let (bible_record, bible) = bible;
    let mut dependencies = vec![
        revisions::dependency_on(screenplay_record, DependencyImpact::Cosmetic)?,
        revisions::dependency_on(bible_record, DependencyImpact::Cosmetic)?,
    ];
    for scene in &screenplay.scenes {
        dependencies.push(scene.dependency()?);
    }
    let characters: BTreeSet<&str> = screenplay
        .scenes
        .iter()
        .flat_map(|scene| scene.characters.iter().map(String::as_str))
        .collect();
    for character in characters {
        dependencies.extend(bible.character_dependency(character));
    }
    let locations: BTreeSet<&str> = screenplay
        .scenes
        .iter()
        .map(|scene| scene.location.as_str())
        .collect();
    for location in locations {
        dependencies.extend(bible.location_dependency(location));
    }
    Ok(dependencies)
}

/// Why a plan no longer follows the locked screenplay and bible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StaleCause {
    /// The screenplay or the bible is not locked.
    NotLocked,
    SceneChanged(String),
    SceneRemoved(String),
    /// The locked screenplay has a scene the plan never read.
    SceneAdded(String),
    /// A bible entry the plan read changed or went, by artifact ID.
    Entry(String),
}

/// What makes the plan revision `record` stale against the locked screenplay
/// and bible; empty when it is fresh.
pub fn staleness(
    project: &CanonicalProject,
    record: &RevisionRecord,
) -> Result<Vec<StaleCause>, RevisionError> {
    let Some((screenplay, bible)) = material(project)? else {
        return Ok(vec![StaleCause::NotLocked]);
    };
    let current = dependencies(&screenplay, &bible)?;
    let screenplay = &screenplay.1;
    let mut causes = Vec::new();
    let mut read = BTreeSet::new();
    for dependency in record
        .dependencies
        .iter()
        .filter(|dependency| dependency.impact == DependencyImpact::Semantic)
    {
        if let Some(scene) = dependency.artifact_id.strip_prefix(SCENE_ARTIFACT_PREFIX) {
            read.insert(scene);
            match screenplay.freshness(&dependency.artifact_id, &dependency.content_hash) {
                Some(SceneFreshness::Changed) => {
                    causes.push(StaleCause::SceneChanged(scene.to_owned()));
                }
                Some(SceneFreshness::Removed) => {
                    causes.push(StaleCause::SceneRemoved(scene.to_owned()));
                }
                Some(SceneFreshness::Fresh) | None => {}
            }
        } else if !current.iter().any(|now| {
            now.artifact_id.as_str() == dependency.artifact_id
                && now.content_hash.as_str() == dependency.content_hash
        }) {
            causes.push(StaleCause::Entry(dependency.artifact_id.clone()));
        }
    }
    for scene in &screenplay.scenes {
        if !read.contains(scene.id.as_str()) {
            causes.push(StaleCause::SceneAdded(scene.id.clone()));
        }
    }
    Ok(causes)
}

/// The director's edit of one scene's shots.
pub fn edit_scene(
    project: &CanonicalProject,
    based_on: u32,
    scene: ScenePlan,
) -> Result<RevisionRecord, RevisionError> {
    let latest = revisions::latest(project, DocumentKind::ShotPlan)?
        .ok_or(RevisionError::NoSuchRevision(based_on))?;
    if latest.revision != based_on {
        return Err(RevisionError::NotLatest {
            based_on,
            latest: latest.revision,
        });
    }
    let base: ShotPlan = revisions::read(project, &latest)?;
    let edited = base
        .with_scene(scene)
        .map_err(RevisionError::InvalidDocument)?;
    check_references(project, &edited)?;
    revisions::save_edit(
        project,
        DocumentKind::ShotPlan,
        based_on,
        &revisions::encode(&edited)?,
    )
}

/// Lock `revision` as the plan production follows. It must cover the locked
/// screenplay and show only what the locked bible describes; a stale plan
/// that does can be locked, and [`staleness`] keeps saying why it may not
/// hold.
pub fn lock(project: &CanonicalProject, revision: u32) -> Result<(), RevisionError> {
    let record = revisions::revisions(project, DocumentKind::ShotPlan)?
        .into_iter()
        .find(|record| record.revision == revision)
        .ok_or(RevisionError::NoSuchRevision(revision))?;
    let plan: ShotPlan = revisions::read(project, &record)?;
    check_references(project, &plan)?;
    revisions::lock(project, DocumentKind::ShotPlan, revision)
}

/// The locked plan and its revision, or `None` when none is locked.
pub fn locked_plan(
    project: &CanonicalProject,
) -> Result<Option<(RevisionRecord, ShotPlan)>, RevisionError> {
    locked(project, DocumentKind::ShotPlan)
}

/// A small, valid plan of [`crate::screenplay::SAMPLE_SCREENPLAY_JSON`] and
/// [`crate::bible::SAMPLE_BIBLE_JSON`], for tests across the workspace.
#[doc(hidden)]
pub const SAMPLE_SHOT_PLAN_JSON: &str = r#"{
  "scenes": [
    {
      "scene_id": "office_night",
      "shots": [
        {
          "id": "office_night_01",
          "purpose": "Establish the two of them across the desk.",
          "direction": "spatial_clarity",
          "framing": "wide",
          "lens": "28mm",
          "camera_motion": "static",
          "action": "The interviewer closes her notebook; the filmmaker waits.",
          "dialogue": "",
          "emotion": "A held breath.",
          "duration_target": 4,
          "characters": ["filmmaker", "interviewer"],
          "continuity": ["Rain on the window behind them."]
        },
        {
          "id": "office_night_02",
          "purpose": "Let the last question land on him.",
          "direction": "reaction",
          "framing": "medium_close_up",
          "lens": "50mm",
          "camera_motion": "slow_dolly_in",
          "action": "He hears the question and does not answer.",
          "dialogue": "Was it worth it?",
          "emotion": "A quiet, withheld reaction.",
          "duration_target": 5,
          "characters": ["filmmaker"],
          "continuity": ["Same coat, same light as the wide."],
          "continuity_from": "office_night_01"
        }
      ]
    }
  ]
}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bible::SAMPLE_BIBLE_JSON, screenplay::SAMPLE_SCREENPLAY_JSON};

    fn sample() -> serde_json::Value {
        serde_json::from_str(SAMPLE_SHOT_PLAN_JSON).unwrap()
    }

    fn parse(value: &serde_json::Value) -> Result<ShotPlan, String> {
        serde_json::from_value(value.clone()).map_err(|error| error.to_string())
    }

    fn screenplay() -> Screenplay {
        serde_json::from_str(SAMPLE_SCREENPLAY_JSON).unwrap()
    }

    fn bible() -> Bible {
        serde_json::from_str(SAMPLE_BIBLE_JSON).unwrap()
    }

    #[test]
    fn the_sample_plan_covers_the_sample_screenplay() {
        let plan = parse(&sample()).unwrap();
        plan.check_against(&screenplay(), &bible()).unwrap();
        let shots: Vec<_> = plan
            .shots()
            .map(|(scene, shot)| (scene, shot.id.as_str()))
            .collect();
        assert_eq!(
            shots,
            [
                ("office_night", "office_night_01"),
                ("office_night", "office_night_02")
            ]
        );
        assert_eq!(plan.scenes[0].shots[1].framing.as_str(), "medium_close_up");
        assert_eq!(
            plan.scenes[0].shots[1].camera_motion.as_str(),
            "slow_dolly_in"
        );
    }

    #[test]
    fn a_plan_that_breaks_its_own_rules_is_refused() {
        type Change = fn(&mut serde_json::Value);
        let cases: [(&str, Change); 9] = [
            ("no shots", |v| {
                v["scenes"][0]["shots"] = serde_json::json!([])
            }),
            ("a shot twice", |v| {
                v["scenes"][0]["shots"][1]["id"] = "office_night_01".into();
            }),
            ("continuing from a later shot", |v| {
                v["scenes"][0]["shots"][0]["continuity_from"] = "office_night_02".into();
            }),
            ("too long", |v| {
                v["scenes"][0]["shots"][0]["duration_target"] = 45.into()
            }),
            ("an empty action", |v| {
                v["scenes"][0]["shots"][0]["action"] = " ".into()
            }),
            ("a lens that is not one", |v| {
                v["scenes"][0]["shots"][0]["lens"] = "a long lens".into();
            }),
            ("an unknown framing", |v| {
                v["scenes"][0]["shots"][0]["framing"] = "vast".into()
            }),
            ("a character twice", |v| {
                v["scenes"][0]["shots"][1]["characters"] =
                    serde_json::json!(["filmmaker", "filmmaker"]);
            }),
            ("an unknown field", |v| {
                v["scenes"][0]["shots"][0]["mood"] = "dark".into()
            }),
        ];
        for (case, change) in cases {
            let mut plan = sample();
            change(&mut plan);
            assert!(parse(&plan).is_err(), "{case}");
        }
    }

    #[test]
    fn a_plan_must_follow_the_screenplay_and_the_bible() {
        let mut stranger = sample();
        stranger["scenes"][0]["shots"][1]["characters"] = serde_json::json!(["waiter"]);
        let error = parse(&stranger)
            .unwrap()
            .check_against(&screenplay(), &bible())
            .unwrap_err();
        assert!(error.contains("\"waiter\", who is not in scene"), "{error}");

        let mut extra = sample();
        let mut dawn = extra["scenes"][0].clone();
        dawn["scene_id"] = "office_dawn".into();
        dawn["shots"][0]["id"] = "office_dawn_01".into();
        dawn["shots"][1]["id"] = "office_dawn_02".into();
        dawn["shots"][1]["continuity_from"] = "office_dawn_01".into();
        extra["scenes"].as_array_mut().unwrap().push(dawn);
        let error = parse(&extra)
            .unwrap()
            .check_against(&screenplay(), &bible())
            .unwrap_err();
        assert!(
            error.contains("the screenplay has [\"office_night\"]"),
            "{error}"
        );
    }

    #[test]
    fn one_scene_is_replaced_and_held_to_the_same_rules() {
        let plan = parse(&sample()).unwrap();
        let mut scene = plan.scenes[0].clone();
        scene.shots.truncate(1);
        let replaced = plan.with_scene(scene.clone()).unwrap();
        assert_eq!(replaced.scenes[0].shots.len(), 1);
        scene.scene_id = "office_dawn".to_owned();
        assert!(plan.with_scene(scene).is_err());
    }
}
