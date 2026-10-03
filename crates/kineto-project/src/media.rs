//! The media the project's canonical records name: where each file is,
//! whose it is, the hash of its bytes, and whether its bytes are kept.
//!
//! Records are the authority. A file a record calls present but that is
//! gone, or whose bytes no longer hash as recorded, is reported, never
//! quietly rewritten. A record whose bytes were let go says so
//! (`media_state: evicted`) and keeps everything else it knows.
//!
//! Generated media is deduplicated by content: files whose bytes are the
//! same, checked by hashing both, become hard links to one another. Every
//! record keeps its own path, so each shot's, character's and style's
//! files stay beside it; a file system without hard links keeps the copies.

use std::{
    collections::BTreeMap,
    error::Error,
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactStatus, ContentHash, cast,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::CanonicalProject,
    shot::{ShotKey, ShotWorkflow, ShotWorkflowError},
    style,
};

/// Whether a record's media bytes are kept. A record without one predates
/// eviction and is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MediaState {
    #[default]
    Present,
    /// The bytes were let go; the record, its prompt and its provenance are
    /// kept.
    Evicted,
}

/// Who keeps a media file.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaOwner {
    Shot(ShotKey),
    Character(String),
    Style,
}

/// What a media file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaKind {
    Keyframe,
    Video,
    /// A portrait candidate or a reference of a character's pack.
    Cast,
    StyleSample,
}

/// Where the director stands on what a media file shows, as far as keeping
/// its bytes is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Standing {
    /// Locked as production truth.
    Locked,
    /// The director's current pick.
    Chosen,
    /// Made, and not yet decided on.
    Candidate,
    /// Replaced by later work, or turned down.
    Passed,
}

/// One media file a canonical record names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaRecord {
    pub owner: MediaOwner,
    pub artifact_id: String,
    pub kind: MediaKind,
    pub path: ProjectRelativePath,
    pub content_hash: ContentHash,
    pub state: MediaState,
    pub standing: Standing,
}

#[derive(Debug)]
pub enum MediaError {
    Shot(ShotWorkflowError),
    Cast(cast::CastError),
    Style(style::StyleError),
    Path(ProjectPathError),
    Fs(ProjectFsError),
}

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shot(error) => error.fmt(formatter),
            Self::Cast(error) => error.fmt(formatter),
            Self::Style(error) => error.fmt(formatter),
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
        }
    }
}

impl Error for MediaError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Shot(error) => Some(error),
            Self::Cast(error) => Some(error),
            Self::Style(error) => Some(error),
            Self::Path(error) => Some(error),
            Self::Fs(error) => Some(error),
        }
    }
}

impl From<ShotWorkflowError> for MediaError {
    fn from(error: ShotWorkflowError) -> Self {
        Self::Shot(error)
    }
}

