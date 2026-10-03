//! The shot plan: the shots of every scene of the locked screenplay, as
//! structured state.
//!
//! `schemas/shot-plan.schema.json` is the contract, and these types enforce
//! it on the way in. A plan is made from the locked screenplay and the
//! locked bible: [`ShotPlan::check_against`] holds it to their scenes and
//! cast, and [`staleness`] says what has changed since. A shot's location is
//! its scene's.
//!
//! A shot's ID is its identity: the takes and videos kept under it are its
//! own. A shot keeps its ID while it stays in the plan; a removed shot keeps
//! its ID and its history, and no shot added later ever takes either
//! ([`with_fresh_ids`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactDependency, DependencyImpact,
    bible::Bible,
    fs::ProjectRelativePath,
    manifest::CanonicalProject,
    revisions::{self, DocumentKind, RevisionError, RevisionRecord, RevisionStatus},
    screenplay::{SCENE_ARTIFACT_PREFIX, SceneFreshness, Screenplay},
    shot::{BOOTSTRAP_SCENE, BOOTSTRAP_SHOTS, ShotDirection, ShotKey, locate},
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

/// Every shot ID the project has used: each shot written under `scenes/`,
/// whether in play or removed, and each shot any plan revision names.
pub fn used_shot_ids(project: &CanonicalProject) -> Result<BTreeSet<String>, RevisionError> {
    let mut used = BTreeSet::new();
    for record in revisions::revisions(project, DocumentKind::ShotPlan)? {
        let plan: ShotPlan = revisions::read(project, &record)?;
        used.extend(plan.shots().map(|(_, shot)| shot.id.clone()));
    }
    let scenes = project
        .root()
        .resolve(&ProjectRelativePath::new("scenes").map_err(RevisionError::Path)?);
    let io = |error| RevisionError::Fs(crate::fs::ProjectFsError::Io(error));
    let entries = match std::fs::read_dir(&scenes) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(io(error)),
    };
    for scene in entries.into_iter().flatten() {
        let shots = match std::fs::read_dir(scene.map_err(io)?.path().join("shots")) {
            Ok(shots) => shots,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(io(error)),
        };
        for shot in shots {
            let shot = shot.map_err(io)?;
            if shot.path().join("shot.json").is_file()
                && let Some(id) = shot.file_name().to_str()
            {
                used.insert(id.to_owned());
            }
        }
    }
    Ok(used)
}

/// `plan` with each shot it adds to `base` (every shot, without one) given
/// a new ID where the project has used its ID before, so a new shot never
/// takes the identity, takes or video of a shot removed earlier. The new ID
/// is the scene's ID and the first number no shot has used; each shot that
/// continues from a renamed one follows it.
pub fn with_fresh_ids(
    project: &CanonicalProject,
    base: Option<&ShotPlan>,
    plan: ShotPlan,
) -> Result<ShotPlan, RevisionError> {
    fresh_ids(&used_shot_ids(project)?, base, plan).map_err(RevisionError::InvalidDocument)
}

/// [`with_fresh_ids`] given the IDs [`used_shot_ids`] read, for a caller
/// that cannot read the project where it renames, such as while checking a
/// model's answer.
pub fn fresh_ids(
    used: &BTreeSet<String>,
    base: Option<&ShotPlan>,
    mut plan: ShotPlan,
) -> Result<ShotPlan, String> {
    let kept: BTreeSet<&str> = base
        .map(|base| base.shots().map(|(_, shot)| shot.id.as_str()).collect())
        .unwrap_or_default();
    let reused: Vec<(String, String)> = plan
        .shots()
        .filter(|(_, shot)| !kept.contains(shot.id.as_str()) && used.contains(&shot.id))
        .map(|(scene, shot)| (scene.to_owned(), shot.id.clone()))
        .collect();
    if reused.is_empty() {
        return Ok(plan);
    }
    let mut taken = used.clone();
    taken.extend(plan.shots().map(|(_, shot)| shot.id.clone()));
    let mut renamed = BTreeMap::new();
    for (scene, id) in reused {
        let fresh = fresh_id(&scene, &taken)
            .ok_or_else(|| format!("scene {scene:?} has no shot ID left"))?;
        taken.insert(fresh.clone());
        renamed.insert(id, fresh);
    }
    for scene in &mut plan.scenes {
        for shot in &mut scene.shots {
            if let Some(fresh) = renamed.get(&shot.id) {
                shot.id.clone_from(fresh);
            }
            if let Some(fresh) = shot
                .continuity_from
                .as_ref()
                .and_then(|from| renamed.get(from))
            {
                shot.continuity_from = Some(fresh.clone());
            }
        }
    }
    plan.validate()?;
    Ok(plan)
}

