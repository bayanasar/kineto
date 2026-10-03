//! Revisions of a document artifact: the story analysis, the screenplay, the
//! world and character bible, the shot plan and the director's preferences.
//!
//! Each revision is a JSON file named by its stable artifact ID, beside a
//! `revisions.json` log that records every revision's status, provenance and
//! dependencies (R5). A new revision never overwrites an earlier one: it
//! supersedes the earlier candidates, and a locked revision stays locked
//! until another one is locked in its place.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    ArtifactDependency, ArtifactId, ArtifactIdError, ContentHash, DependencyImpact, HashValueError,
    InputHash,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError},
    shot::PromptProvenance,
};

const LOG_SCHEMA_VERSION: u32 = 1;

/// A kind of document artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentKind {
    StoryAnalysis,
    Screenplay,
    Bible,
    ShotPlan,
    Preferences,
}

impl DocumentKind {
    const fn directory(self) -> &'static str {
        match self {
            Self::StoryAnalysis => "story/analysis",
            Self::Screenplay => "screenplay",
            Self::Bible => "bible",
            Self::ShotPlan => "shot_plan",
            Self::Preferences => "preferences",
        }
    }

    /// The artifact ID prefix, for example `screenplay` in `screenplay_r0003`.
    #[must_use]
    pub const fn id_prefix(self) -> &'static str {
        match self {
            Self::StoryAnalysis => "story_analysis",
            Self::Screenplay => "screenplay",
            Self::Bible => "bible",
            Self::ShotPlan => "shot_plan",
            Self::Preferences => "preferences",
        }
    }

    fn log_path(self) -> ProjectRelativePath {
        ProjectRelativePath::new(format!("{}/revisions.json", self.directory()))
            .expect("static revision log path is valid")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStatus {
    Candidate,
    Locked,
    Superseded,
}

/// How a revision came about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// A model produced it.
    Generated,
    /// The director wrote it, starting from `based_on`.
    Edited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DependencyRecord {
    pub artifact_id: String,
    pub content_hash: String,
    pub impact: DependencyImpact,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl From<&ArtifactDependency> for DependencyRecord {
    fn from(dependency: &ArtifactDependency) -> Self {
        Self {
            artifact_id: dependency.artifact_id.as_str().to_owned(),
            content_hash: dependency.content_hash.as_str().to_owned(),
            impact: dependency.impact,
            field: dependency.field.clone(),
        }
    }
}

/// One revision, as the log records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionRecord {
    pub revision: u32,
    pub artifact_id: String,
    pub path: String,
    pub status: RevisionStatus,
    pub origin: Origin,
    pub content_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provenance: Option<PromptProvenance>,
    /// The revision an edit started from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub based_on: Option<u32>,
    pub dependencies: Vec<DependencyRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RevisionLog {
    schema_version: u32,
    kind: DocumentKind,
    revisions: Vec<RevisionRecord>,
}

/// The next revision of a document, decided before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NextRevision {
    pub revision: u32,
    pub artifact_id: ArtifactId,
    pub path: ProjectRelativePath,
}

/// Whether attaching wrote anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionAttach {
    Attached,
    AlreadyAttached,
}

#[derive(Debug)]
pub enum RevisionError {
    InvalidLog,
    /// A revision's file no longer holds the bytes its record names.
    Mismatch(String),
    /// A revision with this number already exists with other content.
    Conflict(u32),
    NoSuchRevision(u32),
    /// A superseded revision cannot be locked; it is history.
    Superseded(u32),
    /// An edit started from a revision that is no longer the latest; the
    /// director has to see the newer one first.
    NotLatest {
        based_on: u32,
        latest: u32,
    },
    /// The document breaks its own rules.
    InvalidDocument(String),
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Manifest(ProjectManifestError),
    Json(serde_json::Error),
}

