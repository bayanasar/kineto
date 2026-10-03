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
    ArtifactStatus, ContentHash, cast, dubbing,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::CanonicalProject,
    shot::{ShotKey, ShotWorkflow, ShotWorkflowError},
    style, voice,
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
    /// A scene's spoken lines.
    Scene(String),
}

/// What a media file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaKind {
    Keyframe,
    Video,
    /// A portrait candidate or a reference of a character's pack.
    Cast,
    StyleSample,
    /// An audition of a character's voice.
    Voice,
    /// A take of a line of dialogue.
    Line,
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
    /// The artifacts it was made from, by ID.
    pub dependencies: Vec<String>,
}

#[derive(Debug)]
pub enum MediaError {
    Shot(ShotWorkflowError),
    Cast(cast::CastError),
    Style(style::StyleError),
    Voice(voice::VoiceError),
    Dubbing(dubbing::DubbingError),
    Path(ProjectPathError),
    Fs(ProjectFsError),
}

impl fmt::Display for MediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Shot(error) => error.fmt(formatter),
            Self::Cast(error) => error.fmt(formatter),
            Self::Style(error) => error.fmt(formatter),
            Self::Voice(error) => error.fmt(formatter),
            Self::Dubbing(error) => error.fmt(formatter),
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
            Self::Voice(error) => Some(error),
            Self::Dubbing(error) => Some(error),
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
/// shot), then characters, then the style samples, then scenes' spoken
/// lines.
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
                dependencies: image
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.artifact_id.clone())
                    .collect(),
            });
        }
        let locked_voice = voice::locked(project, &character)
            .map_err(MediaError::Voice)?
            .map(|lock| lock.artifact_id);
        for audition in voice::auditions(project, &character).map_err(MediaError::Voice)? {
            let standing = if locked_voice.as_ref() == Some(&audition.artifact_id) {
                Standing::Locked
            } else {
                match audition.status {
                    voice::AuditionStatus::Selected => Standing::Chosen,
                    voice::AuditionStatus::Rejected => Standing::Passed,
                    voice::AuditionStatus::Candidate => Standing::Candidate,
                }
            };
            records.push(MediaRecord {
                owner: MediaOwner::Character(character.clone()),
                artifact_id: audition.artifact_id,
                kind: MediaKind::Voice,
                path: ProjectRelativePath::new(&audition.media_path)
                    .map_err(|error| MediaError::Voice(voice::VoiceError::Path(error)))?,
                content_hash: ContentHash::new(audition.content_hash)
                    .map_err(|_| MediaError::Voice(voice::VoiceError::InvalidLog))?,
                state: audition.media_state.unwrap_or_default(),
                standing,
                dependencies: audition
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.artifact_id.clone())
                    .collect(),
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
            dependencies: sample
                .dependencies
                .iter()
                .map(|dependency| dependency.artifact_id.clone())
                .collect(),
        });
    }
    for scene in dubbing::scenes(project).map_err(MediaError::Dubbing)? {
        let locked: Vec<String> = dubbing::locks(project, &scene)
            .map_err(MediaError::Dubbing)?
            .into_iter()
            .map(|lock| lock.artifact_id)
            .collect();
        for take in dubbing::takes(project, &scene).map_err(MediaError::Dubbing)? {
            let standing = if locked.contains(&take.artifact_id) {
                Standing::Locked
            } else {
                match take.status {
                    dubbing::TakeStatus::Selected => Standing::Chosen,
                    dubbing::TakeStatus::Rejected => Standing::Passed,
                    dubbing::TakeStatus::Candidate => Standing::Candidate,
                }
            };
            records.push(MediaRecord {
                owner: MediaOwner::Scene(scene.clone()),
                artifact_id: take.artifact_id,
                kind: MediaKind::Line,
                path: ProjectRelativePath::new(&take.media_path)
                    .map_err(|error| MediaError::Dubbing(dubbing::DubbingError::Path(error)))?,
                content_hash: ContentHash::new(take.content_hash)
                    .map_err(|_| MediaError::Dubbing(dubbing::DubbingError::InvalidLog))?,
                state: take.media_state.unwrap_or_default(),
                standing,
                dependencies: take
                    .dependencies
                    .iter()
                    .map(|dependency| dependency.artifact_id.clone())
                    .collect(),
            });
        }
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

/// How many of each owner's latest takes of one kind are kept by default,
/// whatever the director decided on them: two generations of keyframes.
pub const DEFAULT_RECENT: usize = 6;

/// How many of each owner's latest takes of one kind the project keeps:
/// `recent_takes` under `[workflow]` in `project.toml`, at least 1.
#[must_use]
pub fn recent_window(project: &CanonicalProject) -> usize {
    project
        .manifest()
        .workflow
        .extra
        .get("recent_takes")
        .and_then(|value| value.as_integer())
        .and_then(|value| usize::try_from(value).ok())
        .map_or(DEFAULT_RECENT, |value| value.max(1))
}

/// Whether a media file's bytes may be let go, computed from the records on
/// every read and never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RetentionClass {
    /// Locked, or required by something locked: never collected.
    Pinned,
    /// Among its owner's latest takes, or still undecided: kept.
    Recent,
    /// Its bytes were let go; its record, prompt and provenance are kept.
    Archived,
    /// Replaced or turned down, outside the recent takes, and needed by
    /// nothing locked: its bytes may be let go.
    Collectable,
}