/// `<scene>_NN` with the first number not in `taken`.
fn fresh_id(scene: &str, taken: &BTreeSet<String>) -> Option<String> {
    (1..=9_999).find_map(|number: u32| {
        let suffix = format!("_{number:02}");
        let room = MAX_SHOT_ID.checked_sub(suffix.len())?;
        let prefix: String = scene.chars().take(room).collect();
        let id = format!("{prefix}{suffix}");
        (!taken.contains(&id)).then_some(id)
    })
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
    let edited = with_fresh_ids(project, Some(&base), edited)?;
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
    // Shot IDs must not collide with shots another scene already holds.
    for (scene, shot) in plan.shots() {
        if let Some(elsewhere) = locate(project, &shot.id).ok().flatten()
            && elsewhere.scene() != scene
        {
            return Err(RevisionError::InvalidDocument(format!(
                "shot {:?} already exists in scene {:?}",
                shot.id,
                elsewhere.scene()
            )));
        }
    }
    revisions::lock(project, DocumentKind::ShotPlan, revision)?;
    materialize(project).map(|_| ())
}

/// Write the shots of the locked plan into the project: each scene's
/// `scene.json` from the locked screenplay, and each shot's `shot.json`
/// with the plan's intent. A shot already written keeps its generation and
/// video revisions; its intent is replaced, so candidates made for another
/// intent show as stale and are kept. Idempotent; writes nothing when no
/// plan is locked. Returns the shots in plan order.
pub fn materialize(project: &CanonicalProject) -> Result<Vec<ShotKey>, RevisionError> {
    let Some((_, plan)) = locked_plan(project)? else {
        return Ok(Vec::new());
    };
    let (_, screenplay) = crate::bible::locked_screenplay(project)?.ok_or_else(|| {
        RevisionError::InvalidDocument("the shot plan is locked but no screenplay is".to_owned())
    })?;
    let mut keys = Vec::new();
    for scene_plan in &plan.scenes {
        let scene = screenplay.scene(&scene_plan.scene_id).ok_or_else(|| {
            RevisionError::InvalidDocument(format!(
                "the locked screenplay has no scene {:?}",
                scene_plan.scene_id
            ))
        })?;
        write_merged(
            project,
            &format!("scenes/{}/scene.json", scene.id),
            serde_json::json!({
                "scene_id": scene.id,
                "schema_version": 1,
                "location": scene.location,
                "time": scene.time,
                "characters": scene.characters,
                "dramatic_goal": scene.dramatic_goal,
                "shots": scene_plan.shots.iter().map(|shot| shot.id.as_str()).collect::<Vec<_>>(),
            }),
        )?;
        for shot in &scene_plan.shots {
            let key = ShotKey::new(&scene.id, &shot.id).map_err(|_| {
                RevisionError::InvalidDocument(format!("{:?} cannot name a shot", shot.id))
            })?;
            if let Some(elsewhere) = locate(project, &shot.id).map_err(|_| {
                RevisionError::InvalidDocument(format!("shot {:?} is damaged", shot.id))
            })? && elsewhere != key
            {
                return Err(RevisionError::InvalidDocument(format!(
                    "shot {:?} already exists in scene {:?}",
                    shot.id,
                    elsewhere.scene()
                )));
            }
            let mut intent = serde_json::json!({
                "shot_id": shot.id,
                "schema_version": 1,
                "purpose": shot.purpose,
                "direction": shot.direction,
                "duration_target": shot.duration_target,
                "characters": shot.characters,
                "framing": shot.framing.as_str(),
                "lens": shot.lens,
                "camera_motion": shot.camera_motion.as_str(),
                "action": shot.action,
                "dialogue": shot.dialogue,
                "emotion": shot.emotion,
                "continuity": shot.continuity,
                "continuity_from": shot.continuity_from,
            });
            if shot.continuity_from.is_none()
                && let Some(object) = intent.as_object_mut()
            {
                object.remove("continuity_from");
            }
            write_merged(
                project,
                &format!("scenes/{}/shots/{}/shot.json", scene.id, shot.id),
                intent,
            )?;
            keys.push(key);
        }
    }
    Ok(keys)
}

