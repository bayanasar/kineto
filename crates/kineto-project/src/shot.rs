use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    ArtifactDependency, ArtifactId, ArtifactIdError, ArtifactRecord, ArtifactStatus, ContentHash,
    DependencyImpact, HashValueError, InputHash, InputHashMaterial, LifecycleError, SelectionError,
    SelectionManifest,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError, ProjectStoreError},
    media::{MediaKind, MediaOwner, MediaRecord, MediaState},
};

const SHOT_SCHEMA_VERSION: u32 = 1;
const ARTIFACT_SCHEMA_VERSION: u32 = 1;
/// Candidates one generation creates.
pub const GENERATED_CANDIDATE_COUNT: usize = 3;
const LEGACY_HASH_BACKUP_FILE: &str = "artifacts.pre-sha256-migration.json";

/// The intent a candidate records it was made for, by `shot.json` field.
/// A field whose value changes since makes the candidate stale.
const INTENT_FIELDS: [&str; 12] = [
    "direction",
    "purpose",
    "action",
    "framing",
    "lens",
    "camera_motion",
    "emotion",
    "dialogue",
    "characters",
    "duration_target",
    "continuity",
    "continuity_from",
];

/// The artifact field holding a candidate's intent snapshot.
const INTENT_RECORD: &str = "intent";

/// Provider key recorded on candidates from the deterministic local demo
/// generator. It stands in for a real image provider until P0.5.
pub const DEMO_PROVIDER: &str = "local-demo";
/// Model recorded on candidates from the deterministic local demo generator.
pub const DEMO_MODEL: &str = "deterministic-demo-v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
#[serde(rename_all = "snake_case")]
pub enum ShotDirection {
    #[default]
    Reaction = 1,
    SpatialClarity = 2,
    Intimacy = 3,
    Tension = 4,
}

impl ShotDirection {
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Reaction),
            2 => Some(Self::SpatialClarity),
            3 => Some(Self::Intimacy),
            4 => Some(Self::Tension),
            _ => None,
        }
    }

    fn from_legacy_key(value: &str) -> Option<Self> {
        match value {
            "reaction" => Some(Self::Reaction),
            "spatial_clarity" => Some(Self::SpatialClarity),
            "intimacy" => Some(Self::Intimacy),
            "tension" => Some(Self::Tension),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShotProductionManifest {
    pub shot_id: String,
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub direction: ShotDirection,
    #[serde(default)]
    pub generation_revision: u16,
    /// Revision of the shot's latest video generation; 0 before the first.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub video_revision: u16,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[allow(clippy::trivially_copy_pass_by_ref)] // serde passes a reference
const fn is_zero(value: &u16) -> bool {
    *value == 0
}

const fn default_schema_version() -> u32 {
    SHOT_SCHEMA_VERSION
}

/// Where the shot's current video stands in canonical state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoState {
    /// No current video.
    #[default]
    None,
    /// A video candidate from the locked keyframe, awaiting approval.
    Ready,
    /// The video is locked as production truth.
    Locked,
    /// The keyframe the video was made from is no longer the locked one.
    Stale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShotApprovalSnapshot {
    pub generated: bool,
    pub stale: bool,
    pub selected_index: Option<u8>,
    pub locked: bool,
    pub candidate_count: u8,
    pub direction: ShotDirection,
    pub generation_revision: u16,
    pub superseded_count: u16,
    pub migration_pending: bool,
    pub invalid_hash_count: u16,
    pub video: VideoState,
    pub video_revision: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShotHashIssue {
    pub artifact_id: String,
    pub field: &'static str,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShotHashMigrationRecord {
    pub artifact_id: String,
    pub field: &'static str,
    pub from: String,
    pub to: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ShotMigrationReport {
    pub backup_path: Option<String>,
    pub rewritten: Vec<ShotHashMigrationRecord>,
}

/// One generation of candidates for a shot, decided before anything is
/// written: the revision it publishes, the candidate IDs it creates and the
/// input hash they are generated for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationPlan {
    shot_id: String,
    revision: u16,
    input_hash: InputHash,
    candidate_ids: Vec<ArtifactId>,
}

impl GenerationPlan {
    /// Rebuild the plan a candidate belongs to from its ID and the input hash
    /// recorded with its job, for example when an interrupted generation is
    /// completed after a restart.
    pub fn for_candidate(
        candidate: &ArtifactId,
        input_hash: InputHash,
    ) -> Result<Self, ShotWorkflowError> {
        let (shot_id, revision) = parse_candidate_id(candidate.as_str()).ok_or(
            ShotWorkflowError::InvalidTarget(std::path::PathBuf::from(candidate.as_str())),
        )?;
        Ok(Self {
            candidate_ids: candidate_ids(shot_id, revision)?,
            shot_id: shot_id.to_owned(),
            revision,
            input_hash,
        })
    }

    /// The shot the generation belongs to.
    #[must_use]
    pub fn shot_id(&self) -> &str {
        &self.shot_id
    }

    #[must_use]
    pub const fn revision(&self) -> u16 {
        self.revision
    }

    #[must_use]
    pub fn input_hash(&self) -> &InputHash {
        &self.input_hash
    }

    #[must_use]
    pub fn candidate_ids(&self) -> &[ArtifactId] {
        &self.candidate_ids
    }
}

/// What a provider produced for one planned candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateOutput {
    pub artifact_id: ArtifactId,
    pub content_hash: ContentHash,
    pub provider: String,
    pub model: String,
    /// The file holding the candidate's bytes. `content_hash` is the hash of
    /// that file. The demo generator produces no media.
    pub media_path: Option<ProjectRelativePath>,
    /// How the provider input was compiled, for providers that took a prompt.
    pub provenance: Option<PromptProvenance>,
    /// What the prompt was compiled from, such as the locked style and the
    /// locked casts of the shot's characters.
    pub dependencies: Vec<ArtifactDependency>,
}

/// The provider and model recorded on a candidate's artifact. Each is empty
/// when the record names none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CandidateOrigin {
    pub provider: String,
    pub model: String,
}

/// One keyframe candidate as the shot's history keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CandidateRecord {
    pub artifact_id: String,
    /// The generation it came from.
    pub generation: u16,
    pub status: ArtifactStatus,
    pub media_path: Option<ProjectRelativePath>,
    pub origin: CandidateOrigin,
    /// The constraints its prompt applied, such as `director.note`.
    pub applied: Vec<String>,
}

/// One video as the shot's history keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRecord {
    pub artifact_id: String,
    /// The video generation it came from.
    pub generation: u16,
    /// Its place among its generation's candidates; `None` for a video made
    /// before a generation could have several.
    pub candidate: Option<u8>,
    pub status: ArtifactStatus,
    pub media_path: Option<ProjectRelativePath>,
    pub origin: CandidateOrigin,
    /// The director's notes it was made with, oldest first: a branch keeps
    /// the notes of the take it branched from.
    pub notes: Vec<String>,
    /// The take it was branched from.
    pub branched_from: Option<String>,
    /// Why it no longer follows the shot; empty when it does, and for a
    /// superseded video.
    pub stale: Vec<String>,
}

/// How much a compilation lost, derived from its dropped constraints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Degradation {
    None,
    Cosmetic,
    IdentityAffecting,
}

/// A constraint the compiler could not carry into the provider input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DroppedConstraintRecord {
    pub constraint: String,
    pub reason: String,
    pub degradation: Degradation,
}

/// The compiled provider input a candidate was generated from, kept verbatim
/// so it can be explained and reproduced without a newer compiler.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptProvenance {
    pub compiler_version: String,
    pub compiled_prompt: String,
    pub prompt_hash: String,
    pub applied_constraints: Vec<String>,
    pub dropped_constraints: Vec<DroppedConstraintRecord>,
}

impl PromptProvenance {
    #[must_use]
    pub fn degradation(&self) -> Degradation {
        self.dropped_constraints
            .iter()
            .map(|dropped| dropped.degradation)
            .max()
            .unwrap_or(Degradation::None)
    }

    fn validate(&self) -> Result<(), ShotWorkflowError> {
        ContentHash::new(self.prompt_hash.clone())?;
        if self.compiler_version.trim().is_empty()
            || self.compiled_prompt.trim().is_empty()
            || self.dropped_constraints.iter().any(|dropped| {
                dropped.reason.trim().is_empty() || dropped.degradation == Degradation::None
            })
        {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        Ok(())
    }
}

/// Where a keyframe candidate's bytes live: beside the shot's canonical
/// files, named by the candidate's stable artifact ID.
pub fn keyframe_media_path(
    key: &ShotKey,
    artifact_id: &ArtifactId,
    extension: &str,
) -> Result<ProjectRelativePath, ShotWorkflowError> {
    if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return Err(ShotWorkflowError::InvalidCanonicalState);
    }
    shot_path(
        key,
        &format!("keyframes/{}.{extension}", artifact_id.as_str()),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachOutcome {
    /// The generation was published by this call.
    Attached,
    /// Every candidate was already in canonical state; nothing was written.
    AlreadyAttached,
}

/// What a shot is meant to show, as written in its `shot.json`.
///
/// Only `action` is required for a shot to have an intent; every other field
/// is optional, and fields this build does not know stay in the file.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShotIntent {
    pub action: String,
    pub framing: Option<String>,
    pub lens: Option<String>,
    pub camera_motion: Option<String>,
    pub emotion: Option<String>,
    pub dialogue: Option<String>,
    pub characters: Vec<String>,
    pub duration_target: Option<f64>,
    /// What must stay as it was, in words.
    pub continuity: Vec<String>,
    /// The earlier shot this one continues from.
    pub continuity_from: Option<String>,
}

/// The scene a shot belongs to, as written in its `scene.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SceneIntent {
    /// The scene's ID, as its directory is named.
    pub id: String,
    pub location: Option<String>,
    pub time: Option<String>,
    pub dramatic_goal: Option<String>,
    pub characters: Vec<String>,
}

#[derive(Deserialize)]
struct StoredShotIntent {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    framing: Option<String>,
    #[serde(default)]
    lens: Option<String>,
    #[serde(default)]
    camera_motion: Option<String>,
    #[serde(default)]
    emotion: Option<String>,
    #[serde(default)]
    dialogue: Option<String>,
    #[serde(default)]
    characters: Vec<String>,
    #[serde(default)]
    duration_target: Option<f64>,
    #[serde(default)]
    continuity: Vec<String>,
    #[serde(default)]
    continuity_from: Option<String>,
}

#[derive(Deserialize)]
struct StoredSceneIntent {
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    time: Option<String>,
    #[serde(default)]
    dramatic_goal: Option<String>,
    #[serde(default)]
    characters: Vec<String>,
}

fn non_blank(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// The intent of scene `scene_id`, or `None` when it has no `scene.json`.
pub fn load_scene_intent(
    project: &CanonicalProject,
    scene_id: &str,
) -> Result<Option<SceneIntent>, ShotWorkflowError> {
    let path = scene_path(scene_id, "scene.json")?;
    let Some(stored) = read_json_optional::<StoredSceneIntent>(project, &path)? else {
        return Ok(None);
    };
    Ok(Some(SceneIntent {
        id: scene_id.to_owned(),
        location: non_blank(stored.location),
        time: non_blank(stored.time),
        dramatic_goal: non_blank(stored.dramatic_goal),
        characters: stored.characters,
    }))
}

/// Artifact type of a shot's video candidates.
pub const VIDEO_ARTIFACT_TYPE: &str = "shot_video";
/// Most video candidates one generation may be asked for.
pub const MAX_VIDEO_CANDIDATES: u8 = 4;
/// Artifact fields holding the director's notes a video was made with and
/// the take it was branched from.
const NOTES_RECORD: &str = "notes";
const BRANCH_RECORD: &str = "branched_from";

/// Where a video candidate's bytes live: beside the shot's canonical files,
/// named by its stable artifact ID.
pub fn video_media_path(
    key: &ShotKey,
    artifact_id: &ArtifactId,
) -> Result<ProjectRelativePath, ShotWorkflowError> {
    shot_path(key, &format!("videos/{}.mp4", artifact_id.as_str()))
}

/// What a video generation is asked for, beyond the keyframe it starts from.
/// Every field is part of the video's input hash.
#[derive(Debug, Clone, Copy)]
pub struct VideoGenerationSpec<'a> {
    pub provider: &'a str,
    pub model: &'a str,
    /// Deterministic encoding of provider parameters such as clip length.
    pub canonical_params: &'a [u8],
    pub compiler_version: &'a str,
    /// The director's notes, oldest first.
    pub notes: &'a [String],
    /// The take this generation branches from.
    pub branched_from: Option<&'a str>,
}

/// One video candidate of a generation for a shot, decided before anything
/// is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoPlan {
    shot_id: String,
    revision: u16,
    artifact_id: ArtifactId,
    input_hash: InputHash,
    /// The locked keyframe the video starts from. Known when the plan is
    /// made; a plan rebuilt for recovery gets it from the provider's record.
    keyframe: Option<(ArtifactDependency, Option<ProjectRelativePath>)>,
}

impl VideoPlan {
    /// Rebuild the plan a video job belongs to from its artifact ID and the
    /// input hash recorded with the job.
    pub fn for_artifact(
        artifact_id: &ArtifactId,
        input_hash: InputHash,
    ) -> Result<Self, ShotWorkflowError> {
        let (shot_id, revision, _) = parse_video_id(artifact_id.as_str()).ok_or(
            ShotWorkflowError::InvalidTarget(std::path::PathBuf::from(artifact_id.as_str())),
        )?;
        Ok(Self {
            shot_id: shot_id.to_owned(),
            revision,
            artifact_id: artifact_id.clone(),
            input_hash,
            keyframe: None,
        })
    }

    /// The shot the video belongs to.
    #[must_use]
    pub fn shot_id(&self) -> &str {
        &self.shot_id
    }

    #[must_use]
    pub const fn revision(&self) -> u16 {
        self.revision
    }

    #[must_use]
    pub fn artifact_id(&self) -> &ArtifactId {
        &self.artifact_id
    }

    #[must_use]
    pub fn input_hash(&self) -> &InputHash {
        &self.input_hash
    }

    /// The keyframe dependency and its media, when the plan was made from a
    /// locked keyframe.
    #[must_use]
    pub fn keyframe(&self) -> Option<&(ArtifactDependency, Option<ProjectRelativePath>)> {
        self.keyframe.as_ref()
    }
}

/// What a provider produced for one planned video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoOutput {
    pub artifact_id: ArtifactId,
    pub content_hash: ContentHash,
    pub provider: String,
    pub model: String,
    pub media_path: ProjectRelativePath,
    pub provenance: Option<PromptProvenance>,
    /// The keyframe the video was generated from.
    pub keyframe: ArtifactDependency,
    /// The shot's intent when the video was asked for, as
    /// [`ShotWorkflow::intent_record`] gave it; a field changed since makes
    /// the video stale.
    pub intent: Option<Value>,
    /// The director's notes it was made with, oldest first.
    pub notes: Vec<String>,
    /// The take it was branched from.
    pub branched_from: Option<String>,
}