/// Every media file the project's records name, shots first (by scene and
/// shot), then characters, then the style samples.
pub fn catalog(project: &CanonicalProject) -> Result<Vec<MediaRecord>, MediaError> {
    let mut records = Vec::new();
    for key in shots_on_disk(project)? {
        let shot = ShotWorkflow::load(project, &key)?;
        records.extend(shot.media()?.into_iter().map(|media| MediaRecord {
            owner: MediaOwner::Shot(key.clone()),
            ..media
        }));
    }
    for character in directories(project, "characters")? {
        let lock = cast::locked(project, &character).map_err(MediaError::Cast)?;
        let locked: Vec<String> = lock
            .iter()
            .flat_map(|lock| {
                std::iter::once(lock.portrait.artifact_id.clone()).chain(
                    lock.references
                        .iter()
                        .map(|image| image.artifact_id.clone()),
                )
            })
            .collect();
        for image in cast::images(project, &character).map_err(MediaError::Cast)? {
            let standing = if locked.contains(&image.artifact_id) {
                Standing::Locked
            } else {
                match image.status {
                    Some(cast::PortraitStatus::Selected) => Standing::Chosen,
                    Some(cast::PortraitStatus::Rejected) => Standing::Passed,
                    Some(cast::PortraitStatus::Candidate) | None => Standing::Candidate,
                }
            };
            records.push(MediaRecord {
                owner: MediaOwner::Character(character.clone()),
                artifact_id: image.artifact_id,
                kind: MediaKind::Cast,
                path: ProjectRelativePath::new(&image.media_path)
                    .map_err(|error| MediaError::Cast(cast::CastError::Path(error)))?,
                content_hash: ContentHash::new(image.content_hash)
                    .map_err(|_| MediaError::Cast(cast::CastError::InvalidLog))?,
                state: image.media_state.unwrap_or_default(),
                standing,
            });
        }
    }
    let locked_style = style::locked(project).map_err(MediaError::Style)?;
    for sample in style::samples(project).map_err(MediaError::Style)? {
        let standing = if locked_style
            .as_ref()
            .is_some_and(|lock| lock.artifact_id == sample.artifact_id)
        {
            Standing::Locked
        } else {
            match sample.status {
                style::SampleStatus::Selected => Standing::Chosen,
                style::SampleStatus::Rejected => Standing::Passed,
                style::SampleStatus::Candidate => Standing::Candidate,
            }
        };
        records.push(MediaRecord {
            owner: MediaOwner::Style,
            artifact_id: sample.artifact_id,
            kind: MediaKind::StyleSample,
            path: ProjectRelativePath::new(&sample.media_path)
                .map_err(|error| MediaError::Style(style::StyleError::Path(error)))?,
            content_hash: ContentHash::new(sample.content_hash)
                .map_err(|_| MediaError::Style(style::StyleError::InvalidLog))?,
            state: sample.media_state.unwrap_or_default(),
            standing,
        });
    }
    Ok(records)
}

/// What a check of the project's media found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaReport {
    /// Records that say their bytes are kept, whose file is gone.
    pub missing: Vec<MediaRecord>,
    /// Records whose file's bytes no longer hash as recorded. Found only
    /// when the bytes are read.
    pub mismatched: Vec<MediaRecord>,
}

impl MediaReport {
    #[must_use]
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.mismatched.is_empty()
    }
}

/// Check `records` against the files. Only whether each present file
/// exists, unless `read_bytes`, which also hashes every present file: slow
/// for long videos, so it is asked for, not done on every open.
pub fn check(
    project: &CanonicalProject,
    records: &[MediaRecord],
    read_bytes: bool,
) -> Result<MediaReport, MediaError> {
    let mut report = MediaReport::default();
    for record in records
        .iter()
        .filter(|record| record.state == MediaState::Present)
    {
        let file = project.root().resolve(&record.path);
        if !file.is_file() {
            report.missing.push(record.clone());
        } else if read_bytes {
            let bytes = project.root().read(&record.path).map_err(MediaError::Fs)?;
            if ContentHash::of(&bytes) != record.content_hash {
                report.mismatched.push(record.clone());
            }
        }
    }
    Ok(report)
}

/// What deduplication did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dedupe {
    /// Files made hard links to an identical one.
    pub linked: usize,
    /// Bytes no longer stored twice.
    pub bytes: u64,
}

/// Make every present file whose bytes are the same as another's a hard
/// link to it. Both files are hashed first, and a file whose bytes differ
/// from its record is left alone. Where hard links cannot be made, the
/// copies are kept.
pub fn dedupe(project: &CanonicalProject, records: &[MediaRecord]) -> Result<Dedupe, MediaError> {
    let mut by_content: BTreeMap<&str, Vec<&MediaRecord>> = BTreeMap::new();
    for record in records
        .iter()
        .filter(|record| record.state == MediaState::Present)
    {
        by_content
            .entry(record.content_hash.as_str())
            .or_default()
            .push(record);
    }
    let mut done = Dedupe::default();
    for group in by_content.values().filter(|group| group.len() > 1) {
        let files: Vec<PathBuf> = group
            .iter()
            .map(|record| project.root().resolve(&record.path))
            .collect();
        // The first file that holds the recorded bytes is kept; files
        // already linked to it are not read again.
        let Some(original) = files
            .iter()
            .find(|file| holds(file, &group[0].content_hash))
        else {
            continue;
        };
        for file in &files {
            if same_file(original, file) || !holds(file, &group[0].content_hash) {
                continue;
            }
            let size = std::fs::metadata(file).map_or(0, |metadata| metadata.len());
            if link_over(original, file) {
                done.linked += 1;
                done.bytes += size;
            }
        }
    }
    Ok(done)
}

