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
    DependencyImpact, HashValueError, InputHash, LifecycleError, SelectionError, SelectionManifest,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError, ProjectStoreError},
};

const SHOT_SCHEMA_VERSION: u32 = 1;
const ARTIFACT_SCHEMA_VERSION: u32 = 1;
const GENERATED_CANDIDATE_COUNT: usize = 3;
const LEGACY_HASH_BACKUP_FILE: &str = "artifacts.pre-sha256-migration.json";

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
    pub migration_pending: bool,
    pub invalid_hash_count: u16,
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
    shot_number: u32,
    shot: ShotProductionManifest,
    artifacts: Vec<StoredArtifactRecord>,
    selection: StoredSelectionManifest,
    hash_issues: Vec<ShotHashIssue>,
    hash_migration_records: Vec<ShotHashMigrationRecord>,
    legacy_artifacts_bytes: Option<Vec<u8>>,
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

        let artifacts_bytes = read_bytes_optional(project, &artifacts_file)?;
        let artifacts = artifacts_bytes
            .as_deref()
            .map(serde_json::from_slice)
            .transpose()
            .map_err(ShotWorkflowError::Json)?
            .unwrap_or_default();
        let selection = read_json_optional(project, &selection_file)?.unwrap_or_default();
        let mut workflow = Self {
            shot_number,
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
                .filter(|artifact| artifact.status == StoredArtifactStatus::Superseded)
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
        })
    }

    #[must_use]
    pub fn hash_issues(&self) -> &[ShotHashIssue] {
        &self.hash_issues
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
            .manifest()
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;

        let original = self
            .legacy_artifacts_bytes
            .as_ref()
            .ok_or(ShotWorkflowError::InvalidCanonicalState)?;
        let backup_path = shot_path(self.shot_number, LEGACY_HASH_BACKUP_FILE)?;
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

        let artifacts_path = shot_path(self.shot_number, "artifacts.json")?;
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
        self.ensure_mutation_allowed()?;
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
        supersede_active_shot_candidates(&mut next_artifacts)?;

        let input_hash = input_hash(&next_shot)?;
        let candidate_ids = candidate_ids(&next_shot.shot_id, next_shot.generation_revision)?;
        let mut candidate_artifact_ids = Vec::with_capacity(candidate_ids.len());
        for artifact_id in candidate_ids {
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
                content_hash: demo_content_hash(&artifact_id)?.as_str().to_owned(),
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
        self.ensure_mutation_allowed()?;
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

        // Artifact status is published before the selection pointer. If the process
        // stops between the two atomic writes, load() reconciles status back to
        // selection.json, which is the authority for the human selection.
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
        self.ensure_mutation_allowed()?;
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
        self.ensure_mutation_allowed()?;
        project
            .manifest()
            .validate_for_write()
            .map_err(ShotWorkflowError::Manifest)?;
        let mut next_shot = self.shot.clone();
        next_shot.direction = ShotDirection::Reaction;
        let mut next_artifacts = self.artifacts.clone();
        supersede_active_shot_candidates(&mut next_artifacts)?;
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
        // Every file is replaced atomically, but the three files are not an ACID
        // transaction. Publish the monotonic generation revision first, artifact
        // records second, and the selection pointer last. load() recognizes the
        // intermediate states and reconciles them so every completed file boundary
        // is forward-recoverable after a process or power failure.
        write_json(project, &shot_path(self.shot_number, "shot.json")?, shot)?;
        write_json(
            project,
            &shot_path(self.shot_number, "artifacts.json")?,
            artifacts,
        )?;
        write_json(
            project,
            &shot_path(self.shot_number, "selection.json")?,
            selection,
        )
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
                    && let Some(direction) = legacy_input_hash_direction(&shot_id, stored_input_hash)
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
            if matches!(
                artifact.status,
                StoredArtifactStatus::Selected | StoredArtifactStatus::Locked
            ) && selected_id != Some(artifact.artifact_id.as_str())
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
        .manifest()
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
    MigrationRequired,
    InvalidStoredHash {
        artifact_id: String,
        field: &'static str,
        value: String,
    },
    MigrationBackupConflict(String),
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
                write!(formatter, "legacy hash migration backup conflicts at {path}")
            }
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
            | Self::MigrationRequired
            | Self::InvalidStoredHash { .. }
            | Self::MigrationBackupConflict(_)
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

    #[test]
    fn canonical_shot_update_replaces_the_published_file() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let mut source_shot = ShotWorkflow::load(&source_project, 1).unwrap();
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
            let mut recovered = ShotWorkflow::load(&project, 1).unwrap();
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
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
        shot.generate(&project).unwrap();
        shot.select(&project, 0).unwrap();
        let selection_before = fs::read(shot_dir(&target).join("selection.json")).unwrap();
        shot.select(&project, 1).unwrap();
        let artifacts_after = fs::read(shot_dir(&target).join("artifacts.json")).unwrap();
        drop(project);

        fs::write(shot_dir(&target).join("selection.json"), selection_before).unwrap();
        fs::write(shot_dir(&target).join("artifacts.json"), artifacts_after).unwrap();

        let reopened = CanonicalProject::open(&target).unwrap();
        let mut recovered = ShotWorkflow::load(&reopened, 1).unwrap();
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
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let mut recovered = ShotWorkflow::load(&reopened, 1).unwrap();
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
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let mut normalized = ShotWorkflow::load(&reopened, 1).unwrap();
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
    fn unknown_noncanonical_hash_opens_degraded_and_reports_record() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let mut shot = ShotWorkflow::load(&project, 1).unwrap();
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
        let mut degraded = ShotWorkflow::load(&reopened, 1).unwrap();
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
}