/// Output of the deterministic local demo generator for one candidate.
pub fn demo_candidate_output(
    artifact_id: &ArtifactId,
) -> Result<CandidateOutput, ShotWorkflowError> {
    Ok(CandidateOutput {
        artifact_id: artifact_id.clone(),
        content_hash: demo_content_hash(artifact_id)?,
        provider: DEMO_PROVIDER.to_owned(),
        model: DEMO_MODEL.to_owned(),
        media_path: None,
        provenance: None,
        dependencies: Vec::new(),
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredArtifactStatus {
    Draft,
    Candidate,
    Selected,
    Locked,
    Superseded,
}

impl From<StoredArtifactStatus> for ArtifactStatus {
    fn from(value: StoredArtifactStatus) -> Self {
        match value {
            StoredArtifactStatus::Draft => Self::Draft,
            StoredArtifactStatus::Candidate => Self::Candidate,
            StoredArtifactStatus::Selected => Self::Selected,
            StoredArtifactStatus::Locked => Self::Locked,
            StoredArtifactStatus::Superseded => Self::Superseded,
        }
    }
}

impl From<ArtifactStatus> for StoredArtifactStatus {
    fn from(value: ArtifactStatus) -> Self {
        match value {
            ArtifactStatus::Draft => Self::Draft,
            ArtifactStatus::Candidate => Self::Candidate,
            ArtifactStatus::Selected => Self::Selected,
            ArtifactStatus::Locked => Self::Locked,
            ArtifactStatus::Superseded => Self::Superseded,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StoredDependencyImpact {
    Semantic,
    Cosmetic,
}

impl From<StoredDependencyImpact> for DependencyImpact {
    fn from(value: StoredDependencyImpact) -> Self {
        match value {
            StoredDependencyImpact::Semantic => Self::Semantic,
            StoredDependencyImpact::Cosmetic => Self::Cosmetic,
        }
    }
}

impl From<DependencyImpact> for StoredDependencyImpact {
    fn from(value: DependencyImpact) -> Self {
        match value {
            DependencyImpact::Semantic => Self::Semantic,
            DependencyImpact::Cosmetic => Self::Cosmetic,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredArtifactDependency {
    artifact_id: String,
    content_hash: String,
    impact: StoredDependencyImpact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    field: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct StoredArtifactRecord {
    artifact_id: String,
    schema_version: u32,
    #[serde(rename = "type")]
    artifact_type: String,
    status: StoredArtifactStatus,
    content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    input_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    media_state: Option<MediaState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compiler_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compiled_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    prompt_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    applied_constraints: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    dropped_constraints: Option<Vec<DroppedConstraintRecord>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    degradation: Option<Degradation>,
    dependencies: Vec<StoredArtifactDependency>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl StoredArtifactRecord {
    /// Where its media is, unless its bytes were let go.
    fn kept_media(&self) -> Option<&String> {
        self.media_path
            .as_ref()
            .filter(|_| self.media_state != Some(MediaState::Evicted))
    }

    fn validate_structure(&self) -> Result<(), ShotWorkflowError> {
        if self.schema_version == 0 || self.artifact_type.trim().is_empty() {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        ArtifactId::new(self.artifact_id.clone())?;
        for dependency in &self.dependencies {
            dependency.validate_structure()?;
        }
        Ok(())
    }

    fn domain(&self) -> Result<ArtifactRecord, ShotWorkflowError> {
        self.validate_structure()?;
        Ok(ArtifactRecord {
            artifact_id: ArtifactId::new(self.artifact_id.clone())?,
            status: self.status.into(),
            content_hash: ContentHash::new(self.content_hash.clone())?,
            input_hash: self
                .input_hash
                .as_ref()
                .map(|value| InputHash::new(value.clone()))
                .transpose()?,
            dependencies: self
                .dependencies
                .iter()
                .map(StoredArtifactDependency::domain)
                .collect::<Result<_, _>>()?,
        })
    }

    fn transition(&mut self, next: ArtifactStatus) -> Result<(), ShotWorkflowError> {
        let mut domain = self.domain()?;
        domain.transition(next)?;
        self.status = domain.status.into();
        Ok(())
    }
}

impl StoredArtifactDependency {
    fn validate_structure(&self) -> Result<(), ShotWorkflowError> {
        ArtifactId::new(self.artifact_id.clone())?;
        Ok(())
    }

    fn domain(&self) -> Result<ArtifactDependency, ShotWorkflowError> {
        self.validate_structure()?;
        Ok(ArtifactDependency {
            artifact_id: ArtifactId::new(self.artifact_id.clone())?,
            content_hash: ContentHash::new(self.content_hash.clone())?,
            impact: self.impact.into(),
            field: self.field.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
struct StoredSelectionManifest {
    selected_artifact_id: Option<String>,
    candidate_artifact_ids: Vec<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl StoredSelectionManifest {
    fn domain(&self) -> Result<SelectionManifest, ShotWorkflowError> {
        let candidate_artifact_ids = self
            .candidate_artifact_ids
            .iter()
            .map(|value| ArtifactId::new(value.clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let mut unique = BTreeSet::new();
        if candidate_artifact_ids
            .iter()
            .any(|artifact_id| !unique.insert(artifact_id.clone()))
        {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        let selected_artifact_id = self
            .selected_artifact_id
            .as_ref()
            .map(|value| ArtifactId::new(value.clone()))
            .transpose()?;
        if selected_artifact_id
            .as_ref()
            .is_some_and(|selected| !candidate_artifact_ids.contains(selected))
        {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        Ok(SelectionManifest {
            selected_artifact_id,
            candidate_artifact_ids,
        })
    }

    fn select(&mut self, artifact_id: &ArtifactId) -> Result<(), ShotWorkflowError> {
        let mut domain = self.domain()?;
        domain.select(artifact_id)?;
        self.selected_artifact_id = domain
            .selected_artifact_id
            .as_ref()
            .map(|selected| selected.as_str().to_owned());
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ShotWorkflow {
    key: ShotKey,
    shot: ShotProductionManifest,
    artifacts: Vec<StoredArtifactRecord>,
    selection: StoredSelectionManifest,
    hash_issues: Vec<ShotHashIssue>,
    hash_migration_records: Vec<ShotHashMigrationRecord>,
    legacy_artifacts_bytes: Option<Vec<u8>>,
}

impl ShotWorkflow {
    pub fn load(project: &CanonicalProject, key: &ShotKey) -> Result<Self, ShotWorkflowError> {
        let shot_id = key.shot().to_owned();
        let shot_file = shot_path(key, "shot.json")?;
        let artifacts_file = shot_path(key, "artifacts.json")?;
        let selection_file = shot_path(key, "selection.json")?;

        let shot =
            read_json_optional(project, &shot_file)?.unwrap_or_else(|| ShotProductionManifest {
                shot_id: shot_id.clone(),
                schema_version: SHOT_SCHEMA_VERSION,
                direction: ShotDirection::Reaction,
                generation_revision: 0,
                video_revision: 0,
                extra: BTreeMap::new(),
            });
        if shot.shot_id != shot_id || shot.schema_version == 0 {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }

        let artifacts_bytes = read_bytes_optional(project, &artifacts_file)?;
        let artifacts = artifacts_bytes
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()
            .map_err(ShotWorkflowError::Json)?
            .unwrap_or_default();
        let selection = read_json_optional(project, &selection_file)?.unwrap_or_default();
        let mut workflow = Self {
            key: key.clone(),
            shot,
            artifacts,
            selection,
            hash_issues: Vec::new(),
            hash_migration_records: Vec::new(),
            legacy_artifacts_bytes: artifacts_bytes,
        };
        workflow.normalize_legacy_demo_hashes()?;
        if workflow.hash_migration_records.is_empty() {
            workflow.legacy_artifacts_bytes = None;
        }
        if workflow.hash_issues.is_empty() && workflow.hash_migration_records.is_empty() {
            workflow.recover_partial_publication()?;
        }
        workflow.validate()?;
        Ok(workflow)
    }

    pub fn snapshot(&self) -> Result<ShotApprovalSnapshot, ShotWorkflowError> {
        self.validate()?;
        let selection = self.selection.domain()?;
        let candidate_count = u8::try_from(selection.candidate_artifact_ids.len())
            .map_err(|_| ShotWorkflowError::InvalidCanonicalState)?;
        let selected_index = selection
            .selected_artifact_id
            .as_ref()
            .and_then(|selected| {
                selection
                    .candidate_artifact_ids
                    .iter()
                    .position(|candidate| candidate == selected)
            })
            .map(u8::try_from)
            .transpose()
            .map_err(|_| ShotWorkflowError::InvalidCanonicalState)?;
        let locked = selection
            .selected_artifact_id
            .as_ref()
            .and_then(|selected| self.artifact(selected))
            .is_some_and(|artifact| artifact.status == StoredArtifactStatus::Locked);
        let superseded_count = u16::try_from(
            self.artifacts
                .iter()
                .filter(|artifact| {
                    artifact.artifact_type != VIDEO_ARTIFACT_TYPE
                        && artifact.status == StoredArtifactStatus::Superseded
                })
                .count(),
        )
        .map_err(|_| ShotWorkflowError::InvalidCanonicalState)?;
        let invalid_hash_count = u16::try_from(self.hash_issues.len())
            .map_err(|_| ShotWorkflowError::InvalidCanonicalState)?;

        Ok(ShotApprovalSnapshot {
            generated: candidate_count != 0,
            stale: self.is_stale()?,
            selected_index,
            locked,
            candidate_count,
            direction: self.shot.direction,
            generation_revision: self.shot.generation_revision,
            superseded_count,
            migration_pending: !self.hash_migration_records.is_empty(),
            invalid_hash_count,
            video: self.video_state()?,
            video_revision: self.shot.video_revision,
        })
    }

    #[must_use]
    pub fn hash_issues(&self) -> &[ShotHashIssue] {
        &self.hash_issues
    }

    /// The shot's written intent, or `None` when its `shot.json` has no
    /// `action`. A field of the wrong type is invalid canonical state.
    pub fn intent(&self) -> Result<Option<ShotIntent>, ShotWorkflowError> {
        let stored: StoredShotIntent = serde_json::from_value(Value::Object(
            self.shot
                .extra
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ))
        .map_err(ShotWorkflowError::Json)?;
        let Some(action) = non_blank(stored.action) else {
            return Ok(None);
        };
        Ok(Some(ShotIntent {
            action,
            framing: non_blank(stored.framing),
            lens: non_blank(stored.lens),
            camera_motion: non_blank(stored.camera_motion),
            emotion: non_blank(stored.emotion),
            dialogue: non_blank(stored.dialogue),
            characters: stored.characters,
            duration_target: stored.duration_target,
            continuity: stored
                .continuity
                .into_iter()
                .filter_map(|note| non_blank(Some(note)))
                .collect(),
            continuity_from: non_blank(stored.continuity_from),
        }))
    }

    /// Where the shot lives.
    #[must_use]
    pub const fn key(&self) -> &ShotKey {
        &self.key
    }

    #[must_use]
    pub const fn direction(&self) -> ShotDirection {
        self.shot.direction
    }

    /// Whether every candidate of `plan` is already in canonical state, so
    /// attaching it again would change nothing.
    #[must_use]
    pub fn is_attached(&self, plan: &GenerationPlan) -> bool {
        plan.shot_id == self.key.shot
            && self.shot.generation_revision >= plan.revision
            && plan
                .candidate_ids
                .iter()
                .all(|candidate| self.artifact(candidate).is_some())
    }

    /// The media file of each current candidate, in candidate order. `None`
    /// for a candidate without media, such as a demo candidate.
    pub fn candidate_media(&self) -> Result<Vec<Option<ProjectRelativePath>>, ShotWorkflowError> {
        self.selection
            .domain()?
            .candidate_artifact_ids
            .iter()
            .map(|candidate| {
                self.artifact(candidate)
                    .ok_or(ShotWorkflowError::InvalidCanonicalState)?
                    .kept_media()
                    .map(|path| ProjectRelativePath::new(path).map_err(ShotWorkflowError::Path))
                    .transpose()
            })
            .collect()
    }

    /// What changed in the shot's intent since each current candidate was
    /// made, in candidate order, as `intent.<field>`.
    pub fn candidate_intent_changes(&self) -> Result<Vec<Vec<String>>, ShotWorkflowError> {
        self.selection
            .domain()?
            .candidate_artifact_ids
            .iter()
            .map(|candidate| {
                self.changed_intent(
                    self.artifact(candidate)
                        .ok_or(ShotWorkflowError::InvalidCanonicalState)?,
                )
            })
            .collect()
    }

    /// Every keyframe candidate the shot has had, current or superseded,
    /// oldest generation first: what the director compares across takes.
    pub fn candidate_history(&self) -> Result<Vec<CandidateRecord>, ShotWorkflowError> {
        self.artifacts
            .iter()
            .filter(|artifact| artifact.artifact_type == "shot_candidate")
            .map(|artifact| {
                let (_, generation) = parse_candidate_id(&artifact.artifact_id)
                    .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
                Ok(CandidateRecord {
                    artifact_id: artifact.artifact_id.clone(),
                    generation,
                    status: artifact.status.into(),
                    media_path: artifact
                        .kept_media()
                        .map(|path| ProjectRelativePath::new(path).map_err(ShotWorkflowError::Path))
                        .transpose()?,
                    origin: CandidateOrigin {
                        provider: artifact.provider.clone().unwrap_or_default(),
                        model: artifact.model.clone().unwrap_or_default(),
                    },
                    applied: artifact.applied_constraints.clone().unwrap_or_default(),
                })
            })
            .collect()
    }

    /// What each current candidate was compiled from, in candidate order,
    /// as its artifact records it.
    pub fn candidate_dependencies(
        &self,
    ) -> Result<Vec<Vec<ArtifactDependency>>, ShotWorkflowError> {
        self.selection
            .domain()?
            .candidate_artifact_ids
            .iter()
            .map(|candidate| {
                self.artifact(candidate)
                    .ok_or(ShotWorkflowError::InvalidCanonicalState)?
                    .dependencies
                    .iter()
                    .map(|dependency| {
                        Ok(ArtifactDependency {
                            artifact_id: ArtifactId::new(dependency.artifact_id.clone())
                                .map_err(ShotWorkflowError::ArtifactId)?,
                            content_hash: ContentHash::new(dependency.content_hash.clone())
                                .map_err(ShotWorkflowError::Hash)?,
                            impact: dependency.impact.into(),
                            field: dependency.field.clone(),
                        })
                    })
                    .collect()
            })
            .collect()
    }

    /// Who made each current candidate, in candidate order, as its artifact
    /// records it. This is not the session's provider: a project generated
    /// with one provider may be reopened with another.
    pub fn candidate_origins(&self) -> Result<Vec<CandidateOrigin>, ShotWorkflowError> {
        self.selection
            .domain()?
            .candidate_artifact_ids
            .iter()
            .map(|candidate| {
                let artifact = self
                    .artifact(candidate)
                    .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
                Ok(CandidateOrigin {
                    provider: artifact.provider.clone().unwrap_or_default(),
                    model: artifact.model.clone().unwrap_or_default(),
                })
            })
            .collect()
    }

    pub fn migrate_legacy_hashes(
        &mut self,
        project: &CanonicalProject,
    ) -> Result<ShotMigrationReport, ShotWorkflowError> {
        if let Some(issue) = self.hash_issues.first() {
            return Err(invalid_stored_hash(issue));
        }
        if self.hash_migration_records.is_empty() {
            return Ok(ShotMigrationReport::default());
        }
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;

        let original = self
            .legacy_artifacts_bytes
            .as_ref()
            .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
        let backup_path = shot_path(&self.key, LEGACY_HASH_BACKUP_FILE)?;
        match read_bytes_optional(project, &backup_path)? {
            Some(existing) if existing != *original => {
                return Err(ShotWorkflowError::MigrationBackupConflict(
                    backup_path.as_path().to_string_lossy().into_owned(),
                ));
            }
            Some(_) => {}
            None => project
                .root()
                .write_atomic(&backup_path, original)
                .map_err(ShotWorkflowError::Fs)?,
        }

        let artifacts_path = shot_path(&self.key, "artifacts.json")?;
        write_json(project, &artifacts_path, &self.artifacts)?;
        let report = ShotMigrationReport {
            backup_path: Some(backup_path.as_path().to_string_lossy().into_owned()),
            rewritten: self.hash_migration_records.clone(),
        };
        self.hash_migration_records.clear();
        self.legacy_artifacts_bytes = None;
        self.recover_partial_publication()?;
        self.validate()?;
        Ok(report)
    }

    pub fn set_direction(
        &mut self,
        project: &CanonicalProject,
        direction: ShotDirection,
    ) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        if self.snapshot()?.locked {
            return Err(ShotWorkflowError::Locked);
        }
        if self.shot.direction == direction {
            return Ok(());
        }
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;

        let mut next = self.shot.clone();
        next.direction = direction;
        write_json(project, &shot_path(&self.key, "shot.json")?, &next)?;
        self.shot = next;
        Ok(())
    }

    /// Decide the next generation without writing anything.
    pub fn plan_generation(
        &self,
        project: &CanonicalProject,
    ) -> Result<GenerationPlan, ShotWorkflowError> {
        self.plan_generation_from(project, &[])
    }

    /// [`Self::plan_generation`] for candidates compiled from `parents`, such
    /// as the locked style and casts, which the plan's input hash covers.
    pub fn plan_generation_from(
        &self,
        project: &CanonicalProject,
        parents: &[ArtifactDependency],
    ) -> Result<GenerationPlan, ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        if self.snapshot()?.locked {
            return Err(ShotWorkflowError::Locked);
        }
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let revision = self
            .shot
            .generation_revision
            .checked_add(1)
            .ok_or(ShotWorkflowError::GenerationOverflow)?;
        let input_hash = generation_input_hash(&self.shot, parents)?;
        Ok(GenerationPlan {
            shot_id: self.key.shot.clone(),
            revision,
            input_hash,
            candidate_ids: candidate_ids(&self.shot.shot_id, revision)?,
        })
    }

    /// Publish a planned generation with the provider's outputs.
    ///
    /// Idempotent: if every planned candidate is already in canonical state,
    /// nothing is written. This is what lets an interrupted generation be
    /// completed after a restart without duplicating candidates. A plan older
    /// than a generation that has since been published is refused.
    pub fn attach_generation(
        &mut self,
        project: &CanonicalProject,
        plan: &GenerationPlan,
        outputs: &[CandidateOutput],
    ) -> Result<AttachOutcome, ShotWorkflowError> {
        if plan.shot_id != self.key.shot
            || outputs.len() != plan.candidate_ids.len()
            || outputs
                .iter()
                .zip(&plan.candidate_ids)
                .any(|(output, planned)| {
                    &output.artifact_id != planned
                        || output.provider.trim().is_empty()
                        || output.model.trim().is_empty()
                })
        {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        if self.is_attached(plan) {
            // Records with the planned IDs are only this plan's attachment if
            // they hold its outputs. Anything else must not be acknowledged
            // as the provider's result.
            self.check_attached(plan, outputs)?;
            return Ok(AttachOutcome::AlreadyAttached);
        }
        if self.shot.generation_revision > plan.revision {
            return Err(ShotWorkflowError::StaleGeneration);
        }
        if self.shot.generation_revision.saturating_add(1) < plan.revision {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        self.ensure_mutation_allowed()?;
        if self.snapshot()?.locked {
            return Err(ShotWorkflowError::Locked);
        }
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        for output in outputs {
            verify_output(project, &self.key, output)?;
        }

        let mut next_shot = self.shot.clone();
        next_shot.generation_revision = plan.revision;
        let mut next_artifacts = self.artifacts.clone();
        supersede_active_shot_candidates(&mut next_artifacts)?;

        let mut candidate_artifact_ids = Vec::with_capacity(outputs.len());
        for output in outputs {
            if next_artifacts
                .iter()
                .any(|record| record.artifact_id == output.artifact_id.as_str())
            {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
            let provenance = output.provenance.as_ref();
            next_artifacts.push(StoredArtifactRecord {
                artifact_id: output.artifact_id.as_str().to_owned(),
                schema_version: ARTIFACT_SCHEMA_VERSION,
                artifact_type: "shot_candidate".to_owned(),
                status: StoredArtifactStatus::Candidate,
                content_hash: output.content_hash.as_str().to_owned(),
                input_hash: Some(plan.input_hash.as_str().to_owned()),
                provider: Some(output.provider.clone()),
                model: Some(output.model.clone()),
                media_path: output
                    .media_path
                    .as_ref()
                    .map(|path| path.as_path().to_string_lossy().into_owned()),
                media_state: output.media_path.as_ref().map(|_| MediaState::Present),
                compiler_version: provenance.map(|p| p.compiler_version.clone()),
                compiled_prompt: provenance.map(|p| p.compiled_prompt.clone()),
                prompt_hash: provenance.map(|p| p.prompt_hash.clone()),
                applied_constraints: provenance.map(|p| p.applied_constraints.clone()),
                dropped_constraints: provenance.map(|p| p.dropped_constraints.clone()),
                degradation: provenance.map(PromptProvenance::degradation),
                dependencies: output
                    .dependencies
                    .iter()
                    .map(|dependency| StoredArtifactDependency {
                        artifact_id: dependency.artifact_id.as_str().to_owned(),
                        content_hash: dependency.content_hash.as_str().to_owned(),
                        impact: dependency.impact.into(),
                        field: dependency.field.clone(),
                        extra: BTreeMap::new(),
                    })
                    .collect(),
                extra: BTreeMap::from([(INTENT_RECORD.to_owned(), intent_snapshot(&self.shot)?)]),
            });
            candidate_artifact_ids.push(output.artifact_id.as_str().to_owned());
        }
        let next_selection = StoredSelectionManifest {
            selected_artifact_id: None,
            candidate_artifact_ids,
            extra: self.selection.extra.clone(),
        };

        self.persist(project, &next_shot, &next_artifacts, &next_selection)?;
        self.shot = next_shot;
        self.artifacts = next_artifacts;
        self.selection = next_selection;
        Ok(AttachOutcome::Attached)
    }

    /// Generate with the deterministic local demo generator, without a job.
    /// Production generation goes through `kineto_core::generation`, which
    /// records write-ahead intents around the same plan and attach steps.
    pub fn generate(&mut self, project: &CanonicalProject) -> Result<(), ShotWorkflowError> {
        let plan = self.plan_generation(project)?;
        let outputs = plan
            .candidate_ids
            .iter()
            .map(demo_candidate_output)
            .collect::<Result<Vec<_>, _>>()?;
        self.attach_generation(project, &plan, &outputs).map(|_| ())
    }

    pub fn select(
        &mut self,
        project: &CanonicalProject,
        candidate_index: usize,
    ) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        let snapshot = self.snapshot()?;
        if snapshot.locked {
            return Err(ShotWorkflowError::Locked);
        }
        if !snapshot.generated {
            return Err(ShotWorkflowError::NotGenerated);
        }
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;

        let selection = self.selection.domain()?;
        let selected = selection
            .candidate_artifact_ids
            .get(candidate_index)
            .cloned()
            .ok_or(ShotWorkflowError::InvalidCandidate)?;
        let mut next_artifacts = self.artifacts.clone();
        if let Some(previous) = &selection.selected_artifact_id
            && previous != &selected
        {
            find_artifact_mut(&mut next_artifacts, previous)
                .ok_or(ShotWorkflowError::InvalidCanonicalState)?
                .transition(ArtifactStatus::Candidate)?;
        }
        let selected_artifact = find_artifact_mut(&mut next_artifacts, &selected)
            .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
        if selected_artifact.status == StoredArtifactStatus::Candidate {
            selected_artifact.transition(ArtifactStatus::Selected)?;
        }
        let mut next_selection = self.selection.clone();
        next_selection.select(&selected)?;

        // Artifact status is published before the selection pointer. If the process
        // stops between the two atomic writes, load() reconciles status back to
        // selection.json, which is the authority for the human selection.
        write_json(
            project,
            &shot_path(&self.key, "artifacts.json")?,
            &next_artifacts,
        )?;
        write_json(
            project,
            &shot_path(&self.key, "selection.json")?,
            &next_selection,
        )?;
        self.artifacts = next_artifacts;
        self.selection = next_selection;
        Ok(())
    }

    pub fn lock(&mut self, project: &CanonicalProject) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        let snapshot = self.snapshot()?;
        if snapshot.locked {
            return Err(ShotWorkflowError::AlreadyLocked);
        }
        if snapshot.stale {
            return Err(ShotWorkflowError::StaleSelection);
        }
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;

        let selected = self
            .selection
            .domain()?
            .selected_artifact_id
            .ok_or(ShotWorkflowError::NoSelection)?;
        let mut next_artifacts = self.artifacts.clone();
        find_artifact_mut(&mut next_artifacts, &selected)
            .ok_or(ShotWorkflowError::InvalidCanonicalState)?
            .transition(ArtifactStatus::Locked)?;
        write_json(
            project,
            &shot_path(&self.key, "artifacts.json")?,
            &next_artifacts,
        )?;
        self.artifacts = next_artifacts;
        Ok(())
    }

    pub fn reset(&mut self, project: &CanonicalProject) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let mut next_shot = self.shot.clone();
        next_shot.direction = ShotDirection::Reaction;
        let mut next_artifacts = self.artifacts.clone();
        supersede_active_shot_candidates(&mut next_artifacts)?;
        supersede_active_videos(&mut next_artifacts)?;
        let next_selection = StoredSelectionManifest {
            selected_artifact_id: None,
            candidate_artifact_ids: Vec::new(),
            extra: self.selection.extra.clone(),
        };

        self.persist(project, &next_shot, &next_artifacts, &next_selection)?;
        self.shot = next_shot;
        self.artifacts = next_artifacts;
        self.selection = next_selection;
        Ok(())
    }

    /// The locked keyframe a video would start from.
    fn locked_keyframe(&self) -> Option<&StoredArtifactRecord> {
        self.selection
            .selected_artifact_id
            .as_ref()
            .and_then(|selected| ArtifactId::new(selected.clone()).ok())
            .and_then(|selected| self.artifact(&selected))
            .filter(|artifact| artifact.status == StoredArtifactStatus::Locked)
    }

    /// The videos of the shot's latest video generation that are not
    /// superseded, in candidate order.
    fn current_videos(&self) -> Vec<&StoredArtifactRecord> {
        if self.shot.video_revision == 0 {
            return Vec::new();
        }
        let mut videos: Vec<_> = self
            .artifacts
            .iter()
            .filter(|artifact| {
                artifact.artifact_type == VIDEO_ARTIFACT_TYPE
                    && artifact.status != StoredArtifactStatus::Superseded
                    && parse_video_id(&artifact.artifact_id)
                        .is_some_and(|(_, revision, _)| revision == self.shot.video_revision)
            })
            .collect();
        videos.sort_by(|left, right| left.artifact_id.cmp(&right.artifact_id));
        videos
    }

    /// The video that stands for the shot: the locked one, else the
    /// selected one, else the first of the latest generation.
    fn current_video(&self) -> Option<&StoredArtifactRecord> {
        let videos = self.current_videos();
        [StoredArtifactStatus::Locked, StoredArtifactStatus::Selected]
            .iter()
            .find_map(|status| videos.iter().find(|video| video.status == *status))
            .or_else(|| videos.first())
            .copied()
    }

    fn video_state(&self) -> Result<VideoState, ShotWorkflowError> {
        let Some(video) = self.current_video() else {
            return Ok(VideoState::None);
        };
        let current = self.video_stale_causes(video)?.is_empty();
        Ok(match (current, video.status) {
            (false, _) => VideoState::Stale,
            (true, StoredArtifactStatus::Locked) => VideoState::Locked,
            (true, _) => VideoState::Ready,
        })
    }

    /// Why the current video no longer follows the shot: `first_frame` when
    /// it was not made from the locked keyframe's bytes, and each intent
    /// field changed since it was asked for, as `intent.<field>`. Empty
    /// without a video, or for one that is current.
    pub fn video_staleness(&self) -> Result<Vec<String>, ShotWorkflowError> {
        match self.current_video() {
            Some(video) => self.video_stale_causes(video),
            None => Ok(Vec::new()),
        }
    }

    /// Staleness is computed: the video's keyframe dependency must still be
    /// the locked keyframe, with the same bytes, and the intent it recorded
    /// must still be the shot's.
    fn video_stale_causes(
        &self,
        video: &StoredArtifactRecord,
    ) -> Result<Vec<String>, ShotWorkflowError> {
        let from_locked_keyframe = self.locked_keyframe().is_some_and(|keyframe| {
            video.dependencies.iter().any(|dependency| {
                dependency.artifact_id == keyframe.artifact_id
                    && dependency.content_hash == keyframe.content_hash
            })
        });
        let mut causes = Vec::new();
        if !from_locked_keyframe {
            causes.push("first_frame".to_owned());
        }
        causes.extend(self.changed_intent(video)?);
        Ok(causes)
    }

    /// The shot's intent as a generation records it, field by field.
    pub fn intent_record(&self) -> Result<Value, ShotWorkflowError> {
        intent_snapshot(&self.shot)
    }

    /// The locked keyframe a video would start from, as the dependency the
    /// video records and the keyframe's media, if it has any.
    #[allow(clippy::type_complexity)]
    pub fn first_frame(
        &self,
    ) -> Result<Option<(ArtifactDependency, Option<ProjectRelativePath>)>, ShotWorkflowError> {
        let Some(keyframe) = self.locked_keyframe() else {
            return Ok(None);
        };
        let dependency = ArtifactDependency {
            artifact_id: ArtifactId::new(keyframe.artifact_id.clone())?,
            content_hash: ContentHash::new(keyframe.content_hash.clone())?,
            impact: DependencyImpact::Semantic,
            field: Some("first_frame".to_owned()),
        };
        let media = keyframe
            .media_path
            .as_ref()
            .map(|path| ProjectRelativePath::new(path).map_err(ShotWorkflowError::Path))
            .transpose()?;
        Ok(Some((dependency, media)))
    }

    /// Decide the next video generation, of one candidate, without writing
    /// anything.
    pub fn plan_video(
        &self,
        project: &CanonicalProject,
        spec: VideoGenerationSpec<'_>,
    ) -> Result<VideoPlan, ShotWorkflowError> {
        self.plan_videos(project, spec, 1)?
            .pop()
            .ok_or(ShotWorkflowError::InvalidCanonicalState)
    }

    /// Decide the next video generation, of `count` candidates, without
    /// writing anything. It starts from the locked keyframe.
    pub fn plan_videos(
        &self,
        project: &CanonicalProject,
        spec: VideoGenerationSpec<'_>,
        count: u8,
    ) -> Result<Vec<VideoPlan>, ShotWorkflowError> {
        if count == 0 || count > MAX_VIDEO_CANDIDATES {
            return Err(ShotWorkflowError::InvalidCandidate);
        }
        self.ensure_mutation_allowed()?;
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let (dependency, media) = self
            .first_frame()?
            .ok_or(ShotWorkflowError::KeyframeNotLocked)?;
        if self.video_state()? == VideoState::Locked {
            return Err(ShotWorkflowError::VideoLocked);
        }
        let revision = self
            .shot
            .video_revision
            .checked_add(1)
            .ok_or(ShotWorkflowError::GenerationOverflow)?;
        let mut intent = Vec::with_capacity(self.shot.shot_id.len() + 1);
        intent.extend_from_slice(self.shot.shot_id.as_bytes());
        intent.push(self.shot.direction.code());
        // Unchanged for a generation without notes or a branch.
        for note in spec.notes {
            intent.extend_from_slice(&(note.len() as u64).to_be_bytes());
            intent.extend_from_slice(note.as_bytes());
        }
        if let Some(branch) = spec.branched_from {
            intent.extend_from_slice(b"branch:");
            intent.extend_from_slice(branch.as_bytes());
        }
        let input_hash = InputHash::derive(InputHashMaterial {
            dependencies: std::slice::from_ref(&dependency),
            canonical_intent: &intent,
            provider: spec.provider,
            model: spec.model,
            canonical_params: spec.canonical_params,
            compiler_version: spec.compiler_version,
        });
        (0..count)
            .map(|candidate| {
                Ok(VideoPlan {
                    shot_id: self.key.shot.clone(),
                    revision,
                    artifact_id: video_artifact_id(&self.shot.shot_id, revision, candidate)?,
                    input_hash: input_hash.clone(),
                    keyframe: Some((dependency.clone(), media.clone())),
                })
            })
            .collect()
    }

    /// Whether the planned video is already in canonical state.
    #[must_use]
    pub fn is_video_attached(&self, plan: &VideoPlan) -> bool {
        plan.shot_id == self.key.shot && self.artifact(&plan.artifact_id).is_some()
    }

    /// Publish a planned video. Idempotent, like keyframe attach: a video
    /// already in canonical state is not written again.
    pub fn attach_video(
        &mut self,
        project: &CanonicalProject,
        plan: &VideoPlan,
        output: &VideoOutput,
    ) -> Result<AttachOutcome, ShotWorkflowError> {
        if plan.shot_id != self.key.shot
            || output.artifact_id != plan.artifact_id
            || output.provider.trim().is_empty()
            || output.model.trim().is_empty()
        {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        if self.is_video_attached(plan) {
            // Only this video, exactly as generated, counts as attached; the
            // job is acknowledged on the strength of it.
            self.ensure_mutation_allowed()?;
            self.verify_attached_video(plan, output)?;
            return Ok(AttachOutcome::AlreadyAttached);
        }
        if self.shot.video_revision.saturating_add(1) < plan.revision {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        self.ensure_mutation_allowed()?;
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        if let Some(provenance) = &output.provenance {
            provenance.validate()?;
        }
        let expected = video_media_path(&self.key, &plan.artifact_id)?;
        if output.media_path != expected {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
        let bytes = project
            .root()
            .read(&output.media_path)
            .map_err(ShotWorkflowError::Fs)?;
        if ContentHash::of(&bytes) != output.content_hash {
            return Err(ShotWorkflowError::MediaMismatch(
                expected.as_path().to_string_lossy().into_owned(),
            ));
        }

        // A paid video is never thrown away. A candidate of an older
        // generation that finishes late, or of a newer one that finishes
        // after the director locked a video, is kept as superseded.
        let locked = self
            .current_videos()
            .iter()
            .any(|video| video.status == StoredArtifactStatus::Locked);
        let late = plan.revision < self.shot.video_revision
            || (plan.revision > self.shot.video_revision && locked);
        let mut next_shot = self.shot.clone();
        let mut next_artifacts = self.artifacts.clone();
        if !late && plan.revision > self.shot.video_revision {
            next_shot.video_revision = plan.revision;
            supersede_active_videos(&mut next_artifacts)?;
        }
        let mut extra = BTreeMap::new();
        if let Some(intent) = &output.intent {
            extra.insert(INTENT_RECORD.to_owned(), intent.clone());
        }
        if !output.notes.is_empty() {
            extra.insert(NOTES_RECORD.to_owned(), serde_json::json!(output.notes));
        }
        if let Some(branch) = &output.branched_from {
            extra.insert(BRANCH_RECORD.to_owned(), Value::String(branch.clone()));
        }
        let provenance = output.provenance.as_ref();
        next_artifacts.push(StoredArtifactRecord {
            artifact_id: plan.artifact_id.as_str().to_owned(),
            schema_version: ARTIFACT_SCHEMA_VERSION,
            artifact_type: VIDEO_ARTIFACT_TYPE.to_owned(),
            status: if late {
                StoredArtifactStatus::Superseded
            } else {
                StoredArtifactStatus::Candidate
            },
            content_hash: output.content_hash.as_str().to_owned(),
            input_hash: Some(plan.input_hash.as_str().to_owned()),
            provider: Some(output.provider.clone()),
            model: Some(output.model.clone()),
            media_path: Some(expected.as_path().to_string_lossy().into_owned()),
            media_state: Some(MediaState::Present),
            compiler_version: provenance.map(|p| p.compiler_version.clone()),
            compiled_prompt: provenance.map(|p| p.compiled_prompt.clone()),
            prompt_hash: provenance.map(|p| p.prompt_hash.clone()),
            applied_constraints: provenance.map(|p| p.applied_constraints.clone()),
            dropped_constraints: provenance.map(|p| p.dropped_constraints.clone()),
            degradation: provenance.map(PromptProvenance::degradation),
            dependencies: vec![StoredArtifactDependency {
                artifact_id: output.keyframe.artifact_id.as_str().to_owned(),
                content_hash: output.keyframe.content_hash.as_str().to_owned(),
                impact: output.keyframe.impact.into(),
                field: output.keyframe.field.clone(),
                extra: BTreeMap::new(),
            }],
            extra,
        });

        // The revision is published first; a video revision without its
        // record is no current video, and attaching again completes it.
        write_json(project, &shot_path(&self.key, "shot.json")?, &next_shot)?;
        write_json(
            project,
            &shot_path(&self.key, "artifacts.json")?,
            &next_artifacts,
        )?;
        self.shot = next_shot;
        self.artifacts = next_artifacts;
        Ok(AttachOutcome::Attached)
    }

    fn verify_attached_video(
        &self,
        plan: &VideoPlan,
        output: &VideoOutput,
    ) -> Result<(), ShotWorkflowError> {
        let record = self
            .artifact(&plan.artifact_id)
            .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
        let media_path = output.media_path.as_path().to_string_lossy().into_owned();
        let prompt_hash = output
            .provenance
            .as_ref()
            .map(|provenance| provenance.prompt_hash.as_str());
        let keyframe = &output.keyframe;
        let depends_on_keyframe = record.dependencies.len() == 1
            && record.dependencies[0].artifact_id == keyframe.artifact_id.as_str()
            && record.dependencies[0].content_hash == keyframe.content_hash.as_str()
            && record.dependencies[0].field == keyframe.field;
        if record.artifact_type != VIDEO_ARTIFACT_TYPE
            || record.status == StoredArtifactStatus::Draft
            || record.input_hash.as_deref() != Some(plan.input_hash.as_str())
            || record.content_hash != output.content_hash.as_str()
            || record.provider.as_deref() != Some(output.provider.as_str())
            || record.model.as_deref() != Some(output.model.as_str())
            || record.media_path.as_deref() != Some(media_path.as_str())
            || record.prompt_hash.as_deref() != prompt_hash
            || record.extra.get(INTENT_RECORD) != output.intent.as_ref()
            || stored_notes(record) != output.notes
            || stored_branch(record) != output.branched_from
            || !depends_on_keyframe
        {
            return Err(ShotWorkflowError::AttachedRecordMismatch(
                plan.artifact_id.as_str().to_owned(),
            ));
        }
        Ok(())
    }

    /// Choose one of the latest generation's videos, by its place among
    /// them, to lock. Choosing again moves the choice.
    pub fn select_video(
        &mut self,
        project: &CanonicalProject,
        index: usize,
    ) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        let videos: Vec<(String, StoredArtifactStatus)> = self
            .current_videos()
            .iter()
            .map(|video| (video.artifact_id.clone(), video.status))
            .collect();
        if videos
            .iter()
            .any(|(_, status)| *status == StoredArtifactStatus::Locked)
        {
            return Err(ShotWorkflowError::VideoLocked);
        }
        let (chosen, _) = videos
            .get(index)
            .ok_or(ShotWorkflowError::InvalidCandidate)?;
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let mut next_artifacts = self.artifacts.clone();
        for (artifact_id, status) in &videos {
            let target = match (artifact_id == chosen, status) {
                (true, StoredArtifactStatus::Candidate) => ArtifactStatus::Selected,
                (false, StoredArtifactStatus::Selected) => ArtifactStatus::Candidate,
                _ => continue,
            };
            find_artifact_mut(&mut next_artifacts, &ArtifactId::new(artifact_id.clone())?)
                .ok_or(ShotWorkflowError::InvalidCanonicalState)?
                .transition(target)?;
        }
        write_json(
            project,
            &shot_path(&self.key, "artifacts.json")?,
            &next_artifacts,
        )?;
        self.artifacts = next_artifacts;
        Ok(())
    }

    /// Approve the chosen video, or the only one of the latest generation,
    /// as production truth.
    pub fn lock_video(&mut self, project: &CanonicalProject) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        let videos = self.current_videos();
        if videos.is_empty() {
            return Err(ShotWorkflowError::NoVideo);
        }
        if videos
            .iter()
            .any(|video| video.status == StoredArtifactStatus::Locked)
        {
            return Err(ShotWorkflowError::AlreadyLocked);
        }
        let chosen = match videos
            .iter()
            .find(|video| video.status == StoredArtifactStatus::Selected)
        {
            Some(video) => *video,
            None if videos.len() == 1 => videos[0],
            None => return Err(ShotWorkflowError::NoSelection),
        };
        if !self.video_stale_causes(chosen)?.is_empty() {
            return Err(ShotWorkflowError::StaleVideo);
        }
        let artifact_id = ArtifactId::new(chosen.artifact_id.clone())?;
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let mut next_artifacts = self.artifacts.clone();
        let video = find_artifact_mut(&mut next_artifacts, &artifact_id)
            .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
        if video.status == StoredArtifactStatus::Candidate {
            video.transition(ArtifactStatus::Selected)?;
        }
        video.transition(ArtifactStatus::Locked)?;
        write_json(
            project,
            &shot_path(&self.key, "artifacts.json")?,
            &next_artifacts,
        )?;
        self.artifacts = next_artifacts;
        Ok(())
    }

    /// Every video the shot has had, current or superseded, oldest
    /// generation first and in candidate order within one: what the
    /// director compares across takes.
    pub fn video_history(&self) -> Result<Vec<VideoRecord>, ShotWorkflowError> {
        let mut history = self
            .artifacts
            .iter()
            .filter(|artifact| artifact.artifact_type == VIDEO_ARTIFACT_TYPE)
            .map(|artifact| {
                let (_, generation, candidate) = parse_video_id(&artifact.artifact_id)
                    .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
                Ok(VideoRecord {
                    artifact_id: artifact.artifact_id.clone(),
                    generation,
                    candidate,
                    status: artifact.status.into(),
                    media_path: artifact
                        .kept_media()
                        .map(|path| ProjectRelativePath::new(path).map_err(ShotWorkflowError::Path))
                        .transpose()?,
                    origin: CandidateOrigin {
                        provider: artifact.provider.clone().unwrap_or_default(),
                        model: artifact.model.clone().unwrap_or_default(),
                    },
                    notes: stored_notes(artifact),
                    branched_from: stored_branch(artifact),
                    stale: if artifact.status == StoredArtifactStatus::Superseded {
                        Vec::new()
                    } else {
                        self.video_stale_causes(artifact)?
                    },
                })
            })
            .collect::<Result<Vec<_>, ShotWorkflowError>>()?;
        history.sort_by(|left, right| {
            (left.generation, left.candidate).cmp(&(right.generation, right.candidate))
        });
        Ok(history)
    }

    /// Mark the media of `artifact_ids` as let go. Their records stay,
    /// with their prompts and provenance; the caller removes the files
    /// after.
    pub fn evict_media(
        &mut self,
        project: &CanonicalProject,
        artifact_ids: &[String],
    ) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        project
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let mut next_artifacts = self.artifacts.clone();
        for artifact_id in artifact_ids {
            let artifact =
                find_artifact_mut(&mut next_artifacts, &ArtifactId::new(artifact_id.clone())?)
                    .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
            if artifact.media_path.is_none() {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
            artifact.media_state = Some(MediaState::Evicted);
        }
        write_json(
            project,
            &shot_path(&self.key, "artifacts.json")?,
            &next_artifacts,
        )?;
        self.artifacts = next_artifacts;
        Ok(())
    }

    /// Every media file the shot's records name: its keyframes and videos,
    /// current or superseded.
    pub fn media(&self) -> Result<Vec<MediaRecord>, ShotWorkflowError> {
        self.artifacts
            .iter()
            .filter_map(|artifact| {
                let path = artifact.media_path.as_ref()?;
                Some((artifact, path))
            })
            .map(|(artifact, path)| {
                Ok(MediaRecord {
                    owner: MediaOwner::Shot(self.key.clone()),
                    artifact_id: artifact.artifact_id.clone(),
                    kind: if artifact.artifact_type == VIDEO_ARTIFACT_TYPE {
                        MediaKind::Video
                    } else {
                        MediaKind::Keyframe
                    },
                    path: ProjectRelativePath::new(path).map_err(ShotWorkflowError::Path)?,
                    content_hash: ContentHash::new(artifact.content_hash.clone())
                        .map_err(ShotWorkflowError::Hash)?,
                    state: artifact.media_state.unwrap_or_default(),
                    standing: ArtifactStatus::from(artifact.status).into(),
                    dependencies: artifact
                        .dependencies
                        .iter()
                        .map(|dependency| dependency.artifact_id.clone())
                        .collect(),
                })
            })
            .collect()
    }

    /// Absolute-path-free location of the current video's media, if any.
    pub fn video_media(&self) -> Result<Option<ProjectRelativePath>, ShotWorkflowError> {
        self.current_video()
            .and_then(StoredArtifactRecord::kept_media)
            .map(|path| ProjectRelativePath::new(path).map_err(ShotWorkflowError::Path))
            .transpose()
    }

    fn persist(
        &self,
        project: &CanonicalProject,
        shot: &ShotProductionManifest,
        artifacts: &[StoredArtifactRecord],
        selection: &StoredSelectionManifest,
    ) -> Result<(), ShotWorkflowError> {
        // Every file is replaced atomically, but the three files are not an ACID
        // transaction. Publish the monotonic generation revision first, artifact
        // records second, and the selection pointer last. load() recognizes the
        // intermediate states and reconciles them so every completed file boundary
        // is forward-recoverable after a process or power failure.
        write_json(project, &shot_path(&self.key, "shot.json")?, shot)?;
        write_json(project, &shot_path(&self.key, "artifacts.json")?, artifacts)?;
        write_json(project, &shot_path(&self.key, "selection.json")?, selection)
    }

    fn normalize_legacy_demo_hashes(&mut self) -> Result<(), ShotWorkflowError> {
        let shot_id = self.shot.shot_id.clone();
        let mut issues = Vec::new();
        let mut migration_records = Vec::new();

        for artifact in &mut self.artifacts {
            artifact.validate_structure()?;
            if ContentHash::new(artifact.content_hash.clone()).is_err() {
                let artifact_id = ArtifactId::new(artifact.artifact_id.clone())?;
                let legacy_content_hash = format!("demo:{}", artifact_id.as_str());
                if artifact.artifact_type == "shot_candidate"
                    && artifact.content_hash == legacy_content_hash
                {
                    let from = artifact.content_hash.clone();
                    let to = demo_content_hash(&artifact_id)?.as_str().to_owned();
                    artifact.content_hash.clone_from(&to);
                    migration_records.push(ShotHashMigrationRecord {
                        artifact_id: artifact.artifact_id.clone(),
                        field: "content_hash",
                        from,
                        to,
                    });
                } else {
                    issues.push(ShotHashIssue {
                        artifact_id: artifact.artifact_id.clone(),
                        field: "content_hash",
                        value: artifact.content_hash.clone(),
                    });
                }
            }

            if let Some(stored_input_hash) = artifact.input_hash.as_mut()
                && InputHash::new(stored_input_hash.clone()).is_err()
            {
                if artifact.artifact_type == "shot_candidate"
                    && let Some(direction) =
                        legacy_input_hash_direction(&shot_id, stored_input_hash)
                {
                    let from = stored_input_hash.clone();
                    let to = input_hash_for(&shot_id, direction)?.as_str().to_owned();
                    stored_input_hash.clone_from(&to);
                    migration_records.push(ShotHashMigrationRecord {
                        artifact_id: artifact.artifact_id.clone(),
                        field: "input_hash",
                        from,
                        to,
                    });
                } else {
                    issues.push(ShotHashIssue {
                        artifact_id: artifact.artifact_id.clone(),
                        field: "input_hash",
                        value: stored_input_hash.clone(),
                    });
                }
            }

            for dependency in &artifact.dependencies {
                if ContentHash::new(dependency.content_hash.clone()).is_err() {
                    issues.push(ShotHashIssue {
                        artifact_id: artifact.artifact_id.clone(),
                        field: "dependencies[].content_hash",
                        value: dependency.content_hash.clone(),
                    });
                }
            }
        }

        self.hash_issues = issues;
        self.hash_migration_records = migration_records;
        Ok(())
    }

    /// The canonical record of `output`'s candidate must be exactly what
    /// attaching `plan` with `output` wrote.
    /// Prove that `plan` is already in canonical state with exactly
    /// `outputs`, before its jobs are acknowledged.
    pub fn check_attached(
        &self,
        plan: &GenerationPlan,
        outputs: &[CandidateOutput],
    ) -> Result<(), ShotWorkflowError> {
        self.ensure_mutation_allowed()?;
        for output in outputs {
            self.verify_attached(plan, output)?;
        }
        Ok(())
    }

    fn verify_attached(
        &self,
        plan: &GenerationPlan,
        output: &CandidateOutput,
    ) -> Result<(), ShotWorkflowError> {
        let record = self
            .artifact(&output.artifact_id)
            .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
        let media_path = output
            .media_path
            .as_ref()
            .map(|path| path.as_path().to_string_lossy().into_owned());
        let prompt_hash = output
            .provenance
            .as_ref()
            .map(|provenance| provenance.prompt_hash.as_str());
        if record.artifact_type != "shot_candidate"
            || record.status == StoredArtifactStatus::Draft
            || record.input_hash.as_deref() != Some(plan.input_hash.as_str())
            || record.content_hash != output.content_hash.as_str()
            || record.provider.as_deref() != Some(output.provider.as_str())
            || record.model.as_deref() != Some(output.model.as_str())
            || record.media_path != media_path
            || record.prompt_hash.as_deref() != prompt_hash
        {
            return Err(ShotWorkflowError::AttachedRecordMismatch(
                output.artifact_id.as_str().to_owned(),
            ));
        }
        Ok(())
    }

    fn ensure_mutation_allowed(&self) -> Result<(), ShotWorkflowError> {
        if let Some(issue) = self.hash_issues.first() {
            return Err(invalid_stored_hash(issue));
        }
        if !self.hash_migration_records.is_empty() {
            return Err(ShotWorkflowError::MigrationRequired);
        }
        Ok(())
    }

    fn recover_partial_publication(&mut self) -> Result<(), ShotWorkflowError> {
        self.recover_generation_selection()?;
        self.recover_cleared_selection()?;
        self.reconcile_selection_statuses()
    }

    fn recover_generation_selection(&mut self) -> Result<(), ShotWorkflowError> {
        if self.shot.generation_revision == 0 {
            return Ok(());
        }
        let expected = candidate_ids(&self.shot.shot_id, self.shot.generation_revision)?;
        let expected_strings = expected
            .iter()
            .map(|artifact_id| artifact_id.as_str().to_owned())
            .collect::<Vec<_>>();
        if self.selection.candidate_artifact_ids == expected_strings {
            return Ok(());
        }

        let generation_is_fully_published = expected.iter().all(|artifact_id| {
            self.artifact(artifact_id).is_some_and(|artifact| {
                artifact.artifact_type == "shot_candidate"
                    && matches!(
                        artifact.status,
                        StoredArtifactStatus::Candidate
                            | StoredArtifactStatus::Selected
                            | StoredArtifactStatus::Locked
                    )
            })
        });
        if generation_is_fully_published {
            self.selection.candidate_artifact_ids = expected_strings;
            self.selection.selected_artifact_id = None;
        }
        Ok(())
    }

    fn recover_cleared_selection(&mut self) -> Result<(), ShotWorkflowError> {
        if self.selection.candidate_artifact_ids.is_empty() {
            return Ok(());
        }
        let all_superseded = self
            .selection
            .candidate_artifact_ids
            .iter()
            .all(|candidate| {
                ArtifactId::new(candidate.clone())
                    .ok()
                    .is_some_and(|candidate| {
                        self.artifact(&candidate).is_some_and(|artifact| {
                            artifact.status == StoredArtifactStatus::Superseded
                        })
                    })
            });
        if all_superseded {
            self.selection.candidate_artifact_ids.clear();
            self.selection.selected_artifact_id = None;
        }
        Ok(())
    }

    fn reconcile_selection_statuses(&mut self) -> Result<(), ShotWorkflowError> {
        let selection = self.selection.domain()?;
        let candidate_ids = selection
            .candidate_artifact_ids
            .iter()
            .map(|artifact_id| artifact_id.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let selected_id = selection
            .selected_artifact_id
            .as_ref()
            .map(|artifact_id| artifact_id.as_str().to_owned());

        for artifact in &mut self.artifacts {
            // Videos have their own approval, outside the keyframe selection.
            if artifact.artifact_type == VIDEO_ARTIFACT_TYPE {
                continue;
            }
            let is_candidate = candidate_ids.contains(&artifact.artifact_id);
            let is_selected = selected_id.as_deref() == Some(artifact.artifact_id.as_str());
            match (is_candidate, is_selected, artifact.status) {
                (true, true, StoredArtifactStatus::Candidate) => {
                    artifact.transition(ArtifactStatus::Selected)?;
                }
                (true, true, StoredArtifactStatus::Selected | StoredArtifactStatus::Locked) => {}
                (true, true, StoredArtifactStatus::Draft | StoredArtifactStatus::Superseded) => {
                    return Err(ShotWorkflowError::InvalidCanonicalState);
                }
                (true, false, StoredArtifactStatus::Selected) => {
                    artifact.transition(ArtifactStatus::Candidate)?;
                }
                (true, false, StoredArtifactStatus::Candidate) => {}
                (
                    true,
                    false,
                    StoredArtifactStatus::Draft
                    | StoredArtifactStatus::Locked
                    | StoredArtifactStatus::Superseded,
                ) => return Err(ShotWorkflowError::InvalidCanonicalState),
                (false, _, StoredArtifactStatus::Selected | StoredArtifactStatus::Locked) => {
                    return Err(ShotWorkflowError::InvalidCanonicalState);
                }
                (false, _, _) => {}
            }
        }
        Ok(())
    }

    fn validate(&self) -> Result<(), ShotWorkflowError> {
        let selection = self.selection.domain()?;
        let mut artifact_ids = BTreeSet::new();
        for artifact in &self.artifacts {
            artifact.validate_structure()?;
            let artifact_id = ArtifactId::new(artifact.artifact_id.clone())?;
            if !artifact_ids.insert(artifact_id) {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
            if self.hash_issues.is_empty() {
                artifact.domain()?;
            }
        }
        for candidate in &selection.candidate_artifact_ids {
            let artifact = self
                .artifact(candidate)
                .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
            if !matches!(
                artifact.status,
                StoredArtifactStatus::Candidate
                    | StoredArtifactStatus::Selected
                    | StoredArtifactStatus::Locked
            ) {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
        }
        if let Some(selected) = &selection.selected_artifact_id {
            let artifact = self
                .artifact(selected)
                .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
            if !matches!(
                artifact.status,
                StoredArtifactStatus::Selected | StoredArtifactStatus::Locked
            ) {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
        }
        let selected_id = selection
            .selected_artifact_id
            .as_ref()
            .map(|artifact_id| artifact_id.as_str());
        for artifact in &self.artifacts {
            if artifact.artifact_type != VIDEO_ARTIFACT_TYPE
                && matches!(
                    artifact.status,
                    StoredArtifactStatus::Selected | StoredArtifactStatus::Locked
                )
                && selected_id != Some(artifact.artifact_id.as_str())
            {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
        }
        Ok(())
    }

    fn artifact(&self, artifact_id: &ArtifactId) -> Option<&StoredArtifactRecord> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.artifact_id == artifact_id.as_str())
    }

    /// Whether a current candidate was generated for another intent than
    /// the shot's. A candidate made from locks is checked against the hash
    /// the same locks give the current intent; whether the locks themselves
    /// changed is the generation layer's to say.
    fn is_stale(&self) -> Result<bool, ShotWorkflowError> {
        let selection = self.selection.domain()?;
        for candidate in &selection.candidate_artifact_ids {
            let Some(artifact) = self.artifact(candidate) else {
                return Ok(true);
            };
            let parents = artifact
                .dependencies
                .iter()
                .map(|dependency| {
                    Ok(ArtifactDependency {
                        artifact_id: ArtifactId::new(dependency.artifact_id.clone())
                            .map_err(ShotWorkflowError::ArtifactId)?,
                        content_hash: ContentHash::new(dependency.content_hash.clone())
                            .map_err(ShotWorkflowError::Hash)?,
                        impact: dependency.impact.into(),
                        field: dependency.field.clone(),
                    })
                })
                .collect::<Result<Vec<_>, ShotWorkflowError>>()?;
            let expected = generation_input_hash(&self.shot, &parents)?;
            if artifact.input_hash.as_deref() != Some(expected.as_str())
                || !self.changed_intent(artifact)?.is_empty()
            {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The intent fields that changed since `artifact` was made, as
    /// `intent.<field>`; none for a candidate that recorded no intent.
    fn changed_intent(
        &self,
        artifact: &StoredArtifactRecord,
    ) -> Result<Vec<String>, ShotWorkflowError> {
        let Some(Value::Object(recorded)) = artifact.extra.get(INTENT_RECORD) else {
            return Ok(Vec::new());
        };
        let Value::Object(current) = intent_snapshot(&self.shot)? else {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        };
        Ok(INTENT_FIELDS
            .iter()
            .filter(|field| recorded.get(**field) != current.get(**field))
            .map(|field| format!("intent.{field}"))
            .collect())
    }
}

/// The intent `shot` is written with now, field by field.
fn intent_snapshot(shot: &ShotProductionManifest) -> Result<Value, ShotWorkflowError> {
    let mut snapshot = serde_json::Map::new();
    snapshot.insert(
        "direction".to_owned(),
        serde_json::to_value(shot.direction).map_err(ShotWorkflowError::Json)?,
    );
    for field in &INTENT_FIELDS[1..] {
        if let Some(value) = shot.extra.get(*field) {
            snapshot.insert((*field).to_owned(), value.clone());
        }
    }
    Ok(Value::Object(snapshot))
}

/// The input hash of a generation of `shot` compiled from `parents`: the
/// intent's alone without any, otherwise derived from both.
fn generation_input_hash(
    shot: &ShotProductionManifest,
    parents: &[ArtifactDependency],
) -> Result<InputHash, ShotWorkflowError> {
    let intent = input_hash(shot)?;
    Ok(if parents.is_empty() {
        intent
    } else {
        InputHash::derive(InputHashMaterial {
            dependencies: parents,
            canonical_intent: intent.as_str().as_bytes(),
            provider: "",
            model: "",
            canonical_params: b"",
            compiler_version: "",
        })
    })
}

/// A candidate's media must be the shot's own keyframe file, present, and
/// exactly the bytes its content hash names.
fn verify_output(
    project: &CanonicalProject,
    key: &ShotKey,
    output: &CandidateOutput,
) -> Result<(), ShotWorkflowError> {
    if let Some(provenance) = &output.provenance {
        provenance.validate()?;
    }
    let Some(media_path) = &output.media_path else {
        return Ok(());
    };
    let expected_dir = shot_path(key, "keyframes")?;
    if media_path.as_path().parent() != Some(expected_dir.as_path())
        || media_path
            .as_path()
            .file_stem()
            .and_then(|stem| stem.to_str())
            != Some(output.artifact_id.as_str())
    {
        return Err(ShotWorkflowError::InvalidCanonicalState);
    }
    let bytes = project
        .root()
        .read(media_path)
        .map_err(ShotWorkflowError::Fs)?;
    if ContentHash::of(&bytes) != output.content_hash {
        return Err(ShotWorkflowError::MediaMismatch(
            media_path.as_path().to_string_lossy().into_owned(),
        ));
    }
    Ok(())
}

fn invalid_stored_hash(issue: &ShotHashIssue) -> ShotWorkflowError {
    ShotWorkflowError::InvalidStoredHash {
        artifact_id: issue.artifact_id.clone(),
        field: issue.field,
        value: issue.value.clone(),
    }
}

fn supersede_active_shot_candidates(
    artifacts: &mut [StoredArtifactRecord],
) -> Result<(), ShotWorkflowError> {
    for artifact in artifacts {
        if artifact.artifact_type != "shot_candidate" {
            continue;
        }
        match artifact.status {
            StoredArtifactStatus::Candidate | StoredArtifactStatus::Selected => {
                artifact.transition(ArtifactStatus::Superseded)?;
            }
            StoredArtifactStatus::Locked => {
                artifact.transition(ArtifactStatus::Superseded)?;
            }
            StoredArtifactStatus::Draft | StoredArtifactStatus::Superseded => {}
        }
    }
    Ok(())
}

fn supersede_active_videos(
    artifacts: &mut [StoredArtifactRecord],
) -> Result<(), ShotWorkflowError> {
    for artifact in artifacts {
        if artifact.artifact_type == VIDEO_ARTIFACT_TYPE
            && artifact.status != StoredArtifactStatus::Superseded
            && artifact.status != StoredArtifactStatus::Draft
        {
            artifact.transition(ArtifactStatus::Superseded)?;
        }
    }
    Ok(())
}

/// `<shot_id>_vRRRR_candidate_x`, `x` naming the candidate from `a`.
fn video_artifact_id(
    shot_id: &str,
    revision: u16,
    candidate: u8,
) -> Result<ArtifactId, ShotWorkflowError> {
    let letter = char::from(b'a' + candidate.min(MAX_VIDEO_CANDIDATES - 1));
    ArtifactId::new(format!("{shot_id}_v{revision:04}_candidate_{letter}"))
        .map_err(ShotWorkflowError::ArtifactId)
}

/// The director's notes a video record keeps, oldest first.
fn stored_notes(artifact: &StoredArtifactRecord) -> Vec<String> {
    match artifact.extra.get(NOTES_RECORD) {
        Some(Value::Array(notes)) => notes
            .iter()
            .filter_map(|note| note.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// The take a video record was branched from.
fn stored_branch(artifact: &StoredArtifactRecord) -> Option<String> {
    artifact
        .extra
        .get(BRANCH_RECORD)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// A four-digit revision from 0001 on.
fn parse_revision(value: &str) -> Option<u16> {
    if value.len() != 4 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value.parse::<u16>().ok().filter(|revision| *revision != 0)
}

/// Parse `<shot_id>_vRRRR_candidate_x`, or a video made before a
/// generation could have several, `<shot_id>_vRRRR`, into its shot ID,
/// video revision and candidate.
fn parse_video_id(value: &str) -> Option<(&str, u16, Option<u8>)> {
    let (rest, candidate) = match value.rsplit_once("_candidate_") {
        Some((rest, letter)) => {
            let [letter] = letter.as_bytes() else {
                return None;
            };
            let candidate = letter.checked_sub(b'a')?;
            (candidate < MAX_VIDEO_CANDIDATES).then_some((rest, Some(candidate)))?
        }
        None => (value, None),
    };
    let (shot, revision) = rest.rsplit_once("_v")?;
    ShotKey::is_id(shot).then_some((shot, parse_revision(revision)?, candidate))
}

fn input_hash(shot: &ShotProductionManifest) -> Result<InputHash, ShotWorkflowError> {
    input_hash_for(&shot.shot_id, shot.direction)
}

fn input_hash_for(shot_id: &str, direction: ShotDirection) -> Result<InputHash, ShotWorkflowError> {
    let mut hasher = Sha256::new();
    hasher.update(b"kineto:shot-intent:v1\0");
    let shot_id_len =
        u64::try_from(shot_id.len()).map_err(|_| ShotWorkflowError::InvalidCanonicalState)?;
    hasher.update(shot_id_len.to_le_bytes());
    hasher.update(shot_id.as_bytes());
    hasher.update([direction.code()]);
    InputHash::new(format!("sha256:{:x}", hasher.finalize())).map_err(ShotWorkflowError::Hash)
}

fn legacy_input_hash_direction(shot_id: &str, value: &str) -> Option<ShotDirection> {
    let prefix = format!("kineto:shot-intent:v1:{shot_id}:");
    ShotDirection::from_legacy_key(value.strip_prefix(&prefix)?)
}

fn demo_content_hash(artifact_id: &ArtifactId) -> Result<ContentHash, ShotWorkflowError> {
    let mut hasher = Sha256::new();
    hasher.update(b"kineto:demo-shot-candidate:v1\0");
    hasher.update(artifact_id.as_str().as_bytes());
    ContentHash::new(format!("sha256:{:x}", hasher.finalize())).map_err(ShotWorkflowError::Hash)
}

fn candidate_ids(shot_id: &str, revision: u16) -> Result<Vec<ArtifactId>, ShotWorkflowError> {
    (0..GENERATED_CANDIDATE_COUNT)
        .map(|index| {
            let label = char::from(b'a' + u8::try_from(index).expect("candidate count fits in u8"));
            ArtifactId::new(format!("{shot_id}_g{revision:04}_candidate_{label}"))
                .map_err(ShotWorkflowError::ArtifactId)
        })
        .collect()
}

/// Parse `<shot_id>_gRRRR_candidate_x` back into its shot ID and revision.
fn parse_candidate_id(value: &str) -> Option<(&str, u16)> {
    let (rest, label) = value.rsplit_once("_candidate_")?;
    let (shot, revision) = rest.rsplit_once("_g")?;
    if label.len() != 1 || !ShotKey::is_id(shot) {
        return None;
    }
    let revision = parse_revision(revision)?;
    let index = usize::from(label.as_bytes()[0].checked_sub(b'a')?);
    (index < GENERATED_CANDIDATE_COUNT).then_some((shot, revision))
}

fn find_artifact_mut<'a>(
    artifacts: &'a mut [StoredArtifactRecord],
    artifact_id: &ArtifactId,
) -> Option<&'a mut StoredArtifactRecord> {
    artifacts
        .iter_mut()
        .find(|artifact| artifact.artifact_id == artifact_id.as_str())
}

fn shot_path(key: &ShotKey, file: &str) -> Result<ProjectRelativePath, ShotWorkflowError> {
    scene_path(&key.scene, &format!("shots/{}/{file}", key.shot))
}

fn scene_path(scene_id: &str, file: &str) -> Result<ProjectRelativePath, ShotWorkflowError> {
    if !ShotKey::is_id(scene_id) {
        return Err(ShotWorkflowError::InvalidShotNumber);
    }
    ProjectRelativePath::new(format!("scenes/{scene_id}/{file}")).map_err(ShotWorkflowError::Path)
}

/// Where a shot lives: its scene and its own ID, both portable identifiers
/// of at most 64 characters. Shot IDs are unique across the project.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShotKey {
    scene: String,
    shot: String,
}

impl ShotKey {
    /// The longest scene or shot ID.
    pub const MAX_ID: usize = 64;

    pub fn new(
        scene: impl Into<String>,
        shot: impl Into<String>,
    ) -> Result<Self, ShotWorkflowError> {
        let (scene, shot) = (scene.into(), shot.into());
        if Self::is_id(&scene) && Self::is_id(&shot) {
            Ok(Self { scene, shot })
        } else {
            Err(ShotWorkflowError::InvalidShotNumber)
        }
    }

    /// Shot `number` of the bootstrap scene, as in `scene_001/shot_002`.
    #[must_use]
    pub fn bootstrap(number: u32) -> Self {
        Self {
            scene: BOOTSTRAP_SCENE.to_owned(),
            shot: format!("shot_{number:03}"),
        }
    }

    #[must_use]
    pub fn scene(&self) -> &str {
        &self.scene
    }

    #[must_use]
    pub fn shot(&self) -> &str {
        &self.shot
    }

    /// Whether `value` can name a scene or a shot.
    #[must_use]
    pub fn is_id(value: &str) -> bool {
        value.len() <= Self::MAX_ID && crate::story::is_portable_id(value)
    }
}

/// Whether `shot_id` names a bootstrap shot, as in `shot_002`.
fn is_bootstrap_shot(shot_id: &str) -> bool {
    shot_id.strip_prefix("shot_").is_some_and(|number| {
        number.len() >= 3
            && number.bytes().all(|byte| byte.is_ascii_digit())
            && number.parse::<u32>().is_ok_and(|number| number != 0)
    })
}

/// The scene every project created by the desktop app starts with.
pub const BOOTSTRAP_SCENE: &str = "scene_001";

/// Shots of the bootstrap scene, `shot_001` on.
pub const BOOTSTRAP_SHOTS: u32 = 2;

/// Where shot `shot_id` lives: the scene directory holding it, found by
/// looking. `None` when no scene has it.
pub fn locate(
    project: &CanonicalProject,
    shot_id: &str,
) -> Result<Option<ShotKey>, ShotWorkflowError> {
    if !ShotKey::is_id(shot_id) {
        return Ok(None);
    }
    let scenes = project
        .root()
        .resolve(&ProjectRelativePath::new("scenes").map_err(ShotWorkflowError::Path)?);
    let entries = match std::fs::read_dir(&scenes) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(ShotWorkflowError::Fs(crate::fs::ProjectFsError::Io(error))),
    };
    let mut found = Vec::new();
    for entry in entries.into_iter().flatten() {
        let entry =
            entry.map_err(|error| ShotWorkflowError::Fs(crate::fs::ProjectFsError::Io(error)))?;
        let Some(scene) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if ShotKey::is_id(&scene)
            && entry
                .path()
                .join("shots")
                .join(shot_id)
                .join("shot.json")
                .is_file()
        {
            found.push(ShotKey::new(scene, shot_id)?);
        }
    }
    match found.len() {
        // A bootstrap shot is in the bootstrap scene, written or not.
        0 => Ok(is_bootstrap_shot(shot_id).then(|| ShotKey {
            scene: BOOTSTRAP_SCENE.to_owned(),
            shot: shot_id.to_owned(),
        })),
        1 => Ok(found.pop()),
        // A shot ID names one shot; two scenes claiming it is damage.
        _ => Err(ShotWorkflowError::InvalidCanonicalState),
    }
}

fn read_bytes_optional(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
) -> Result<Option<Vec<u8>>, ShotWorkflowError> {
    match project.root().read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ShotWorkflowError::Fs(error)),
    }
}

fn read_json_optional<T: DeserializeOwned>(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
) -> Result<Option<T>, ShotWorkflowError> {
    read_bytes_optional(project, path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(ShotWorkflowError::Json))
        .transpose()
}

fn write_json<T: Serialize + ?Sized>(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
    value: &T,
) -> Result<(), ShotWorkflowError> {
    project
        .validate_for_write()
        .map_err(ShotWorkflowError::Manifest)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(ShotWorkflowError::Json)?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(path, &bytes)
        .map_err(ShotWorkflowError::Fs)
}

#[derive(Debug)]
pub enum ShotWorkflowError {
    InvalidShotNumber,
    InvalidCanonicalState,
    NotGenerated,
    InvalidCandidate,
    Locked,
    AlreadyLocked,
    NoSelection,
    StaleSelection,
    GenerationOverflow,
    /// A generation newer than the one being attached is already published.
    StaleGeneration,
    MigrationRequired,
    InvalidStoredHash {
        artifact_id: String,
        field: &'static str,
        value: String,
    },
    MigrationBackupConflict(String),
    /// A candidate's media file is missing its bytes or holds other bytes
    /// than its content hash names.
    MediaMismatch(String),
    /// A video starts from the locked keyframe; this shot has none.
    KeyframeNotLocked,
    /// The shot's video is locked; it cannot be regenerated.
    VideoLocked,
    /// There is no current video to lock.
    NoVideo,
    /// The video was made from a keyframe that is no longer the locked one.
    StaleVideo,
    /// A candidate already in canonical state does not hold the output its
    /// generation produced, so that generation cannot be acknowledged.
    AttachedRecordMismatch(String),
    InvalidTarget(std::path::PathBuf),
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Store(ProjectStoreError),
    Manifest(ProjectManifestError),
    Json(serde_json::Error),
    ArtifactId(ArtifactIdError),
    Hash(HashValueError),
    Lifecycle(LifecycleError),
    Selection(SelectionError),
}

impl fmt::Display for ShotWorkflowError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidShotNumber => formatter.write_str("shot number must be at least 1"),
            Self::InvalidCanonicalState => {
                formatter.write_str("invalid canonical shot approval state")
            }
            Self::NotGenerated => formatter.write_str("generate candidates before selecting"),
            Self::InvalidCandidate => formatter.write_str("candidate index is out of range"),
            Self::Locked => formatter.write_str("locked shot cannot be changed"),
            Self::AlreadyLocked => formatter.write_str("shot selection is already locked"),
            Self::NoSelection => formatter.write_str("select a candidate before locking"),
            Self::StaleSelection => {
                formatter.write_str("selected candidate is stale; regenerate before locking")
            }
            Self::GenerationOverflow => formatter.write_str("shot generation revision overflow"),
            Self::StaleGeneration => formatter
                .write_str("a newer shot generation is already published; this one cannot attach"),
            Self::MigrationRequired => {
                formatter.write_str("legacy shot hashes require explicit migration before writing")
            }
            Self::InvalidStoredHash {
                artifact_id,
                field,
                value,
            } => write!(
                formatter,
                "artifact {artifact_id} has invalid {field}: {value}"
            ),
            Self::MigrationBackupConflict(path) => {
                write!(
                    formatter,
                    "legacy hash migration backup conflicts at {path}"
                )
            }
            Self::KeyframeNotLocked => {
                formatter.write_str("lock a keyframe before generating video")
            }
            Self::VideoLocked => formatter.write_str("the shot's video is locked"),
            Self::NoVideo => formatter.write_str("generate a video before locking it"),
            Self::StaleVideo => formatter.write_str(
                "the video no longer follows the shot: its locked keyframe or its intent changed \
                 since; regenerate it",
            ),
            Self::MediaMismatch(path) => write!(
                formatter,
                "candidate media does not match its content hash: {path}"
            ),
            Self::AttachedRecordMismatch(artifact_id) => write!(
                formatter,
                "candidate {artifact_id} in canonical state does not match its generation"
            ),
            Self::InvalidTarget(path) => {
                write!(formatter, "invalid shot state target: {}", path.display())
            }
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Store(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid canonical shot JSON: {error}"),
            Self::ArtifactId(error) => error.fmt(formatter),
            Self::Hash(error) => error.fmt(formatter),
            Self::Lifecycle(error) => error.fmt(formatter),
            Self::Selection(error) => error.fmt(formatter),
        }
    }
}

impl Error for ShotWorkflowError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Fs(error) => Some(error),
            Self::Store(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::ArtifactId(error) => Some(error),
            Self::Hash(error) => Some(error),
            Self::Lifecycle(error) => Some(error),
            Self::Selection(error) => Some(error),
            Self::InvalidShotNumber
            | Self::InvalidCanonicalState
            | Self::NotGenerated
            | Self::InvalidCandidate
            | Self::Locked
            | Self::AlreadyLocked
            | Self::NoSelection
            | Self::StaleSelection
            | Self::GenerationOverflow
            | Self::StaleGeneration
            | Self::MigrationRequired
            | Self::InvalidStoredHash { .. }
            | Self::MigrationBackupConflict(_)
            | Self::MediaMismatch(_)
            | Self::KeyframeNotLocked
            | Self::VideoLocked
            | Self::NoVideo
            | Self::StaleVideo
            | Self::AttachedRecordMismatch(_)
            | Self::InvalidTarget(_) => None,
        }
    }
}

impl From<ArtifactIdError> for ShotWorkflowError {
    fn from(value: ArtifactIdError) -> Self {
        Self::ArtifactId(value)
    }
}

impl From<HashValueError> for ShotWorkflowError {
    fn from(value: HashValueError) -> Self {
        Self::Hash(value)
    }
}

impl From<LifecycleError> for ShotWorkflowError {
    fn from(value: LifecycleError) -> Self {
        Self::Lifecycle(value)
    }
}

impl From<SelectionError> for ShotWorkflowError {
    fn from(value: SelectionError) -> Self {
        Self::Selection(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use std::{
        fs,
        path::{Path, PathBuf},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            // SystemTime is only microsecond-precise on macOS, so tests running in
            // parallel could otherwise share a directory name.
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let nonce = format!(
                "{nonce}-{}",
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            );
            let path = std::env::temp_dir()
                .join(format!("kineto-shot-store-{}-{nonce}", std::process::id()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(target: &Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "shot_test".to_owned(),
                title: "Shot Test".to_owned(),
                created_at: "2026-09-10T00:00:00Z".to_owned(),
                source: ProjectSource {
                    kind: "text".to_owned(),
                    path: "source/story.txt".to_owned(),
                    extra: BTreeMap::new(),
                },
                defaults: ProjectDefaults {
                    language: "en".to_owned(),
                    extra: BTreeMap::new(),
                },
                workflow: ProjectWorkflow {
                    recipe_id: "default-film".to_owned(),
                    recipe_version: 1,
                    extra: BTreeMap::new(),
                },
                extra: BTreeMap::new(),
            },
            b"source\n",
        )
        .unwrap()
    }

    fn shot_dir(target: &Path) -> PathBuf {
        target.join("scenes/scene_001/shots/shot_001")
    }

    #[derive(Clone)]
    struct StoredStateFiles {
        shot: Vec<u8>,
        artifacts: Vec<u8>,
        selection: Vec<u8>,
    }

    fn read_state_files(target: &Path) -> StoredStateFiles {
        let base = shot_dir(target);
        StoredStateFiles {
            shot: fs::read(base.join("shot.json")).unwrap(),
            artifacts: fs::read(base.join("artifacts.json")).unwrap(),
            selection: fs::read(base.join("selection.json")).unwrap(),
        }
    }

    fn write_state_files(target: &Path, state: &StoredStateFiles) {
        let base = shot_dir(target);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("shot.json"), &state.shot).unwrap();
        fs::write(base.join("artifacts.json"), &state.artifacts).unwrap();
        fs::write(base.join("selection.json"), &state.selection).unwrap();
    }

    #[test]
    fn written_intent_is_read_and_survives_a_canonical_write() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        assert_eq!(
            ShotWorkflow::load(&project, &ShotKey::bootstrap(1))
                .unwrap()
                .intent()
                .unwrap(),
            None
        );
        assert_eq!(load_scene_intent(&project, BOOTSTRAP_SCENE).unwrap(), None);

        fs::create_dir_all(shot_dir(&target)).unwrap();
        fs::write(
            shot_dir(&target).join("shot.json"),
            include_bytes!(
                "../../../fixtures/projects/minimal/scenes/scene_001/shots/shot_001/shot.json"
            ),
        )
        .unwrap();
        fs::write(
            target.join("scenes/scene_001/scene.json"),
            include_bytes!("../../../fixtures/projects/minimal/scenes/scene_001/scene.json"),
        )
        .unwrap();

        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        let expected = ShotIntent {
            action: "Alice picks up the sealed letter".to_owned(),
            framing: Some("medium_close_up".to_owned()),
            lens: Some("50mm".to_owned()),
            camera_motion: Some("slow_dolly_in".to_owned()),
            emotion: Some("cautious curiosity".to_owned()),
            dialogue: None,
            characters: vec!["alice".to_owned()],
            duration_target: Some(4.0),
            continuity: Vec::new(),
            continuity_from: None,
        };
        assert_eq!(shot.intent().unwrap(), Some(expected.clone()));
        assert_eq!(
            load_scene_intent(&project, BOOTSTRAP_SCENE).unwrap(),
            Some(SceneIntent {
                id: "scene_001".to_owned(),
                location: Some("empty_room".to_owned()),
                time: Some("day".to_owned()),
                dramatic_goal: Some("Alice discovers the letter".to_owned()),
                characters: vec!["alice".to_owned()],
            })
        );

        shot.set_direction(&project, ShotDirection::Tension)
            .unwrap();
        let reloaded = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert_eq!(reloaded.direction(), ShotDirection::Tension);
        assert_eq!(reloaded.intent().unwrap(), Some(expected));
    }

    fn spec() -> VideoGenerationSpec<'static> {
        VideoGenerationSpec {
            provider: "test-video",
            model: "clip-1",
            canonical_params: b"duration=6",
            compiler_version: "video-prompt/1",
            notes: &[],
            branched_from: None,
        }
    }

    /// Write a fake clip for `plan` and the output that names it.
    fn video_output(project: &CanonicalProject, plan: &VideoPlan, bytes: &[u8]) -> VideoOutput {
        let media_path = video_media_path(&ShotKey::bootstrap(1), plan.artifact_id()).unwrap();
        project.root().write_atomic(&media_path, bytes).unwrap();
        VideoOutput {
            artifact_id: plan.artifact_id().clone(),
            content_hash: ContentHash::of(bytes),
            provider: "test-video".to_owned(),
            model: "clip-1".to_owned(),
            media_path,
            provenance: None,
            keyframe: plan.keyframe().unwrap().0.clone(),
            intent: None,
            notes: Vec::new(),
            branched_from: None,
        }
    }

    fn locked_shot(project: &CanonicalProject) -> ShotWorkflow {
        let mut shot = ShotWorkflow::load(project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(project).unwrap();
        shot.select(project, 1).unwrap();
        shot.lock(project).unwrap();
        shot
    }

    #[test]
    fn a_video_starts_from_the_locked_keyframe_and_can_be_locked() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert!(matches!(
            shot.plan_video(&project, spec()),
            Err(ShotWorkflowError::KeyframeNotLocked)
        ));
        shot.generate(&project).unwrap();
        shot.select(&project, 1).unwrap();
        assert!(matches!(
            shot.plan_video(&project, spec()),
            Err(ShotWorkflowError::KeyframeNotLocked)
        ));
        shot.lock(&project).unwrap();
        assert!(matches!(
            shot.lock_video(&project),
            Err(ShotWorkflowError::NoVideo)
        ));

        let plan = shot.plan_video(&project, spec()).unwrap();
        assert_eq!(plan.artifact_id().as_str(), "shot_001_v0001_candidate_a");
        let (keyframe, _) = plan.keyframe().unwrap();
        assert_eq!(keyframe.artifact_id.as_str(), "shot_001_g0001_candidate_b");
        let output = video_output(&project, &plan, b"\0\0\0\x18ftypisom-one");
        assert_eq!(
            shot.attach_video(&project, &plan, &output).unwrap(),
            AttachOutcome::Attached
        );
        assert_eq!(
            shot.attach_video(&project, &plan, &output).unwrap(),
            AttachOutcome::AlreadyAttached
        );
        let snapshot = shot.snapshot().unwrap();
        assert_eq!(snapshot.video, VideoState::Ready);
        assert_eq!(snapshot.video_revision, 1);
        // The keyframe approval is unchanged by the video beside it.
        assert!(snapshot.locked && snapshot.selected_index == Some(1));
        assert_eq!(snapshot.superseded_count, 0);

        shot.lock_video(&project).unwrap();
        let reloaded = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert_eq!(reloaded.snapshot().unwrap().video, VideoState::Locked);
        assert_eq!(
            reloaded.video_media().unwrap().unwrap().as_path(),
            std::path::Path::new(
                "scenes/scene_001/shots/shot_001/videos/shot_001_v0001_candidate_a.mp4"
            )
        );
        assert!(matches!(
            reloaded.plan_video(&project, spec()),
            Err(ShotWorkflowError::VideoLocked)
        ));
    }

    #[test]
    fn regenerating_a_video_supersedes_the_last_and_keeps_its_record() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = locked_shot(&project);

        let first = shot.plan_video(&project, spec()).unwrap();
        let first_output = video_output(&project, &first, b"\0\0\0\x18ftypisom-one");
        shot.attach_video(&project, &first, &first_output).unwrap();
        let second = shot.plan_video(&project, spec()).unwrap();
        assert_eq!(second.artifact_id().as_str(), "shot_001_v0002_candidate_a");
        assert_eq!(
            second.input_hash(),
            first.input_hash(),
            "same keyframe, same inputs"
        );
        let output = video_output(&project, &second, b"\0\0\0\x18ftypisom-two");
        shot.attach_video(&project, &second, &output).unwrap();
        // Attaching an older video again changes nothing.
        assert_eq!(
            shot.attach_video(&project, &first, &first_output).unwrap(),
            AttachOutcome::AlreadyAttached
        );
        // But only with the output it was attached with.
        let mut other = first_output.clone();
        other.content_hash = ContentHash::of(b"another clip");
        assert!(matches!(
            shot.attach_video(&project, &first, &other),
            Err(ShotWorkflowError::AttachedRecordMismatch(_))
        ));
        let mut other = first_output.clone();
        other.keyframe.content_hash = ContentHash::of(b"another frame");
        assert!(matches!(
            shot.attach_video(&project, &first, &other),
            Err(ShotWorkflowError::AttachedRecordMismatch(_))
        ));

        let artifacts: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(shot_dir(&target).join("artifacts.json")).unwrap())
                .unwrap();
        let videos = artifacts
            .iter()
            .filter(|artifact| artifact["type"] == "shot_video")
            .map(|artifact| (artifact["artifact_id"].clone(), artifact["status"].clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            videos,
            [
                ("shot_001_v0001_candidate_a".into(), "superseded".into()),
                ("shot_001_v0002_candidate_a".into(), "candidate".into())
            ]
        );
        assert_eq!(
            artifacts[3]["dependencies"][0]["artifact_id"],
            "shot_001_g0001_candidate_b"
        );
        assert_eq!(artifacts[3]["dependencies"][0]["field"], "first_frame");
    }

    #[test]
    fn a_video_whose_keyframe_changed_is_stale_and_cannot_be_locked() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = locked_shot(&project);
        let plan = shot.plan_video(&project, spec()).unwrap();
        let output = video_output(&project, &plan, b"\0\0\0\x18ftypisom-one");
        shot.attach_video(&project, &plan, &output).unwrap();

        // The recorded dependency no longer names the locked keyframe's bytes.
        let path = shot_dir(&target).join("artifacts.json");
        let mut artifacts: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        artifacts[3]["dependencies"][0]["content_hash"] =
            format!("sha256:{}", "0".repeat(64)).into();
        fs::write(&path, serde_json::to_vec(&artifacts).unwrap()).unwrap();

        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert_eq!(shot.snapshot().unwrap().video, VideoState::Stale);
        assert!(matches!(
            shot.lock_video(&project),
            Err(ShotWorkflowError::StaleVideo)
        ));

        // Starting the shot over supersedes the video; its record stays.
        shot.reset(&project).unwrap();
        assert_eq!(shot.snapshot().unwrap().video, VideoState::None);
        let artifacts: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(artifacts[3]["status"], "superseded");
    }

    #[test]
    fn video_ids_name_their_shot_generation_and_candidate() {
        assert_eq!(
            parse_video_id("office_night_02_v0003_candidate_c"),
            Some(("office_night_02", 3, Some(2)))
        );
        // A video made before a generation could have several.
        assert_eq!(
            parse_video_id("shot_001_v0001"),
            Some(("shot_001", 1, None))
        );
        // A shot ID may itself hold `_v`.
        assert_eq!(
            parse_video_id("scene_the_view_01_v0002_candidate_a"),
            Some(("scene_the_view_01", 2, Some(0)))
        );
        for other in [
            "shot_001_g0001_candidate_a",
            "shot_001_v0001_candidate_z",
            "shot_001_v0001_candidate_ab",
            "shot_001_v0000_candidate_a",
            "shot_001",
        ] {
            assert_eq!(parse_video_id(other), None, "{other}");
        }
    }

    #[test]
    fn a_generations_candidates_are_chosen_from_and_late_ones_are_kept_aside() {
        let temp = TempDir::new();
        let project = project(&temp.0.join("film"));
        let mut shot = locked_shot(&project);
        assert!(matches!(
            shot.plan_videos(&project, spec(), 0),
            Err(ShotWorkflowError::InvalidCandidate)
        ));
        assert!(matches!(
            shot.plan_videos(&project, spec(), MAX_VIDEO_CANDIDATES + 1),
            Err(ShotWorkflowError::InvalidCandidate)
        ));
        let first = shot.plan_videos(&project, spec(), 3).unwrap();
        assert_eq!(
            first
                .iter()
                .map(|plan| (plan.artifact_id().as_str(), plan.revision()))
                .collect::<Vec<_>>(),
            [
                ("shot_001_v0001_candidate_a", 1),
                ("shot_001_v0001_candidate_b", 1),
                ("shot_001_v0001_candidate_c", 1),
            ]
        );
        assert!(
            first
                .iter()
                .all(|plan| plan.input_hash() == first[0].input_hash())
        );
        // Candidates finish in any order; each joins its generation.
        for (index, plan) in [(1, &first[1]), (0, &first[0])] {
            let output = video_output(
                &project,
                plan,
                format!("\0\0\0\x18ftypisom-{index}").as_bytes(),
            );
            shot.attach_video(&project, plan, &output).unwrap();
        }
        assert_eq!(shot.snapshot().unwrap().video, VideoState::Ready);
        assert!(matches!(
            shot.lock_video(&project),
            Err(ShotWorkflowError::NoSelection)
        ));
        assert!(matches!(
            shot.select_video(&project, 2),
            Err(ShotWorkflowError::InvalidCandidate)
        ));
        shot.select_video(&project, 0).unwrap();
        shot.select_video(&project, 1).unwrap();
        let statuses = |shot: &ShotWorkflow| {
            shot.video_history()
                .unwrap()
                .into_iter()
                .map(|take| (take.artifact_id, take.status))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            statuses(&shot),
            [
                (
                    "shot_001_v0001_candidate_a".to_owned(),
                    ArtifactStatus::Candidate
                ),
                (
                    "shot_001_v0001_candidate_b".to_owned(),
                    ArtifactStatus::Selected
                ),
            ]
        );

        // Another generation is asked for before the director locks; it
        // finishes only after they locked candidate b, so it is kept aside.
        let second = shot.plan_video(&project, spec()).unwrap();
        assert_eq!(second.revision(), 2);
        shot.lock_video(&project).unwrap();
        assert_eq!(shot.snapshot().unwrap().video, VideoState::Locked);
        assert!(matches!(
            shot.select_video(&project, 0),
            Err(ShotWorkflowError::VideoLocked)
        ));
        let output = video_output(&project, &second, b"\0\0\0\x18ftypisom-second");
        shot.attach_video(&project, &second, &output).unwrap();
        // The first generation's last candidate, finishing late, joins its
        // generation beside the locked one.
        let output = video_output(&project, &first[2], b"\0\0\0\x18ftypisom-late");
        shot.attach_video(&project, &first[2], &output).unwrap();
        assert_eq!(
            shot.attach_video(&project, &first[2], &output).unwrap(),
            AttachOutcome::AlreadyAttached
        );
        let reloaded = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert_eq!(reloaded.snapshot().unwrap().video, VideoState::Locked);
        assert_eq!(reloaded.snapshot().unwrap().video_revision, 1);
        assert_eq!(
            statuses(&reloaded),
            [
                (
                    "shot_001_v0001_candidate_a".to_owned(),
                    ArtifactStatus::Candidate
                ),
                (
                    "shot_001_v0001_candidate_b".to_owned(),
                    ArtifactStatus::Locked
                ),
                (
                    "shot_001_v0001_candidate_c".to_owned(),
                    ArtifactStatus::Candidate
                ),
                (
                    "shot_001_v0002_candidate_a".to_owned(),
                    ArtifactStatus::Superseded
                ),
            ]
        );
        assert!(
            reloaded
                .video_history()
                .unwrap()
                .iter()
                .all(|take| take.media_path.is_some() && take.stale.is_empty())
        );
    }

    #[test]
    fn a_video_must_be_its_own_file_with_its_own_bytes() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = locked_shot(&project);
        let plan = shot.plan_video(&project, spec()).unwrap();
        let mut output = video_output(&project, &plan, b"\0\0\0\x18ftypisom-one");
        output.content_hash = ContentHash::of(b"other");
        assert!(matches!(
            shot.attach_video(&project, &plan, &output),
            Err(ShotWorkflowError::MediaMismatch(_))
        ));
        assert_eq!(shot.snapshot().unwrap().video, VideoState::None);
        assert_eq!(
            VideoPlan::for_artifact(plan.artifact_id(), plan.input_hash().clone())
                .unwrap()
                .revision(),
            1
        );
        assert!(
            VideoPlan::for_artifact(
                &ArtifactId::new("shot_001_g0001_candidate_a").unwrap(),
                plan.input_hash().clone()
            )
            .is_err()
        );
    }

    #[test]
    fn an_intent_field_of_the_wrong_type_is_refused() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        fs::create_dir_all(shot_dir(&target)).unwrap();
        fs::write(
            shot_dir(&target).join("shot.json"),
            br#"{"shot_id":"shot_001","schema_version":1,"action":7}"#,
        )
        .unwrap();

        assert!(matches!(
            ShotWorkflow::load(&project, &ShotKey::bootstrap(1))
                .unwrap()
                .intent(),
            Err(ShotWorkflowError::Json(_))
        ));
    }

    #[test]
    fn committed_artifact_and_selection_fixtures_decode() {
        let artifacts: Vec<StoredArtifactRecord> = serde_json::from_str(include_str!(
            "../../../fixtures/projects/minimal/characters/alice/artifacts.json"
        ))
        .unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(
            artifacts[0].domain().unwrap().status,
            ArtifactStatus::Locked
        );

        let selection: StoredSelectionManifest = serde_json::from_str(include_str!(
            "../../../fixtures/projects/minimal/characters/alice/selection.json"
        ))
        .unwrap();
        assert!(selection.domain().unwrap().selected_artifact_id.is_some());
    }

    #[test]
    fn approval_state_survives_reopen_and_dot_kineto_deletion() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.set_direction(&project, ShotDirection::Tension)
            .unwrap();
        shot.generate(&project).unwrap();
        shot.select(&project, 2).unwrap();
        shot.lock(&project).unwrap();
        let before = shot.snapshot().unwrap();
        drop(project);

        fs::create_dir_all(target.join(".kineto")).unwrap();
        fs::write(target.join(".kineto/project.db"), b"derived").unwrap();
        fs::remove_dir_all(target.join(".kineto")).unwrap();

        let reopened = CanonicalProject::open(&target).unwrap();
        let reopened_shot = ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1)).unwrap();
        assert_eq!(reopened_shot.snapshot().unwrap(), before);
        assert_eq!(before.direction, ShotDirection::Tension);
        assert_eq!(before.selected_index, Some(2));
        assert!(before.locked);
    }

    #[test]
    fn direction_change_preserves_candidates_as_stale_until_regeneration() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();
        shot.select(&project, 1).unwrap();
        let selected_before = shot.selection.selected_artifact_id.clone();

        shot.set_direction(&project, ShotDirection::Intimacy)
            .unwrap();
        let stale = shot.snapshot().unwrap();
        assert!(stale.generated);
        assert!(stale.stale);
        assert_eq!(stale.selected_index, Some(1));
        assert_eq!(shot.selection.selected_artifact_id, selected_before);
        assert!(matches!(
            shot.lock(&project),
            Err(ShotWorkflowError::StaleSelection)
        ));

        shot.generate(&project).unwrap();
        let regenerated = shot.snapshot().unwrap();
        assert!(!regenerated.stale);
        assert_eq!(regenerated.generation_revision, 2);
        assert_eq!(regenerated.superseded_count, 3);
        assert_eq!(regenerated.selected_index, None);
    }

    #[test]
    fn canonical_json_round_trip_keeps_future_fields() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let base = shot_dir(&target);
        fs::create_dir_all(&base).unwrap();
        fs::write(
            base.join("shot.json"),
            br#"{"shot_id":"shot_001","schema_version":1,"generation_revision":1,"future_shot":"keep"}"#,
        )
        .unwrap();
        fs::write(
            base.join("artifacts.json"),
            br#"[{"artifact_id":"shot_001_g0001_candidate_a","schema_version":1,"type":"shot_candidate","status":"candidate","content_hash":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","input_hash":"sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","dependencies":[],"future_artifact":7}]"#,
        )
        .unwrap();
        fs::write(
            base.join("selection.json"),
            br#"{"selected_artifact_id":null,"candidate_artifact_ids":["shot_001_g0001_candidate_a"],"future_selection":true}"#,
        )
        .unwrap();

        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.set_direction(&project, ShotDirection::SpatialClarity)
            .unwrap();
        shot.generate(&project).unwrap();

        let shot_json: Value =
            serde_json::from_slice(&fs::read(base.join("shot.json")).unwrap()).unwrap();
        let artifacts_json: Value =
            serde_json::from_slice(&fs::read(base.join("artifacts.json")).unwrap()).unwrap();
        let selection_json: Value =
            serde_json::from_slice(&fs::read(base.join("selection.json")).unwrap()).unwrap();
        assert_eq!(shot_json["future_shot"], Value::String("keep".to_owned()));
        assert_eq!(artifacts_json[0]["future_artifact"], Value::from(7));
        assert_eq!(selection_json["future_selection"], Value::Bool(true));
    }

    #[test]
    fn canonical_shot_update_replaces_the_published_file() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.set_direction(&project, ShotDirection::Tension)
            .unwrap();

        let base = shot_dir(&target);
        let published = base.join("shot.json");
        let previous = base.join("shot.previous.json");
        fs::hard_link(&published, &previous).unwrap();

        shot.set_direction(&project, ShotDirection::Intimacy)
            .unwrap();

        let previous_json: Value = serde_json::from_slice(&fs::read(previous).unwrap()).unwrap();
        let current_json: Value = serde_json::from_slice(&fs::read(published).unwrap()).unwrap();
        assert_eq!(
            previous_json["direction"],
            Value::String("tension".to_owned())
        );
        assert_eq!(
            current_json["direction"],
            Value::String("intimacy".to_owned())
        );
    }

    #[test]
    fn generation_crash_points_are_forward_recoverable() {
        let temp = TempDir::new();
        let source_target = temp.0.join("source-film");
        let source_project = project(&source_target);
        let mut source_shot = ShotWorkflow::load(&source_project, &ShotKey::bootstrap(1)).unwrap();
        source_shot.generate(&source_project).unwrap();
        let generation_one = read_state_files(&source_target);
        source_shot.generate(&source_project).unwrap();
        let generation_two = read_state_files(&source_target);
        drop(source_project);

        let cases = [
            (
                "after-shot",
                StoredStateFiles {
                    shot: generation_two.shot.clone(),
                    artifacts: generation_one.artifacts.clone(),
                    selection: generation_one.selection.clone(),
                },
            ),
            (
                "after-artifacts",
                StoredStateFiles {
                    shot: generation_two.shot.clone(),
                    artifacts: generation_two.artifacts.clone(),
                    selection: generation_one.selection.clone(),
                },
            ),
            ("after-selection", generation_two.clone()),
        ];

        for (name, state) in cases {
            let target = temp.0.join(name);
            let project = project(&target);
            write_state_files(&target, &state);
            let mut recovered = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
            recovered.generate(&project).unwrap();
            let snapshot = recovered.snapshot().unwrap();
            assert_eq!(snapshot.generation_revision, 3, "case {name}");
            assert_eq!(snapshot.candidate_count, 3, "case {name}");
            assert_eq!(snapshot.selected_index, None, "case {name}");
        }
    }

    #[test]
    fn select_crash_reconciles_artifact_status_to_selection_pointer() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();
        shot.select(&project, 0).unwrap();
        let selection_before = fs::read(shot_dir(&target).join("selection.json")).unwrap();
        shot.select(&project, 1).unwrap();
        let artifacts_after = fs::read(shot_dir(&target).join("artifacts.json")).unwrap();
        drop(project);

        fs::write(shot_dir(&target).join("selection.json"), selection_before).unwrap();
        fs::write(shot_dir(&target).join("artifacts.json"), artifacts_after).unwrap();

        let reopened = CanonicalProject::open(&target).unwrap();
        let mut recovered = ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1)).unwrap();
        assert_eq!(recovered.snapshot().unwrap().selected_index, Some(0));
        assert_eq!(
            recovered
                .artifacts
                .iter()
                .filter(|artifact| artifact.status == StoredArtifactStatus::Selected)
                .count(),
            1
        );
        recovered.select(&reopened, 1).unwrap();
        assert_eq!(recovered.snapshot().unwrap().selected_index, Some(1));
    }

    #[test]
    fn reset_crash_after_artifact_publish_recovers_cleared_selection() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();
        shot.select(&project, 1).unwrap();
        let selection_before = fs::read(shot_dir(&target).join("selection.json")).unwrap();
        shot.reset(&project).unwrap();
        let reset_state = read_state_files(&target);
        drop(project);

        fs::write(shot_dir(&target).join("selection.json"), selection_before).unwrap();
        fs::write(shot_dir(&target).join("shot.json"), reset_state.shot).unwrap();
        fs::write(
            shot_dir(&target).join("artifacts.json"),
            reset_state.artifacts,
        )
        .unwrap();

        let reopened = CanonicalProject::open(&target).unwrap();
        let mut recovered = ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1)).unwrap();
        let snapshot = recovered.snapshot().unwrap();
        assert_eq!(snapshot.candidate_count, 0);
        assert_eq!(snapshot.selected_index, None);
        recovered.generate(&reopened).unwrap();
        assert_eq!(recovered.snapshot().unwrap().candidate_count, 3);
    }

    #[test]
    fn validate_rejects_orphaned_selected_and_locked_artifacts() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();
        shot.select(&project, 0).unwrap();

        let mut orphaned_selected = shot.clone();
        orphaned_selected.selection.selected_artifact_id = None;
        assert!(matches!(
            orphaned_selected.validate(),
            Err(ShotWorkflowError::InvalidCanonicalState)
        ));

        shot.lock(&project).unwrap();
        let mut orphaned_locked = shot.clone();
        orphaned_locked.selection.selected_artifact_id = None;
        assert!(matches!(
            orphaned_locked.validate(),
            Err(ShotWorkflowError::InvalidCanonicalState)
        ));
    }

    #[test]
    fn generated_hashes_are_sha256_digests() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();

        for artifact in &shot.artifacts {
            if artifact.status == StoredArtifactStatus::Candidate {
                assert!(artifact.content_hash.starts_with("sha256:"));
                assert_eq!(artifact.content_hash.len(), 71);
                let input_hash = artifact.input_hash.as_ref().unwrap();
                assert!(input_hash.starts_with("sha256:"));
                assert_eq!(input_hash.len(), 71);
            }
        }
    }

    #[test]
    fn legacy_demo_hashes_load_without_rewrite_and_migrate_with_backup() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.set_direction(&project, ShotDirection::Tension)
            .unwrap();
        shot.generate(&project).unwrap();
        shot.select(&project, 2).unwrap();
        shot.lock(&project).unwrap();
        let before = shot.snapshot().unwrap();

        let artifacts_path = shot_dir(&target).join("artifacts.json");
        let mut artifacts: Vec<StoredArtifactRecord> =
            serde_json::from_slice(&fs::read(&artifacts_path).unwrap()).unwrap();
        for artifact in &mut artifacts {
            artifact.content_hash = format!("demo:{}", artifact.artifact_id);
            artifact.input_hash = Some("kineto:shot-intent:v1:shot_001:tension".to_owned());
        }
        let legacy_bytes = serde_json::to_vec_pretty(&artifacts).unwrap();
        fs::write(&artifacts_path, &legacy_bytes).unwrap();
        drop(project);

        let reopened = CanonicalProject::open(&target).unwrap();
        let mut normalized = ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1)).unwrap();
        let mut expected_pending = before;
        expected_pending.migration_pending = true;
        assert_eq!(normalized.snapshot().unwrap(), expected_pending);
        assert_eq!(fs::read(&artifacts_path).unwrap(), legacy_bytes);
        assert_eq!(normalized.hash_issues(), &[]);
        assert!(matches!(
            normalized.reset(&reopened),
            Err(ShotWorkflowError::MigrationRequired)
        ));

        let report = normalized.migrate_legacy_hashes(&reopened).unwrap();
        assert_eq!(report.rewritten.len(), 6);
        let backup = report.backup_path.expect("migration creates backup");
        assert_eq!(fs::read(target.join(backup)).unwrap(), legacy_bytes);
        assert_eq!(normalized.snapshot().unwrap(), before);

        let rewritten: Vec<StoredArtifactRecord> =
            serde_json::from_slice(&fs::read(artifacts_path).unwrap()).unwrap();
        for artifact in rewritten {
            assert!(ContentHash::new(artifact.content_hash).is_ok());
            assert!(InputHash::new(artifact.input_hash.unwrap()).is_ok());
        }
    }

    #[test]
    fn read_only_legacy_hashes_open_without_write_and_refuse_migration() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();

        let shot_path = shot_dir(&target).join("shot.json");
        let artifacts_path = shot_dir(&target).join("artifacts.json");
        let selection_path = shot_dir(&target).join("selection.json");
        let shot_bytes = fs::read(&shot_path).unwrap();
        let selection_bytes = fs::read(&selection_path).unwrap();
        let mut artifacts: Vec<StoredArtifactRecord> =
            serde_json::from_slice(&fs::read(&artifacts_path).unwrap()).unwrap();
        for artifact in &mut artifacts {
            artifact.content_hash = format!("demo:{}", artifact.artifact_id);
            artifact.input_hash = Some("kineto:shot-intent:v1:shot_001:reaction".to_owned());
        }
        let legacy_bytes = serde_json::to_vec_pretty(&artifacts).unwrap();
        fs::write(&artifacts_path, &legacy_bytes).unwrap();
        drop(project);

        let read_only = CanonicalProject::open_with_supported_version_for_test(
            &target,
            PROJECT_FORMAT_VERSION + 1,
        )
        .unwrap();
        assert!(read_only.is_read_only());

        let mut first = ShotWorkflow::load(&read_only, &ShotKey::bootstrap(1)).unwrap();
        assert!(first.snapshot().unwrap().migration_pending);
        assert_eq!(fs::read(&shot_path).unwrap(), shot_bytes);
        assert_eq!(fs::read(&artifacts_path).unwrap(), legacy_bytes);
        assert_eq!(fs::read(&selection_path).unwrap(), selection_bytes);

        let reopened = CanonicalProject::open_with_supported_version_for_test(
            &target,
            PROJECT_FORMAT_VERSION + 1,
        )
        .unwrap();
        let second = ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1)).unwrap();
        assert!(second.snapshot().unwrap().migration_pending);
        assert_eq!(fs::read(&shot_path).unwrap(), shot_bytes);
        assert_eq!(fs::read(&artifacts_path).unwrap(), legacy_bytes);
        assert_eq!(fs::read(&selection_path).unwrap(), selection_bytes);

        assert!(matches!(
            first.migrate_legacy_hashes(&read_only),
            Err(ShotWorkflowError::Manifest(
                ProjectManifestError::ReadOnlyFormatVersion(PROJECT_FORMAT_VERSION)
            ))
        ));
        assert_eq!(fs::read(&shot_path).unwrap(), shot_bytes);
        assert_eq!(fs::read(&artifacts_path).unwrap(), legacy_bytes);
        assert_eq!(fs::read(&selection_path).unwrap(), selection_bytes);
        assert!(!shot_dir(&target).join(LEGACY_HASH_BACKUP_FILE).exists());
    }

    #[test]
    fn unknown_noncanonical_hash_opens_degraded_and_reports_record() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();

        let artifacts_path = shot_dir(&target).join("artifacts.json");
        let mut artifacts: Vec<StoredArtifactRecord> =
            serde_json::from_slice(&fs::read(&artifacts_path).unwrap()).unwrap();
        let artifact_id = artifacts[0].artifact_id.clone();
        artifacts[0].content_hash = "banana".to_owned();
        fs::write(
            &artifacts_path,
            serde_json::to_vec_pretty(&artifacts).unwrap(),
        )
        .unwrap();
        drop(project);

        let reopened = CanonicalProject::open(&target).unwrap();
        let mut degraded = ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1)).unwrap();
        let snapshot = degraded.snapshot().unwrap();
        assert_eq!(snapshot.invalid_hash_count, 1);
        assert!(!snapshot.migration_pending);
        assert_eq!(
            degraded.hash_issues(),
            &[ShotHashIssue {
                artifact_id: artifact_id.clone(),
                field: "content_hash",
                value: "banana".to_owned(),
            }]
        );
        assert!(matches!(
            degraded.reset(&reopened),
            Err(ShotWorkflowError::InvalidStoredHash {
                artifact_id: actual_id,
                field: "content_hash",
                value,
            }) if actual_id == artifact_id && value == "banana"
        ));
        assert!(matches!(
            degraded.migrate_legacy_hashes(&reopened),
            Err(ShotWorkflowError::InvalidStoredHash {
                artifact_id: actual_id,
                field: "content_hash",
                value,
            }) if actual_id == artifact_id && value == "banana"
        ));
    }

    fn demo_outputs(plan: &GenerationPlan) -> Vec<CandidateOutput> {
        plan.candidate_ids()
            .iter()
            .map(|id| demo_candidate_output(id).unwrap())
            .collect()
    }

    #[test]
    fn planning_a_generation_writes_nothing() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        let before = project.root().canonical_snapshot().unwrap();

        let plan = shot.plan_generation(&project).unwrap();
        assert_eq!(plan.shot_id(), "shot_001");
        assert_eq!(plan.revision(), 1);
        assert_eq!(plan.candidate_ids().len(), GENERATED_CANDIDATE_COUNT);
        assert_eq!(project.root().canonical_snapshot().unwrap(), before);
    }

    #[test]
    fn attaching_a_generation_twice_writes_once_and_records_provenance() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        let plan = shot.plan_generation(&project).unwrap();
        let outputs = demo_outputs(&plan);

        assert_eq!(
            shot.attach_generation(&project, &plan, &outputs).unwrap(),
            AttachOutcome::Attached
        );
        let published = project.root().canonical_snapshot().unwrap();
        assert_eq!(
            shot.attach_generation(&project, &plan, &outputs).unwrap(),
            AttachOutcome::AlreadyAttached
        );
        // A fresh load (as after a restart) sees the same generation as done.
        let mut reloaded = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert_eq!(
            reloaded
                .attach_generation(&project, &plan, &outputs)
                .unwrap(),
            AttachOutcome::AlreadyAttached
        );
        assert_eq!(project.root().canonical_snapshot().unwrap(), published);

        let artifacts: Vec<StoredArtifactRecord> =
            serde_json::from_slice(&fs::read(shot_dir(&target).join("artifacts.json")).unwrap())
                .unwrap();
        assert_eq!(artifacts.len(), GENERATED_CANDIDATE_COUNT);
        for artifact in artifacts {
            assert_eq!(artifact.provider.as_deref(), Some(DEMO_PROVIDER));
            assert_eq!(artifact.model.as_deref(), Some(DEMO_MODEL));
            assert_eq!(
                artifact.input_hash.as_deref(),
                Some(plan.input_hash().as_str())
            );
        }
    }

    #[test]
    fn an_orphaned_older_plan_cannot_attach_over_a_newer_generation() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();
        let orphan = shot.plan_generation(&project).unwrap();
        assert_eq!(orphan.revision(), 2);

        // Revision 2 was published before a crash, and a later generation
        // then moved the shot on to revision 3 without it.
        let mut half = shot.shot.clone();
        half.generation_revision = orphan.revision();
        write_json(
            &project,
            &shot_path(&ShotKey::bootstrap(1), "shot.json").unwrap(),
            &half,
        )
        .unwrap();
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();
        assert_eq!(shot.snapshot().unwrap().generation_revision, 3);

        assert!(matches!(
            shot.attach_generation(&project, &orphan, &demo_outputs(&orphan)),
            Err(ShotWorkflowError::StaleGeneration)
        ));
        // Revision 1 was published and is now superseded: reattaching it is a
        // no-op rather than an error or a duplicate.
        let first = GenerationPlan::for_candidate(
            &ArtifactId::new("shot_001_g0001_candidate_a").unwrap(),
            orphan.input_hash().clone(),
        )
        .unwrap();
        let before = project.root().canonical_snapshot().unwrap();
        assert_eq!(
            shot.attach_generation(&project, &first, &demo_outputs(&first))
                .unwrap(),
            AttachOutcome::AlreadyAttached
        );
        assert_eq!(project.root().canonical_snapshot().unwrap(), before);
    }

    #[test]
    fn a_half_published_generation_completes_when_attached_again() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        let plan = shot.plan_generation(&project).unwrap();

        // A crash after the revision was published but before the artifacts.
        let mut half = shot.shot.clone();
        half.generation_revision = plan.revision();
        write_json(
            &project,
            &shot_path(&ShotKey::bootstrap(1), "shot.json").unwrap(),
            &half,
        )
        .unwrap();

        let mut reloaded = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        assert!(!reloaded.snapshot().unwrap().generated);
        assert_eq!(
            reloaded
                .attach_generation(&project, &plan, &demo_outputs(&plan))
                .unwrap(),
            AttachOutcome::Attached
        );
        let snapshot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1))
            .unwrap()
            .snapshot()
            .unwrap();
        assert!(snapshot.generated);
        assert_eq!(snapshot.generation_revision, plan.revision());
        assert_eq!(
            usize::from(snapshot.candidate_count),
            GENERATED_CANDIDATE_COUNT
        );
    }

    #[test]
    fn candidate_ids_round_trip_into_their_plan() {
        let input = input_hash_for("shot_002", ShotDirection::Tension).unwrap();
        let candidate = ArtifactId::new("shot_002_g0007_candidate_c").unwrap();
        let plan = GenerationPlan::for_candidate(&candidate, input.clone()).unwrap();
        assert_eq!(plan.shot_id(), "shot_002");
        assert_eq!(plan.revision(), 7);
        assert_eq!(plan.input_hash(), &input);
        assert_eq!(plan.candidate_ids()[2], candidate);

        // A planned shot's ID is whatever the plan named it.
        let planned = ArtifactId::new("office_night_02_g0001_candidate_a").unwrap();
        let plan = GenerationPlan::for_candidate(&planned, input.clone()).unwrap();
        assert_eq!(plan.shot_id(), "office_night_02");
        assert_eq!(plan.revision(), 1);

        for malformed in [
            "shot_002_g07_candidate_a",
            "shot_002_g0000_candidate_a",
            "shot_002_g0001_candidate_d",
            "shot_002_g0001_candidate_ab",
            "g0001_candidate_a",
            "shot_002_candidate_a",
        ] {
            let id = ArtifactId::new(malformed).unwrap();
            assert!(
                GenerationPlan::for_candidate(&id, input.clone()).is_err(),
                "{malformed}"
            );
        }
    }

    #[test]
    fn legacy_and_unrecognised_hashes_together_refuse_migration() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, &ShotKey::bootstrap(1)).unwrap();
        shot.generate(&project).unwrap();

        let artifacts_path = shot_dir(&target).join("artifacts.json");
        let mut artifacts: Vec<StoredArtifactRecord> =
            serde_json::from_slice(&fs::read(&artifacts_path).unwrap()).unwrap();
        artifacts[0].content_hash = "banana".to_owned();
        artifacts[1].content_hash = format!("demo:{}", artifacts[1].artifact_id);
        let mixed = serde_json::to_vec_pretty(&artifacts).unwrap();
        fs::write(&artifacts_path, &mixed).unwrap();
        drop(project);

        // Both conditions are reported at once. Migration is refused while
        // any hash is unrecognised, so a caller must not offer it then.
        let reopened = CanonicalProject::open(&target).unwrap();
        let mut shot = ShotWorkflow::load(&reopened, &ShotKey::bootstrap(1)).unwrap();
        let snapshot = shot.snapshot().unwrap();
        assert!(snapshot.migration_pending);
        assert_eq!(snapshot.invalid_hash_count, 1);
        assert!(matches!(
            shot.migrate_legacy_hashes(&reopened),
            Err(ShotWorkflowError::InvalidStoredHash { .. })
        ));
        assert_eq!(fs::read(&artifacts_path).unwrap(), mixed);
        assert!(!shot_dir(&target).join(LEGACY_HASH_BACKUP_FILE).exists());
    }
}
