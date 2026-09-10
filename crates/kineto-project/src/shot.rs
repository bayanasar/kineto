use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt, fs,
    fs::OpenOptions,
    io::Write,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;

use crate::{
    ArtifactDependency, ArtifactId, ArtifactIdError, ArtifactRecord, ArtifactStatus, ContentHash,
    DependencyImpact, HashValueError, InputHash, LifecycleError, SelectionError, SelectionManifest,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath, ProjectRoot},
    manifest::{CanonicalProject, ProjectManifestError, ProjectStoreError},
};

const SHOT_SCHEMA_VERSION: u32 = 1;
const ARTIFACT_SCHEMA_VERSION: u32 = 1;
const GENERATED_CANDIDATE_COUNT: usize = 3;

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

    const fn key(self) -> &'static str {
        match self {
            Self::Reaction => "reaction",
            Self::SpatialClarity => "spatial_clarity",
            Self::Intimacy => "intimacy",
            Self::Tension => "tension",
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
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

const fn default_schema_version() -> u32 {
    SHOT_SCHEMA_VERSION
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
    dependencies: Vec<StoredArtifactDependency>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl StoredArtifactRecord {
    fn domain(&self) -> Result<ArtifactRecord, ShotWorkflowError> {
        if self.schema_version == 0 || self.artifact_type.trim().is_empty() {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }
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
    fn domain(&self) -> Result<ArtifactDependency, ShotWorkflowError> {
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
    shot_number: u32,
    shot: ShotProductionManifest,
    artifacts: Vec<StoredArtifactRecord>,
    selection: StoredSelectionManifest,
}

impl ShotWorkflow {
    pub fn load(project: &CanonicalProject, shot_number: u32) -> Result<Self, ShotWorkflowError> {
        if shot_number == 0 {
            return Err(ShotWorkflowError::InvalidShotNumber);
        }
        let shot_id = format!("shot_{shot_number:03}");
        let shot_file = shot_path(shot_number, "shot.json")?;
        let artifacts_file = shot_path(shot_number, "artifacts.json")?;
        let selection_file = shot_path(shot_number, "selection.json")?;

        let shot =
            read_json_optional(project, &shot_file)?.unwrap_or_else(|| ShotProductionManifest {
                shot_id: shot_id.clone(),
                schema_version: SHOT_SCHEMA_VERSION,
                direction: ShotDirection::Reaction,
                generation_revision: 0,
                extra: BTreeMap::new(),
            });
        if shot.shot_id != shot_id || shot.schema_version == 0 {
            return Err(ShotWorkflowError::InvalidCanonicalState);
        }

        let artifacts = read_json_optional(project, &artifacts_file)?.unwrap_or_default();
        let selection = read_json_optional(project, &selection_file)?.unwrap_or_default();
        let workflow = Self {
            shot_number,
            shot,
            artifacts,
            selection,
        };
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
                .filter(|artifact| artifact.status == StoredArtifactStatus::Superseded)
                .count(),
        )
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
        })
    }

    pub fn set_direction(
        &mut self,
        project: &CanonicalProject,
        direction: ShotDirection,
    ) -> Result<(), ShotWorkflowError> {
        if self.snapshot()?.locked {
            return Err(ShotWorkflowError::Locked);
        }
        if self.shot.direction == direction {
            return Ok(());
        }
        project
            .manifest()
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;

        let mut next = self.shot.clone();
        next.direction = direction;
        write_json(project, &shot_path(self.shot_number, "shot.json")?, &next)?;
        self.shot = next;
        Ok(())
    }

    pub fn generate(&mut self, project: &CanonicalProject) -> Result<(), ShotWorkflowError> {
        if self.snapshot()?.locked {
            return Err(ShotWorkflowError::Locked);
        }
        project
            .manifest()
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;

        let mut next_shot = self.shot.clone();
        next_shot.generation_revision = next_shot
            .generation_revision
            .checked_add(1)
            .ok_or(ShotWorkflowError::GenerationOverflow)?;
        let mut next_artifacts = self.artifacts.clone();
        for artifact_id in &self.selection.domain()?.candidate_artifact_ids {
            let artifact = find_artifact_mut(&mut next_artifacts, artifact_id)
                .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
            if artifact.status != StoredArtifactStatus::Superseded {
                artifact.transition(ArtifactStatus::Superseded)?;
            }
        }

        let input_hash = input_hash(&next_shot)?;
        let mut candidate_artifact_ids = Vec::with_capacity(GENERATED_CANDIDATE_COUNT);
        for index in 0..GENERATED_CANDIDATE_COUNT {
            let label = char::from(b'a' + u8::try_from(index).expect("candidate count fits in u8"));
            let artifact_id = ArtifactId::new(format!(
                "{}_g{:04}_candidate_{label}",
                next_shot.shot_id, next_shot.generation_revision
            ))?;
            if next_artifacts
                .iter()
                .any(|record| record.artifact_id == artifact_id.as_str())
            {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
            next_artifacts.push(StoredArtifactRecord {
                artifact_id: artifact_id.as_str().to_owned(),
                schema_version: ARTIFACT_SCHEMA_VERSION,
                artifact_type: "shot_candidate".to_owned(),
                status: StoredArtifactStatus::Candidate,
                content_hash: format!("demo:{}", artifact_id.as_str()),
                input_hash: Some(input_hash.as_str().to_owned()),
                dependencies: Vec::new(),
                extra: BTreeMap::new(),
            });
            candidate_artifact_ids.push(artifact_id.as_str().to_owned());
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
        Ok(())
    }

    pub fn select(
        &mut self,
        project: &CanonicalProject,
        candidate_index: usize,
    ) -> Result<(), ShotWorkflowError> {
        let snapshot = self.snapshot()?;
        if snapshot.locked {
            return Err(ShotWorkflowError::Locked);
        }
        if !snapshot.generated {
            return Err(ShotWorkflowError::NotGenerated);
        }
        project
            .manifest()
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

        write_json(
            project,
            &shot_path(self.shot_number, "artifacts.json")?,
            &next_artifacts,
        )?;
        write_json(
            project,
            &shot_path(self.shot_number, "selection.json")?,
            &next_selection,
        )?;
        self.artifacts = next_artifacts;
        self.selection = next_selection;
        Ok(())
    }

    pub fn lock(&mut self, project: &CanonicalProject) -> Result<(), ShotWorkflowError> {
        let snapshot = self.snapshot()?;
        if snapshot.locked {
            return Err(ShotWorkflowError::AlreadyLocked);
        }
        if snapshot.stale {
            return Err(ShotWorkflowError::StaleSelection);
        }
        project
            .manifest()
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
            &shot_path(self.shot_number, "artifacts.json")?,
            &next_artifacts,
        )?;
        self.artifacts = next_artifacts;
        Ok(())
    }

    pub fn reset(&mut self, project: &CanonicalProject) -> Result<(), ShotWorkflowError> {
        project
            .manifest()
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let mut next_shot = self.shot.clone();
        next_shot.direction = ShotDirection::Reaction;
        let mut next_artifacts = self.artifacts.clone();
        for artifact_id in &self.selection.domain()?.candidate_artifact_ids {
            let artifact = find_artifact_mut(&mut next_artifacts, artifact_id)
                .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
            if artifact.status != StoredArtifactStatus::Superseded {
                artifact.transition(ArtifactStatus::Superseded)?;
            }
        }
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

    fn persist(
        &self,
        project: &CanonicalProject,
        shot: &ShotProductionManifest,
        artifacts: &[StoredArtifactRecord],
        selection: &StoredSelectionManifest,
    ) -> Result<(), ShotWorkflowError> {
        write_json(
            project,
            &shot_path(self.shot_number, "artifacts.json")?,
            artifacts,
        )?;
        write_json(
            project,
            &shot_path(self.shot_number, "selection.json")?,
            selection,
        )?;
        write_json(project, &shot_path(self.shot_number, "shot.json")?, shot)
    }

    fn validate(&self) -> Result<(), ShotWorkflowError> {
        let selection = self.selection.domain()?;
        let mut artifact_ids = BTreeSet::new();
        for artifact in &self.artifacts {
            let domain = artifact.domain()?;
            if !artifact_ids.insert(domain.artifact_id.clone()) {
                return Err(ShotWorkflowError::InvalidCanonicalState);
            }
        }
        for candidate in &selection.candidate_artifact_ids {
            let artifact = self
                .artifact(candidate)
                .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
            if artifact.status == StoredArtifactStatus::Superseded {
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
        Ok(())
    }

    fn artifact(&self, artifact_id: &ArtifactId) -> Option<&StoredArtifactRecord> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.artifact_id == artifact_id.as_str())
    }

    fn is_stale(&self) -> Result<bool, ShotWorkflowError> {
        let selection = self.selection.domain()?;
        if selection.candidate_artifact_ids.is_empty() {
            return Ok(false);
        }
        let current_input_hash = input_hash(&self.shot)?;
        Ok(selection.candidate_artifact_ids.iter().any(|candidate| {
            self.artifact(candidate).is_none_or(|artifact| {
                artifact.input_hash.as_deref() != Some(current_input_hash.as_str())
            })
        }))
    }
}

fn input_hash(shot: &ShotProductionManifest) -> Result<InputHash, ShotWorkflowError> {
    Ok(InputHash::new(format!(
        "kineto:shot-intent:v1:{}:{}",
        shot.shot_id,
        shot.direction.key()
    ))?)
}

fn find_artifact_mut<'a>(
    artifacts: &'a mut [StoredArtifactRecord],
    artifact_id: &ArtifactId,
) -> Option<&'a mut StoredArtifactRecord> {
    artifacts
        .iter_mut()
        .find(|artifact| artifact.artifact_id == artifact_id.as_str())
}

fn shot_path(shot_number: u32, file: &str) -> Result<ProjectRelativePath, ShotWorkflowError> {
    ProjectRelativePath::new(format!(
        "scenes/scene_001/shots/shot_{shot_number:03}/{file}"
    ))
    .map_err(ShotWorkflowError::Path)
}

fn read_json_optional<T: DeserializeOwned>(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
) -> Result<Option<T>, ShotWorkflowError> {
    match project.root().read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(ShotWorkflowError::Json),
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(ShotWorkflowError::Fs(error)),
    }
}