/// A media file's retention class and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retention {
    pub record: MediaRecord,
    pub class: RetentionClass,
    pub reason: String,
}

/// The retention class of every record, with `recent` latest takes kept per
/// owner and kind.
#[must_use]
pub fn retention(records: &[MediaRecord], recent: usize) -> Vec<Retention> {
    // Locked work and everything it was made from, transitively.
    let by_id: BTreeMap<&str, &MediaRecord> = records
        .iter()
        .map(|record| (record.artifact_id.as_str(), record))
        .collect();
    let mut pinned: BTreeMap<&str, String> = BTreeMap::new();
    let mut stack: Vec<(&str, String)> = records
        .iter()
        .filter(|record| record.standing == Standing::Locked)
        .map(|record| (record.artifact_id.as_str(), "locked".to_owned()))
        .collect();
    while let Some((id, reason)) = stack.pop() {
        if pinned.contains_key(id) {
            continue;
        }
        if let Some(record) = by_id.get(id) {
            for parent in &record.dependencies {
                stack.push((parent.as_str(), format!("{id} was made from it")));
            }
        }
        pinned.insert(id, reason);
    }
    // The latest `recent` of each owner's takes of one kind, in the order
    // they were made.
    let mut seen: BTreeMap<(&MediaOwner, MediaKind), usize> = BTreeMap::new();
    let mut latest = vec![false; records.len()];
    for (index, record) in records.iter().enumerate().rev() {
        let count = seen.entry((&record.owner, record.kind)).or_default();
        latest[index] = *count < recent;
        *count += 1;
    }
    records
        .iter()
        .zip(latest)
        .map(|(record, latest)| {
            let (class, reason) = if let Some(reason) = pinned.get(record.artifact_id.as_str()) {
                (RetentionClass::Pinned, reason.clone())
            } else if record.state == MediaState::Evicted {
                (RetentionClass::Archived, "its bytes were let go".to_owned())
            } else if record.standing != Standing::Passed {
                (RetentionClass::Recent, "not decided on yet".to_owned())
            } else if latest {
                (
                    RetentionClass::Recent,
                    format!("among the latest {recent} takes"),
                )
            } else {
                (
                    RetentionClass::Collectable,
                    format!("replaced or turned down, and older than the latest {recent} takes"),
                )
            };
            Retention {
                record: record.clone(),
                class,
                reason,
            }
        })
        .collect()
}

/// How much of the disk the project's media takes, each file counted once
/// however many records name it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    pub bytes: u64,
    /// By owner, in owner order; a file two owners share counts for both.
    pub by_owner: Vec<(MediaOwner, u64)>,
}

/// The disk the present media of `records` takes.
#[must_use]
pub fn usage(project: &CanonicalProject, records: &[MediaRecord]) -> Usage {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut by_owner: BTreeMap<MediaOwner, (Vec<PathBuf>, u64)> = BTreeMap::new();
    let mut bytes = 0;
    for record in records
        .iter()
        .filter(|record| record.state == MediaState::Present)
    {
        let file = project.root().resolve(&record.path);
        let Ok(metadata) = std::fs::metadata(&file) else {
            continue;
        };
        if !seen.iter().any(|other| same_file(other, &file)) {
            bytes += metadata.len();
            seen.push(file.clone());
        }
        let (files, owned) = by_owner.entry(record.owner.clone()).or_default();
        if !files.iter().any(|other| same_file(other, &file)) {
            *owned += metadata.len();
            files.push(file);
        }
    }
    Usage {
        bytes,
        by_owner: by_owner
            .into_iter()
            .map(|(owner, (_, owned))| (owner, owned))
            .collect(),
    }
}

/// What collecting would let go, worked out without touching anything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collection {
    /// Every collectable file, in catalog order.
    pub items: Vec<Retention>,
    /// The bytes freed: a file is freed only when every record naming it
    /// is collected.
    pub bytes: u64,
}

