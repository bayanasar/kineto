pub mod fs;
pub mod manifest;
pub mod shot;

use std::{collections::BTreeMap, error::Error, fmt};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ArtifactId(String);

impl ArtifactId {
    pub fn new(value: impl Into<String>) -> Result<Self, ArtifactIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ArtifactIdError::Empty);
        }
        if value.len() > 128 {
            return Err(ArtifactIdError::TooLong);
        }
        if !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
        {
            return Err(ArtifactIdError::InvalidCharacter);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ArtifactId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactIdError {
    Empty,
    TooLong,
    InvalidCharacter,
}

impl fmt::Display for ArtifactIdError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Empty => "artifact id must not be empty",
            Self::TooLong => "artifact id must be at most 128 bytes",
            Self::InvalidCharacter => "artifact id contains a non-portable character",
        })
    }
}

impl Error for ArtifactIdError {}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ContentHash(String);

impl ContentHash {
    pub fn new(value: impl Into<String>) -> Result<Self, HashValueError> {
        let value = value.into();
        if value.is_empty() {
            return Err(HashValueError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InputHash(String);

impl InputHash {
    pub fn new(value: impl Into<String>) -> Result<Self, HashValueError> {
        let value = value.into();
        if value.is_empty() {
            return Err(HashValueError);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HashValueError;

impl fmt::Display for HashValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("hash value must not be empty")
    }
}

impl Error for HashValueError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArtifactStatus {
    Draft,
    Candidate,
    Selected,
    Locked,
    Superseded,
}

impl ArtifactStatus {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Draft, Self::Candidate)
                | (Self::Candidate, Self::Selected)
                | (Self::Candidate, Self::Superseded)
                | (Self::Selected, Self::Candidate)
                | (Self::Selected, Self::Locked)
                | (Self::Selected, Self::Superseded)
                | (Self::Locked, Self::Superseded)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencyImpact {
    Semantic,
    Cosmetic,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDependency {
    pub artifact_id: ArtifactId,
    pub content_hash: ContentHash,
    pub impact: DependencyImpact,
    pub field: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRecord {
    pub artifact_id: ArtifactId,
    pub status: ArtifactStatus,
    pub content_hash: ContentHash,
    pub input_hash: Option<InputHash>,
    pub dependencies: Vec<ArtifactDependency>,
}

impl ArtifactRecord {
    pub fn transition(&mut self, next: ArtifactStatus) -> Result<(), LifecycleError> {
        if !self.status.can_transition_to(next) {
            return Err(LifecycleError {
                current: self.status,
                requested: next,
            });
        }
        self.status = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LifecycleError {
    pub current: ArtifactStatus,
    pub requested: ArtifactStatus,
}

impl fmt::Display for LifecycleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid artifact lifecycle transition: {:?} -> {:?}",
            self.current, self.requested
        )
    }
}

impl Error for LifecycleError {}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectionManifest {
    pub selected_artifact_id: Option<ArtifactId>,
    pub candidate_artifact_ids: Vec<ArtifactId>,
}

impl SelectionManifest {
    pub fn select(&mut self, artifact_id: &ArtifactId) -> Result<(), SelectionError> {
        if !self.candidate_artifact_ids.contains(artifact_id) {
            return Err(SelectionError::UnknownCandidate(artifact_id.clone()));
        }
        self.selected_artifact_id = Some(artifact_id.clone());
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionError {
    UnknownCandidate(ArtifactId),
}

impl fmt::Display for SelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCandidate(id) => write!(formatter, "artifact {id} is not a candidate"),
        }
    }
}

impl Error for SelectionError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyChange {
    Missing {
        artifact_id: ArtifactId,
        field: Option<String>,
    },
    ContentChanged {
        artifact_id: ArtifactId,
        field: Option<String>,
        recorded: ContentHash,
        current: ContentHash,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Currentness {
    Current,
    Stale(Vec<DependencyChange>),
    Incompatible(Vec<DependencyChange>),
}

#[derive(Debug, Clone, Default)]
pub struct ArtifactIndex {
    content_hashes: BTreeMap<ArtifactId, ContentHash>,
}

impl ArtifactIndex {
    #[must_use]
    pub fn from_records<'a>(records: impl IntoIterator<Item = &'a ArtifactRecord>) -> Self {
        Self {
            content_hashes: records
                .into_iter()
                .map(|record| (record.artifact_id.clone(), record.content_hash.clone()))
                .collect(),
        }
    }

    #[must_use]
    pub fn currentness(&self, record: &ArtifactRecord) -> Currentness {
        let mut missing = Vec::new();
        let mut changed = Vec::new();

        for dependency in record
            .dependencies
            .iter()
            .filter(|dependency| dependency.impact == DependencyImpact::Semantic)
        {
            match self.content_hashes.get(&dependency.artifact_id) {
                None => missing.push(DependencyChange::Missing {
                    artifact_id: dependency.artifact_id.clone(),
                    field: dependency.field.clone(),
                }),
                Some(current) if current != &dependency.content_hash => {
                    changed.push(DependencyChange::ContentChanged {
                        artifact_id: dependency.artifact_id.clone(),
                        field: dependency.field.clone(),
                        recorded: dependency.content_hash.clone(),
                        current: current.clone(),
                    });
                }
                Some(_) => {}
            }
        }

        if !missing.is_empty() {
            missing.extend(changed);
            Currentness::Incompatible(missing)
        } else if changed.is_empty() {
            Currentness::Current
        } else {
            Currentness::Stale(changed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(value: &str) -> ArtifactId {
        ArtifactId::new(value).unwrap()
    }

    fn hash(value: &str) -> ContentHash {
        ContentHash::new(value).unwrap()
    }

    fn record(id_value: &str, content_hash: &str) -> ArtifactRecord {
        ArtifactRecord {
            artifact_id: id(id_value),
            status: ArtifactStatus::Candidate,
            content_hash: hash(content_hash),
            input_hash: None,
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn selection_is_metadata_and_keeps_candidate_identity_stable() {
        let first = id("cand_01J7X");
        let second = id("cand_01J7Y");
        let mut selection = SelectionManifest {
            selected_artifact_id: None,
            candidate_artifact_ids: vec![first.clone(), second.clone()],
        };

        selection.select(&second).unwrap();

        assert_eq!(selection.selected_artifact_id, Some(second));
        assert_eq!(
            selection.candidate_artifact_ids,
            vec![first, id("cand_01J7Y")]
        );
    }

    #[test]
    fn selected_candidate_can_return_to_candidate_before_lock() {
        let mut artifact = record("cand_a", "sha256:a");
        artifact.transition(ArtifactStatus::Selected).unwrap();
        artifact.transition(ArtifactStatus::Candidate).unwrap();
        assert_eq!(artifact.status, ArtifactStatus::Candidate);
    }

    #[test]
    fn unlocked_generated_artifacts_can_be_superseded() {
        let mut candidate = record("cand_a", "sha256:a");
        candidate.transition(ArtifactStatus::Superseded).unwrap();
        assert_eq!(candidate.status, ArtifactStatus::Superseded);

        let mut selected = record("cand_b", "sha256:b");
        selected.transition(ArtifactStatus::Selected).unwrap();
        selected.transition(ArtifactStatus::Superseded).unwrap();
        assert_eq!(selected.status, ArtifactStatus::Superseded);
    }

    #[test]
    fn changing_one_scene_only_stales_artifacts_with_that_semantic_edge() {
        let scene_one_old = record("scene_001", "sha256:scene-1-old");
        let scene_one_new = record("scene_001", "sha256:scene-1-new");
        let scene_two = record("scene_002", "sha256:scene-2");
        let shot_one = ArtifactRecord {
            artifact_id: id("shot_001"),
            status: ArtifactStatus::Candidate,
            content_hash: hash("sha256:shot-1"),
            input_hash: InputHash::new("sha256:input-1").ok(),
            dependencies: vec![ArtifactDependency {
                artifact_id: scene_one_old.artifact_id.clone(),
                content_hash: scene_one_old.content_hash.clone(),
                impact: DependencyImpact::Semantic,
                field: Some("action".to_owned()),
            }],
        };
        let shot_two = ArtifactRecord {
            artifact_id: id("shot_002"),
            status: ArtifactStatus::Candidate,
            content_hash: hash("sha256:shot-2"),
            input_hash: InputHash::new("sha256:input-2").ok(),
            dependencies: vec![ArtifactDependency {
                artifact_id: scene_two.artifact_id.clone(),
                content_hash: scene_two.content_hash.clone(),
                impact: DependencyImpact::Semantic,
                field: Some("dialogue".to_owned()),
            }],
        };
        let index = ArtifactIndex::from_records([&scene_one_new, &scene_two]);
        assert!(matches!(
            index.currentness(&shot_one),
            Currentness::Stale(_)
        ));
        assert_eq!(index.currentness(&shot_two), Currentness::Current);
    }

    #[test]
    fn cosmetic_dependency_changes_do_not_propagate_staleness() {
        let title_new = record("screenplay_title", "sha256:new-title");
        let shot = ArtifactRecord {
            artifact_id: id("shot_003"),
            status: ArtifactStatus::Candidate,
            content_hash: hash("sha256:shot-3"),
            input_hash: None,
            dependencies: vec![ArtifactDependency {
                artifact_id: title_new.artifact_id.clone(),
                content_hash: hash("sha256:old-title"),
                impact: DependencyImpact::Cosmetic,
                field: Some("display_title".to_owned()),
            }],
        };
        let index = ArtifactIndex::from_records([&title_new]);
        assert_eq!(index.currentness(&shot), Currentness::Current);
    }

    #[test]
    fn locked_artifact_cannot_be_mutated_backwards() {
        let mut artifact = record("character_alice_v2", "sha256:alice");
        artifact.status = ArtifactStatus::Locked;
        assert!(artifact.transition(ArtifactStatus::Selected).is_err());
        artifact.transition(ArtifactStatus::Superseded).unwrap();
        assert_eq!(artifact.status, ArtifactStatus::Superseded);
    }
}