fn write_json<T: Serialize + ?Sized>(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
    value: &T,
) -> Result<(), ShotWorkflowError> {
    project
        .manifest()
        .validate_for_write()
        .map_err(ShotWorkflowError::Manifest)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(ShotWorkflowError::Json)?;
    bytes.push(b'\n');
    write_canonical(project.root(), path, &bytes)
}

fn write_canonical(
    root: &ProjectRoot,
    path: &ProjectRelativePath,
    bytes: &[u8],
) -> Result<(), ShotWorkflowError> {
    let resolved = root.resolve(path);
    let parent = resolved
        .parent()
        .ok_or_else(|| ShotWorkflowError::InvalidTarget(resolved.clone()))?;
    ensure_directory(root, parent)?;

    match fs::symlink_metadata(&resolved) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() {
                return Err(ShotWorkflowError::Fs(ProjectFsError::Symlink(resolved)));
            }
            if !metadata.is_file() {
                return Err(ShotWorkflowError::Fs(ProjectFsError::NotFile(resolved)));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ShotWorkflowError::Fs(ProjectFsError::Io(error))),
    }

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&resolved)
        .map_err(|error| ShotWorkflowError::Fs(ProjectFsError::Io(error)))?;
    file.write_all(bytes)
        .map_err(|error| ShotWorkflowError::Fs(ProjectFsError::Io(error)))?;
    file.sync_all()
        .map_err(|error| ShotWorkflowError::Fs(ProjectFsError::Io(error)))?;
    ProjectRoot::sync_directory(parent).map_err(ShotWorkflowError::Fs)
}