impl fmt::Display for RevisionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLog => formatter.write_str("revision log is invalid"),
            Self::Mismatch(path) => write!(formatter, "{path} does not match its recorded hash"),
            Self::Conflict(revision) => {
                write!(
                    formatter,
                    "revision {revision} already exists with other content"
                )
            }
            Self::NoSuchRevision(revision) => write!(formatter, "there is no revision {revision}"),
            Self::Superseded(revision) => {
                write!(
                    formatter,
                    "revision {revision} is superseded and cannot be locked"
                )
            }
            Self::NotLatest { based_on, latest } => write!(
                formatter,
                "the edit started from revision {based_on}, but revision {latest} is newer"
            ),
            Self::InvalidDocument(reason) => write!(formatter, "the document is invalid: {reason}"),
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid revision JSON: {error}"),
        }
    }
}

impl Error for RevisionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Fs(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ArtifactIdError> for RevisionError {
    fn from(_: ArtifactIdError) -> Self {
        Self::InvalidLog
    }
}

impl From<HashValueError> for RevisionError {
    fn from(_: HashValueError) -> Self {
        Self::InvalidLog
    }
}

fn read_optional(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
) -> Result<Option<Vec<u8>>, RevisionError> {
    match project.root().read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RevisionError::Fs(error)),
    }
}

fn load(project: &CanonicalProject, kind: DocumentKind) -> Result<RevisionLog, RevisionError> {
    let Some(bytes) = read_optional(project, &kind.log_path())? else {
        return Ok(RevisionLog {
            schema_version: LOG_SCHEMA_VERSION,
            kind,
            revisions: Vec::new(),
        });
    };
    let log: RevisionLog = serde_json::from_slice(&bytes).map_err(RevisionError::Json)?;
    let numbered = log
        .revisions
        .iter()
        .enumerate()
        .all(|(index, record)| u32::try_from(index + 1) == Ok(record.revision));
    let locked = log
        .revisions
        .iter()
        .filter(|record| record.status == RevisionStatus::Locked)
        .count();
    if log.schema_version == 0 || log.kind != kind || !numbered || locked > 1 {
        return Err(RevisionError::InvalidLog);
    }
    for record in &log.revisions {
        ArtifactId::new(record.artifact_id.clone())?;
        ContentHash::new(record.content_hash.clone())?;
        if let Some(input_hash) = &record.input_hash {
            InputHash::new(input_hash.clone())?;
        }
    }
    Ok(log)
}

fn save(project: &CanonicalProject, log: &RevisionLog) -> Result<(), RevisionError> {
    let mut bytes = serde_json::to_vec_pretty(log).map_err(RevisionError::Json)?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(&log.kind.log_path(), &bytes)
        .map_err(RevisionError::Fs)
}

/// Every revision of `kind`, oldest first.
pub fn revisions(
    project: &CanonicalProject,
    kind: DocumentKind,
) -> Result<Vec<RevisionRecord>, RevisionError> {
    load(project, kind).map(|log| log.revisions)
}

/// The revision work builds on: the locked one if there is one, otherwise
/// the latest candidate.
pub fn current(
    project: &CanonicalProject,
    kind: DocumentKind,
) -> Result<Option<RevisionRecord>, RevisionError> {
    let revisions = revisions(project, kind)?;
    Ok(revisions
        .iter()
        .find(|record| record.status == RevisionStatus::Locked)
        .or_else(|| {
            revisions
                .iter()
                .rev()
                .find(|record| record.status == RevisionStatus::Candidate)
        })
        .cloned())
}

/// The newest revision still in play: the one the director is working on,
/// whether it is a candidate newer than the locked revision or the locked
/// revision itself.
pub fn latest(
    project: &CanonicalProject,
    kind: DocumentKind,
) -> Result<Option<RevisionRecord>, RevisionError> {
    Ok(revisions(project, kind)?
        .into_iter()
        .rev()
        .find(|record| record.status != RevisionStatus::Superseded))
}

/// The revision a new generation or edit of `kind` would create.
pub fn next(project: &CanonicalProject, kind: DocumentKind) -> Result<NextRevision, RevisionError> {
    let revision = u32::try_from(revisions(project, kind)?.len() + 1)
        .map_err(|_| RevisionError::InvalidLog)?;
    revision_at(kind, revision)
}