/// A dry run: what collecting `records` would let go, with `recent` takes
/// kept per owner and kind.
#[must_use]
pub fn plan(project: &CanonicalProject, records: &[MediaRecord], recent: usize) -> Collection {
    let classes = retention(records, recent);
    let items: Vec<Retention> = classes
        .iter()
        .filter(|retention| retention.class == RetentionClass::Collectable)
        .cloned()
        .collect();
    // A file is freed when no record that keeps its bytes names it.
    let kept: Vec<PathBuf> = classes
        .iter()
        .filter(|retention| {
            retention.class != RetentionClass::Collectable
                && retention.record.state == MediaState::Present
        })
        .map(|retention| project.root().resolve(&retention.record.path))
        .collect();
    let mut freed: Vec<PathBuf> = Vec::new();
    let mut bytes = 0;
    for item in &items {
        let file = project.root().resolve(&item.record.path);
        let Ok(metadata) = std::fs::metadata(&file) else {
            continue;
        };
        if kept.iter().any(|other| same_file(other, &file))
            || freed.iter().any(|other| same_file(other, &file))
        {
            continue;
        }
        bytes += metadata.len();
        freed.push(file);
    }
    Collection { items, bytes }
}

/// What a collection did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collected {
    /// The records marked evicted and whose files were removed, by ID.
    pub evicted: Vec<String>,
    /// Planned records that are no longer collectable, left as they are.
    pub skipped: Vec<String>,
}

/// Let go of the bytes `approved` lists, as a dry run showed them, and of
/// nothing else: each item is collected only if it is still collectable,
/// the same file with the same bytes. Each owner's records are marked
/// evicted first and the files removed after, so an interruption leaves at
/// worst a record that says evicted beside a file still there, which
/// [`reconcile`] removes.
pub fn collect(
    project: &CanonicalProject,
    approved: &Collection,
    recent: usize,
) -> Result<Collected, MediaError> {
    let current = retention(&catalog(project)?, recent);
    let mut done = Collected::default();
    let mut by_owner: BTreeMap<MediaOwner, Vec<MediaRecord>> = BTreeMap::new();
    for item in &approved.items {
        let still = current.iter().any(|now| {
            now.class == RetentionClass::Collectable
                && now.record.artifact_id == item.record.artifact_id
                && now.record.path == item.record.path
                && now.record.content_hash == item.record.content_hash
        });
        if still {
            by_owner
                .entry(item.record.owner.clone())
                .or_default()
                .push(item.record.clone());
        } else {
            done.skipped.push(item.record.artifact_id.clone());
        }
    }
    for (owner, records) in by_owner {
        let ids: Vec<String> = records
            .iter()
            .map(|record| record.artifact_id.clone())
            .collect();
        match &owner {
            MediaOwner::Shot(key) => {
                ShotWorkflow::load(project, key)?.evict_media(project, &ids)?
            }
            MediaOwner::Character(character) => {
                let (voices, images): (Vec<&MediaRecord>, Vec<&MediaRecord>) = records
                    .iter()
                    .partition(|record| record.kind == MediaKind::Voice);
                let ids_of = |records: Vec<&MediaRecord>| -> Vec<String> {
                    records
                        .into_iter()
                        .map(|record| record.artifact_id.clone())
                        .collect()
                };
                let (voices, images) = (ids_of(voices), ids_of(images));
                if !images.is_empty() {
                    cast::evict(project, character, &images).map_err(MediaError::Cast)?;
                }
                if !voices.is_empty() {
                    voice::evict(project, character, &voices).map_err(MediaError::Voice)?;
                }
            }
            MediaOwner::Style => style::evict(project, &ids).map_err(MediaError::Style)?,
            MediaOwner::Scene(scene) => {
                dubbing::evict(project, scene, &ids).map_err(MediaError::Dubbing)?;
            }
        }
        for record in records {
            remove(project, &record)?;
            done.evicted.push(record.artifact_id);
        }
    }
    Ok(done)
}

/// Remove the files of evicted records that are still there, as an
/// interrupted collection leaves them. Returns how many were removed.
pub fn reconcile(project: &CanonicalProject, records: &[MediaRecord]) -> Result<usize, MediaError> {
    let mut removed = 0;
    for record in records
        .iter()
        .filter(|record| record.state == MediaState::Evicted)
    {
        if project.root().resolve(&record.path).is_file() {
            remove(project, record)?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn remove(project: &CanonicalProject, record: &MediaRecord) -> Result<(), MediaError> {
    match std::fs::remove_file(project.root().resolve(&record.path)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(MediaError::Fs(ProjectFsError::Io(error))),
    }
}