/// Write `fields` over the JSON object at `path`, keeping its other fields
/// (such as a shot's generation revision). Writes nothing when nothing
/// changes.
fn write_merged(
    project: &CanonicalProject,
    path: &str,
    fields: serde_json::Value,
) -> Result<(), RevisionError> {
    let path = ProjectRelativePath::new(path).map_err(RevisionError::Path)?;
    let mut current = match project.root().read(&path) {
        Ok(bytes) => {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(RevisionError::Json)?
        }
        Err(crate::fs::ProjectFsError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            serde_json::json!({})
        }
        Err(error) => return Err(RevisionError::Fs(error)),
    };
    let before = current.clone();
    let (Some(object), serde_json::Value::Object(fields)) = (current.as_object_mut(), fields)
    else {
        return Err(RevisionError::InvalidDocument(format!(
            "{} is not a JSON object",
            path.as_path().display()
        )));
    };
    for (name, value) in fields {
        object.insert(name, value);
    }
    if current == before {
        return Ok(());
    }
    project
        .validate_for_write()
        .map_err(RevisionError::Manifest)?;
    let mut bytes = serde_json::to_vec_pretty(&current).map_err(RevisionError::Json)?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(&path, &bytes)
        .map_err(RevisionError::Fs)
}

/// The shots production works on, in order: those of the locked plan, or,
/// before any plan is locked, those the bootstrap scene lists, or its
/// [`BOOTSTRAP_SHOTS`] when it has no `scene.json`.
pub fn shots_in_play(project: &CanonicalProject) -> Result<Vec<ShotKey>, RevisionError> {
    if let Some((_, plan)) = locked_plan(project)? {
        return plan
            .shots()
            .map(|(scene, shot)| {
                ShotKey::new(scene, &shot.id).map_err(|_| {
                    RevisionError::InvalidDocument(format!("{:?} cannot name a shot", shot.id))
                })
            })
            .collect();
    }
    let path = ProjectRelativePath::new(format!("scenes/{BOOTSTRAP_SCENE}/scene.json"))
        .map_err(RevisionError::Path)?;
    let bytes = match project.root().read(&path) {
        Ok(bytes) => bytes,
        Err(crate::fs::ProjectFsError::Io(error))
            if error.kind() == std::io::ErrorKind::NotFound =>
        {
            return Ok((1..=BOOTSTRAP_SHOTS).map(ShotKey::bootstrap).collect());
        }
        Err(error) => return Err(RevisionError::Fs(error)),
    };
    #[derive(Deserialize)]
    struct Scene {
        #[serde(default)]
        shots: Vec<String>,
    }
    let scene: Scene = serde_json::from_slice(&bytes).map_err(RevisionError::Json)?;
    scene
        .shots
        .iter()
        .map(|shot| {
            ShotKey::new(BOOTSTRAP_SCENE, shot)
                .map_err(|_| RevisionError::InvalidDocument(format!("{shot:?} cannot name a shot")))
        })
        .collect()
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

    #[test]
    fn an_added_shot_with_a_used_id_is_renamed_and_followed() {
        let plan = parse(&sample()).unwrap();
        let used: BTreeSet<String> = ["office_night_01", "office_night_02", "office_night_03"]
            .map(str::to_owned)
            .into();
        // Only the first shot is kept; the second was used before.
        let mut base = plan.clone();
        base.scenes[0].shots.truncate(1);
        let fresh = fresh_ids(&used, Some(&base), plan.clone()).unwrap();
        let shots: Vec<_> = fresh
            .shots()
            .map(|(_, shot)| (shot.id.as_str(), shot.continuity_from.as_deref()))
            .collect();
        assert_eq!(
            shots,
            [
                ("office_night_01", None),
                ("office_night_04", Some("office_night_01"))
            ]
        );
        // Without a base every shot is added; the one continued from is
        // followed under its new ID.
        let fresh = fresh_ids(&used, None, plan.clone()).unwrap();
        assert_eq!(
            fresh.scenes[0].shots[1].continuity_from.as_deref(),
            Some(fresh.scenes[0].shots[0].id.as_str())
        );
        // A plan that reuses nothing is returned as it is.
        assert_eq!(
            fresh_ids(&BTreeSet::new(), None, plan.clone()).unwrap(),
            plan
        );
        // A long scene ID is cut so the shot ID still fits.
        let long = "s".repeat(80);
        let id = fresh_id(&long, &BTreeSet::new()).unwrap();
        assert_eq!((id.len(), id.ends_with("_01")), (MAX_SHOT_ID, true));
    }
}