/// The artifact ID and path revision `revision` of `kind` has.
pub fn revision_at(kind: DocumentKind, revision: u32) -> Result<NextRevision, RevisionError> {
    let artifact_id = ArtifactId::new(format!("{}_r{revision:04}", kind.id_prefix()))?;
    let path = ProjectRelativePath::new(format!(
        "{}/{}.json",
        kind.directory(),
        artifact_id.as_str()
    ))
    .map_err(RevisionError::Path)?;
    Ok(NextRevision {
        revision,
        artifact_id,
        path,
    })
}

/// Parse `revision_id` as a revision of `kind`.
#[must_use]
pub fn parse_revision(kind: DocumentKind, artifact_id: &ArtifactId) -> Option<u32> {
    artifact_id
        .as_str()
        .strip_prefix(kind.id_prefix())?
        .strip_prefix("_r")
        .filter(|digits| digits.len() == 4 && digits.bytes().all(|byte| byte.is_ascii_digit()))?
        .parse()
        .ok()
        .filter(|revision| *revision != 0)
}

/// The bytes a document revision is stored as: pretty JSON with a final
/// newline, so its hash is reproducible.
pub fn encode<T: Serialize>(document: &T) -> Result<Vec<u8>, RevisionError> {
    let mut bytes = serde_json::to_vec_pretty(document).map_err(RevisionError::Json)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Read a revision's document, checked against its hash.
pub fn read<T: DeserializeOwned>(
    project: &CanonicalProject,
    record: &RevisionRecord,
) -> Result<T, RevisionError> {
    let path = ProjectRelativePath::new(&record.path).map_err(RevisionError::Path)?;
    let bytes = project.root().read(&path).map_err(RevisionError::Fs)?;
    if ContentHash::of(&bytes).as_str() != record.content_hash {
        return Err(RevisionError::Mismatch(record.path.clone()));
    }
    serde_json::from_slice(&bytes).map_err(RevisionError::Json)
}

/// Write a revision's document file. Its log record is added by [`attach`];
/// a file without a record is not a revision yet.
pub fn write_document(
    project: &CanonicalProject,
    next: &NextRevision,
    bytes: &[u8],
) -> Result<ContentHash, RevisionError> {
    project
        .validate_for_write()
        .map_err(RevisionError::Manifest)?;
    project
        .root()
        .write_atomic(&next.path, bytes)
        .map_err(RevisionError::Fs)?;
    Ok(ContentHash::of(bytes))
}

/// Add `record` to the log, superseding earlier candidates. Idempotent: a
/// record already in the log exactly as given changes nothing; one that
/// differs is refused.
pub fn attach(
    project: &CanonicalProject,
    kind: DocumentKind,
    record: RevisionRecord,
) -> Result<RevisionAttach, RevisionError> {
    project
        .validate_for_write()
        .map_err(RevisionError::Manifest)?;
    let mut log = load(project, kind)?;
    if let Some(existing) = log
        .revisions
        .iter()
        .find(|existing| existing.revision == record.revision)
    {
        let mut expected = existing.clone();
        // A later revision may since have superseded it.
        expected.status = record.status;
        return if expected == record {
            Ok(RevisionAttach::AlreadyAttached)
        } else {
            Err(RevisionError::Conflict(record.revision))
        };
    }
    let expected = revision_at(kind, record.revision)?;
    if usize::try_from(record.revision) != Ok(log.revisions.len() + 1)
        || record.artifact_id != expected.artifact_id.as_str()
        || record.path != expected.path.as_path().to_string_lossy().replace('\\', "/")
        || record.status != RevisionStatus::Candidate
    {
        return Err(RevisionError::InvalidLog);
    }
    let path = ProjectRelativePath::new(&record.path).map_err(RevisionError::Path)?;
    let bytes = project.root().read(&path).map_err(RevisionError::Fs)?;
    if ContentHash::of(&bytes).as_str() != record.content_hash {
        return Err(RevisionError::Mismatch(record.path));
    }
    for earlier in &mut log.revisions {
        if earlier.status == RevisionStatus::Candidate {
            earlier.status = RevisionStatus::Superseded;
        }
    }
    log.revisions.push(record);
    save(project, &log)?;
    Ok(RevisionAttach::Attached)
}

/// Save the director's edit of revision `based_on` of `kind`, encoded as
/// `bytes`, as the next revision. `based_on` must be the latest revision, so
/// an edit never silently replaces one the director has not seen. An edit
/// that changes nothing returns the revision it started from.
pub fn save_edit(
    project: &CanonicalProject,
    kind: DocumentKind,
    based_on: u32,
    bytes: &[u8],
) -> Result<RevisionRecord, RevisionError> {
    let latest = latest(project, kind)?.ok_or(RevisionError::NoSuchRevision(based_on))?;
    if latest.revision != based_on {
        return Err(RevisionError::NotLatest {
            based_on,
            latest: latest.revision,
        });
    }
    if ContentHash::of(bytes).as_str() == latest.content_hash {
        return Ok(latest);
    }
    let next = next(project, kind)?;
    let content_hash = write_document(project, &next, bytes)?;
    let own_revisions = format!("{}_r", kind.id_prefix());
    let record = RevisionRecord {
        revision: next.revision,
        artifact_id: next.artifact_id.as_str().to_owned(),
        path: next.path.as_path().to_string_lossy().replace('\\', "/"),
        status: RevisionStatus::Candidate,
        origin: Origin::Edited,
        content_hash: content_hash.as_str().to_owned(),
        input_hash: None,
        provider: None,
        model: None,
        provenance: None,
        based_on: Some(based_on),
        // Still made from what the edited revision was made from; `based_on`
        // records the lineage.
        dependencies: latest
            .dependencies
            .into_iter()
            .filter(|dependency| !dependency.artifact_id.starts_with(&own_revisions))
            .collect(),
    };
    attach(project, kind, record.clone())?;
    Ok(record)
}

/// Lock `revision` as production truth. A previously locked revision is
/// superseded; nothing is deleted.
pub fn lock(
    project: &CanonicalProject,
    kind: DocumentKind,
    revision: u32,
) -> Result<(), RevisionError> {
    project
        .validate_for_write()
        .map_err(RevisionError::Manifest)?;
    let mut log = load(project, kind)?;
    let target = log
        .revisions
        .iter()
        .find(|record| record.revision == revision)
        .ok_or(RevisionError::NoSuchRevision(revision))?;
    match target.status {
        RevisionStatus::Locked => return Ok(()),
        RevisionStatus::Superseded => return Err(RevisionError::Superseded(revision)),
        RevisionStatus::Candidate => {}
    }
    for record in &mut log.revisions {
        if record.revision == revision {
            record.status = RevisionStatus::Locked;
        } else if matches!(
            record.status,
            RevisionStatus::Locked | RevisionStatus::Candidate
        ) {
            record.status = RevisionStatus::Superseded;
        }
    }
    save(project, &log)
}

/// The dependency a revision records on this one.
pub fn dependency_on(
    record: &RevisionRecord,
    impact: DependencyImpact,
) -> Result<ArtifactDependency, RevisionError> {
    Ok(ArtifactDependency {
        artifact_id: ArtifactId::new(record.artifact_id.clone())?,
        content_hash: ContentHash::new(record.content_hash.clone())?,
        impact,
        field: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use std::{collections::BTreeMap, fs, path::PathBuf};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kineto-revisions-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn project(temp: &TempDir) -> CanonicalProject {
        CanonicalProject::create(
            temp.0.join("film"),
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "revisions_test".to_owned(),
                title: "Revisions Test".to_owned(),
                created_at: "2026-10-01T00:00:00Z".to_owned(),
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

    fn add(project: &CanonicalProject, value: &serde_json::Value) -> RevisionRecord {
        let next = next(project, DocumentKind::Screenplay).unwrap();
        let hash = write_document(project, &next, &encode(value).unwrap()).unwrap();
        let record = RevisionRecord {
            revision: next.revision,
            artifact_id: next.artifact_id.as_str().to_owned(),
            path: next.path.as_path().to_string_lossy().into_owned(),
            status: RevisionStatus::Candidate,
            origin: Origin::Edited,
            content_hash: hash.as_str().to_owned(),
            input_hash: None,
            provider: None,
            model: None,
            provenance: None,
            based_on: next
                .revision
                .checked_sub(1)
                .filter(|revision| *revision != 0),
            dependencies: Vec::new(),
        };
        assert_eq!(
            attach(project, DocumentKind::Screenplay, record.clone()).unwrap(),
            RevisionAttach::Attached
        );
        record
    }

    #[test]
    fn revisions_supersede_candidates_and_lock_one_at_a_time() {
        let temp = TempDir::new();
        let project = project(&temp);
        assert_eq!(current(&project, DocumentKind::Screenplay).unwrap(), None);
        let first = add(&project, &serde_json::json!({"draft": 1}));
        assert_eq!(first.artifact_id, "screenplay_r0001");
        assert_eq!(first.path, "screenplay/screenplay_r0001.json");
        let second = add(&project, &serde_json::json!({"draft": 2}));
        let all = revisions(&project, DocumentKind::Screenplay).unwrap();
        assert_eq!(all[0].status, RevisionStatus::Superseded);
        assert_eq!(
            current(&project, DocumentKind::Screenplay)
                .unwrap()
                .unwrap()
                .revision,
            2
        );

        lock(&project, DocumentKind::Screenplay, 2).unwrap();
        let third = add(&project, &serde_json::json!({"draft": 3}));
        // The lock holds while a newer candidate exists, and the director
        // works on that candidate.
        assert_eq!(
            current(&project, DocumentKind::Screenplay)
                .unwrap()
                .unwrap()
                .revision,
            2
        );
        assert_eq!(
            latest(&project, DocumentKind::Screenplay)
                .unwrap()
                .unwrap()
                .revision,
            3
        );
        lock(&project, DocumentKind::Screenplay, 3).unwrap();
        let all = revisions(&project, DocumentKind::Screenplay).unwrap();
        assert_eq!(
            all.iter().map(|record| record.status).collect::<Vec<_>>(),
            [
                RevisionStatus::Superseded,
                RevisionStatus::Superseded,
                RevisionStatus::Locked
            ]
        );
        assert!(matches!(
            lock(&project, DocumentKind::Screenplay, 1),
            Err(RevisionError::Superseded(1))
        ));
        let value: serde_json::Value = read(&project, &all[1]).unwrap();
        assert_eq!(value["draft"], 2, "every revision stays readable");
        let _ = (second, third);
    }

    #[test]
    fn attaching_is_idempotent_and_refuses_a_different_record() {
        let temp = TempDir::new();
        let project = project(&temp);
        let first = add(&project, &serde_json::json!({"draft": 1}));
        assert_eq!(
            attach(&project, DocumentKind::Screenplay, first.clone()).unwrap(),
            RevisionAttach::AlreadyAttached
        );
        let mut other = first.clone();
        other.provider = Some("someone-else".to_owned());
        assert!(matches!(
            attach(&project, DocumentKind::Screenplay, other),
            Err(RevisionError::Conflict(1))
        ));
        // A file edited after it was recorded is not read as that revision.
        fs::write(temp.0.join("film/screenplay/screenplay_r0001.json"), "{}\n").unwrap();
        assert!(matches!(
            read::<serde_json::Value>(&project, &first),
            Err(RevisionError::Mismatch(_))
        ));
        assert_eq!(
            parse_revision(
                DocumentKind::Screenplay,
                &ArtifactId::new("screenplay_r0012").unwrap()
            ),
            Some(12)
        );
        assert_eq!(
            parse_revision(
                DocumentKind::StoryAnalysis,
                &ArtifactId::new("screenplay_r0012").unwrap()
            ),
            None
        );
    }
}