fn ensure_directory(root: &ProjectRoot, directory: &Path) -> Result<(), ShotWorkflowError> {
    let relative = directory
        .strip_prefix(root.root())
        .map_err(|_| ShotWorkflowError::Fs(ProjectFsError::EscapedRoot(directory.to_path_buf())))?;
    let mut current = root.root().to_path_buf();

    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(ShotWorkflowError::Fs(ProjectFsError::EscapedRoot(
                directory.to_path_buf(),
            )));
        };
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(ShotWorkflowError::Fs(ProjectFsError::Symlink(current)));
                }
                if !metadata.is_dir() {
                    return Err(ShotWorkflowError::Fs(ProjectFsError::NotDirectory(current)));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|error| ShotWorkflowError::Fs(ProjectFsError::Io(error)))?;
            }
            Err(error) => return Err(ShotWorkflowError::Fs(ProjectFsError::Io(error))),
        }
    }
    Ok(())
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

impl Error for ShotWorkflowError {}

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
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
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
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let reopened_shot = ShotWorkflow::load(&reopened, 1).unwrap();
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
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let base = target.join("scenes/scene_001/shots/shot_001");
        fs::create_dir_all(&base).unwrap();
        fs::write(
            base.join("shot.json"),
            br#"{"shot_id":"shot_001","schema_version":1,"generation_revision":1,"future_shot":"keep"}"#,
        )
        .unwrap();
        fs::write(
            base.join("artifacts.json"),
            br#"[{"artifact_id":"shot_001_g0001_candidate_a","schema_version":1,"type":"shot_candidate","status":"candidate","content_hash":"demo:a","input_hash":"kineto:shot-intent:v1:shot_001:reaction","dependencies":[],"future_artifact":7}]"#,
        )
        .unwrap();
        fs::write(
            base.join("selection.json"),
            br#"{"selected_artifact_id":null,"candidate_artifact_ids":["shot_001_g0001_candidate_a"],"future_selection":true}"#,
        )
        .unwrap();

        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
}