/// Whether `file` exists and its bytes hash to `hash`.
fn holds(file: &Path, hash: &ContentHash) -> bool {
    std::fs::read(file).is_ok_and(|bytes| ContentHash::of(&bytes) == *hash)
}

/// Replace `copy` by a hard link to `original`, atomically: the link is
/// made beside it and renamed over it. False when the file system cannot.
fn link_over(original: &Path, copy: &Path) -> bool {
    let Some(name) = copy.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let staged = copy.with_file_name(format!(".{name}.link"));
    let _ = std::fs::remove_file(&staged);
    if std::fs::hard_link(original, &staged).is_err() {
        return false;
    }
    if std::fs::rename(&staged, copy).is_err() {
        let _ = std::fs::remove_file(&staged);
        return false;
    }
    true
}

/// Whether two paths are one file.
#[cfg(unix)]
fn same_file(left: &Path, right: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(left), std::fs::metadata(right)) {
        (Ok(left), Ok(right)) => left.dev() == right.dev() && left.ino() == right.ino(),
        _ => false,
    }
}

/// Whether two paths are one file. Without a portable file identity here,
/// the same canonical path is the only sure answer; linking an existing
/// link again is harmless.
#[cfg(not(unix))]
fn same_file(left: &Path, right: &Path) -> bool {
    std::fs::canonicalize(left).ok() == std::fs::canonicalize(right).ok()
}

/// Every shot with canonical files under `scenes/`, in scene and shot
/// order, whether or not it is in play.
fn shots_on_disk(project: &CanonicalProject) -> Result<Vec<ShotKey>, MediaError> {
    let mut keys = Vec::new();
    for scene in directories(project, "scenes")? {
        for shot in directories(project, &format!("scenes/{scene}/shots"))? {
            let has_shot = project
                .root()
                .resolve(
                    &ProjectRelativePath::new(format!("scenes/{scene}/shots/{shot}/shot.json"))
                        .map_err(MediaError::Path)?,
                )
                .is_file();
            if has_shot && ShotKey::is_id(&scene) && ShotKey::is_id(&shot) {
                keys.push(ShotKey::new(scene.clone(), shot)?);
            }
        }
    }
    Ok(keys)
}

/// The names of the directories in `relative`, sorted; none when it does
/// not exist.
fn directories(project: &CanonicalProject, relative: &str) -> Result<Vec<String>, MediaError> {
    let path = project
        .root()
        .resolve(&ProjectRelativePath::new(relative).map_err(MediaError::Path)?);
    let entries = match std::fs::read_dir(&path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(MediaError::Fs(ProjectFsError::Io(error))),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| MediaError::Fs(ProjectFsError::Io(error)))?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir())
            && let Some(name) = entry.file_name().to_str()
            && !name.starts_with('.')
        {
            names.push(name.to_owned());
        }
    }
    names.sort();
    Ok(names)
}

impl From<ArtifactStatus> for Standing {
    fn from(status: ArtifactStatus) -> Self {
        match status {
            ArtifactStatus::Locked => Self::Locked,
            ArtifactStatus::Selected => Self::Chosen,
            ArtifactStatus::Draft | ArtifactStatus::Candidate => Self::Candidate,
            ArtifactStatus::Superseded => Self::Passed,
        }
    }
}
