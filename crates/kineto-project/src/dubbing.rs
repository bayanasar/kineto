//! Dubbing's canonical state: the takes of every line of a scene, and the
//! one locked for each.
//!
//! `scenes/<scene>/voice/lines.json` (`lines.schema.json`) records every
//! take of the scene's lines, as `line.<scene>.<NN>.<TTTT>` (line `NN` of
//! the scene, take `TTTT` of that line), its audio beside it. A take is a
//! `candidate`, the one `selected` for its line, or `rejected`; nothing is
//! deleted. Locking a line names its selected take in `locks`. Every change
//! is one atomic file write.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactId, ContentHash, InputHash,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError},
    media::MediaState,
    revisions::DependencyRecord,
    shot::PromptProvenance,
    story::is_portable_id,
    voice::VoiceSpec,
};

const SCHEMA_VERSION: u32 = 1;

/// The artifact ID prefix of a line take, as in `line.office.03.0002`.
pub const LINE_PREFIX: &str = "line.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TakeStatus {
    Candidate,
    /// The director's pick for its line, which locking makes the line's.
    Selected,
    Rejected,
}

/// A stretch of a take and when it is heard.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpanRecord {
    pub text: String,
    pub start_ms: u32,
    pub end_ms: u32,
}

/// One take of a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineTake {
    pub artifact_id: String,
    /// The line's place among the scene's lines, from 1.
    pub line: u32,
    /// Who says it.
    pub character: String,
    /// What it says, as the screenplay had it.
    pub text: String,
    /// The screenplay's direction for the line, as it had it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parenthetical: Option<String>,
    pub status: TakeStatus,
    /// The voice as it spoke the take, with its delivery and rate.
    pub voice: VoiceSpec,
    /// The director's note the take was made with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub content_hash: String,
    pub input_hash: String,
    pub provider: String,
    pub model: String,
    pub parameters: String,
    pub media_path: String,
    pub duration_ms: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sentences: Vec<SpanRecord>,
    pub provenance: PromptProvenance,
    pub dependencies: Vec<DependencyRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_state: Option<MediaState>,
}

/// The take a line is locked to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedLine {
    pub line: u32,
    pub artifact_id: String,
    pub content_hash: String,
    pub media_path: String,
    pub duration_ms: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct LineLog {
    schema_version: u32,
    scene_id: String,
    takes: Vec<LineTake>,
    #[serde(default)]
    locks: Vec<LockedLine>,
}

/// Whether attaching wrote anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeAttach {
    Attached,
    AlreadyAttached,
}

#[derive(Debug)]
pub enum DubbingError {
    InvalidLog,
    InvalidScene(String),
    NoSuchTake(String),
    /// A rejected take cannot be selected.
    Rejected(String),
    /// The locked take of a line cannot be rejected; lock another first.
    Locked(String),
    /// Locking a line needs a selected take of it.
    NoSelection(u32),
    /// The selected take was made for the line or voice as they were.
    StaleSelection(u32),
    /// A take no longer holds the bytes its record names.
    Mismatch(String),
    /// A take with this ID is already recorded with other content.
    Conflict(String),
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Manifest(ProjectManifestError),
    Json(serde_json::Error),
}

impl fmt::Display for DubbingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLog => formatter.write_str("the dubbing record is invalid"),
            Self::InvalidScene(id) => write!(formatter, "{id:?} is not a scene id"),
            Self::NoSuchTake(id) => write!(formatter, "there is no line take {id}"),
            Self::Rejected(id) => write!(formatter, "line take {id} was rejected"),
            Self::Locked(id) => write!(
                formatter,
                "line take {id} is locked; lock another take of its line before rejecting it"
            ),
            Self::NoSelection(line) => write!(formatter, "select a take of line {line} first"),
            Self::StaleSelection(line) => write!(
                formatter,
                "the selected take of line {line} was made for a line or voice that has since changed"
            ),
            Self::Mismatch(path) => write!(formatter, "{path} does not match its recorded hash"),
            Self::Conflict(id) => write!(
                formatter,
                "line take {id} is already recorded with other content"
            ),
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid dubbing JSON: {error}"),
        }
    }
}

impl Error for DubbingError {
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

fn checked(scene_id: &str) -> Result<&str, DubbingError> {
    if is_portable_id(scene_id) && scene_id.len() <= 64 {
        Ok(scene_id)
    } else {
        Err(DubbingError::InvalidScene(scene_id.to_owned()))
    }
}

fn log_path(scene_id: &str) -> Result<ProjectRelativePath, DubbingError> {
    ProjectRelativePath::new(format!("scenes/{}/voice/lines.json", checked(scene_id)?))
        .map_err(DubbingError::Path)
}

/// The directory a scene's takes are kept in.
pub fn media_dir(scene_id: &str) -> Result<String, DubbingError> {
    Ok(format!("scenes/{}/voice", checked(scene_id)?))
}

fn take_id(scene_id: &str, line: u32, take: usize) -> Result<ArtifactId, DubbingError> {
    ArtifactId::new(format!("{LINE_PREFIX}{scene_id}.{line:02}.{take:04}"))
        .map_err(|_| DubbingError::InvalidScene(scene_id.to_owned()))
}

/// The scene and line a take ID belongs to, as in `("office", 3)` for
/// `line.office.03.0002`.
#[must_use]
pub fn line_of(artifact_id: &str) -> Option<(&str, u32)> {
    let rest = artifact_id.strip_prefix(LINE_PREFIX)?;
    let (rest, take) = rest.rsplit_once('.')?;
    let (scene, line) = rest.rsplit_once('.')?;
    let digits = |text: &str, width: usize| {
        text.len() >= width && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    (digits(take, 4) && digits(line, 2))
        .then(|| line.parse().ok().map(|line| (scene, line)))
        .flatten()
}

fn load(project: &CanonicalProject, scene_id: &str) -> Result<LineLog, DubbingError> {
    let bytes = match project.root().read(&log_path(scene_id)?) {
        Ok(bytes) => bytes,
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LineLog {
                schema_version: SCHEMA_VERSION,
                scene_id: scene_id.to_owned(),
                takes: Vec::new(),
                locks: Vec::new(),
            });
        }
        Err(error) => return Err(DubbingError::Fs(error)),
    };
    let log: LineLog = serde_json::from_slice(&bytes).map_err(DubbingError::Json)?;
    let mut numbered = true;
    for (index, take) in log.takes.iter().enumerate() {
        let number = log.takes[..index]
            .iter()
            .filter(|earlier| earlier.line == take.line)
            .count()
            + 1;
        numbered &=
            take_id(scene_id, take.line, number).is_ok_and(|id| id.as_str() == take.artifact_id);
    }
    let one_selection = log.takes.iter().all(|take| {
        take.status != TakeStatus::Selected
            || log
                .takes
                .iter()
                .filter(|other| other.line == take.line && other.status == TakeStatus::Selected)
                .count()
                == 1
    });
    let locks_named = log.locks.iter().all(|lock| {
        log.takes
            .iter()
            .any(|take| take.artifact_id == lock.artifact_id && take.line == lock.line)
    });
    if log.schema_version == 0
        || log.scene_id != scene_id
        || !numbered
        || !one_selection
        || !locks_named
    {
        return Err(DubbingError::InvalidLog);
    }
    for take in &log.takes {
        ContentHash::new(take.content_hash.clone()).map_err(|_| DubbingError::InvalidLog)?;
        InputHash::new(take.input_hash.clone()).map_err(|_| DubbingError::InvalidLog)?;
        ProjectRelativePath::new(&take.media_path).map_err(DubbingError::Path)?;
    }
    Ok(log)
}

fn save(project: &CanonicalProject, log: &LineLog) -> Result<(), DubbingError> {
    project
        .validate_for_write()
        .map_err(DubbingError::Manifest)?;
    let mut bytes = serde_json::to_vec_pretty(log).map_err(DubbingError::Json)?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(&log_path(&log.scene_id)?, &bytes)
        .map_err(DubbingError::Fs)
}

/// Every take of `scene_id`'s lines, in the order they were made.
pub fn takes(project: &CanonicalProject, scene_id: &str) -> Result<Vec<LineTake>, DubbingError> {
    load(project, scene_id).map(|log| log.takes)
}

/// Each line of `scene_id` that is locked, and its take.
pub fn locks(project: &CanonicalProject, scene_id: &str) -> Result<Vec<LockedLine>, DubbingError> {
    load(project, scene_id).map(|log| log.locks)
}

/// The artifact ID the next take of line `line` of `scene_id` gets.
pub fn next_artifact_id(
    project: &CanonicalProject,
    scene_id: &str,
    line: u32,
) -> Result<ArtifactId, DubbingError> {
    let made = load(project, scene_id)?
        .takes
        .iter()
        .filter(|take| take.line == line)
        .count();
    take_id(scene_id, line, made + 1)
}

fn check_media(project: &CanonicalProject, take: &LineTake) -> Result<(), DubbingError> {
    let media = ProjectRelativePath::new(&take.media_path).map_err(DubbingError::Path)?;
    let bytes = project.root().read(&media).map_err(DubbingError::Fs)?;
    if ContentHash::of(&bytes).as_str() == take.content_hash {
        Ok(())
    } else {
        Err(DubbingError::Mismatch(take.media_path.clone()))
    }
}

/// Record `take` of `scene_id`. Idempotent: a take already recorded exactly
/// as given changes nothing; one that differs is refused. It arrives as a
/// candidate, numbered right after its line's earlier takes, and its file
/// must already hold the bytes its record names.
pub fn attach(
    project: &CanonicalProject,
    scene_id: &str,
    take: LineTake,
) -> Result<TakeAttach, DubbingError> {
    project
        .validate_for_write()
        .map_err(DubbingError::Manifest)?;
    let mut log = load(project, scene_id)?;
    if let Some(existing) = log
        .takes
        .iter()
        .find(|existing| existing.artifact_id == take.artifact_id)
    {
        let mut expected = existing.clone();
        // The director may since have selected or rejected it.
        expected.status = take.status;
        return if expected == take {
            Ok(TakeAttach::AlreadyAttached)
        } else {
            Err(DubbingError::Conflict(take.artifact_id))
        };
    }
    let made = log
        .takes
        .iter()
        .filter(|earlier| earlier.line == take.line)
        .count();
    if take_id(scene_id, take.line, made + 1)?.as_str() != take.artifact_id
        || take.status != TakeStatus::Candidate
    {
        return Err(DubbingError::InvalidLog);
    }
    check_media(project, &take)?;
    log.takes.push(take);
    save(project, &log)?;
    Ok(TakeAttach::Attached)
}

fn change(
    project: &CanonicalProject,
    scene_id: &str,
    artifact_id: &str,
    apply: impl FnOnce(&mut LineLog, usize) -> Result<(), DubbingError>,
) -> Result<(), DubbingError> {
    let mut log = load(project, scene_id)?;
    let index = log
        .takes
        .iter()
        .position(|take| take.artifact_id == artifact_id)
        .ok_or_else(|| DubbingError::NoSuchTake(artifact_id.to_owned()))?;
    let before = log.clone();
    apply(&mut log, index)?;
    if log != before {
        save(project, &log)?;
    }
    Ok(())
}

/// Select take `artifact_id` for its line; a take of the line selected
/// before becomes a candidate again.
pub fn select(
    project: &CanonicalProject,
    scene_id: &str,
    artifact_id: &str,
) -> Result<(), DubbingError> {
    change(project, scene_id, artifact_id, |log, index| {
        if log.takes[index].status == TakeStatus::Rejected {
            return Err(DubbingError::Rejected(artifact_id.to_owned()));
        }
        let line = log.takes[index].line;
        for take in log.takes.iter_mut().filter(|take| take.line == line) {
            if take.status == TakeStatus::Selected {
                take.status = TakeStatus::Candidate;
            }
        }
        log.takes[index].status = TakeStatus::Selected;
        Ok(())
    })
}

/// Reject take `artifact_id`. It stays recorded, with its audio.
pub fn reject(
    project: &CanonicalProject,
    scene_id: &str,
    artifact_id: &str,
) -> Result<(), DubbingError> {
    change(project, scene_id, artifact_id, |log, index| {
        if log.locks.iter().any(|lock| lock.artifact_id == artifact_id) {
            return Err(DubbingError::Locked(artifact_id.to_owned()));
        }
        log.takes[index].status = TakeStatus::Rejected;
        Ok(())
    })
}

/// The selected take of line `line` of `scene_id`, if one is.
pub fn selected(
    project: &CanonicalProject,
    scene_id: &str,
    line: u32,
) -> Result<Option<LineTake>, DubbingError> {
    Ok(takes(project, scene_id)?
        .into_iter()
        .find(|take| take.line == line && take.status == TakeStatus::Selected))
}

/// Lock line `line` of `scene_id` to its selected take, whose audio must
/// still hold the bytes its record names. Whether the take still fits the
/// line and the voice is for the caller to decide first.
pub fn lock(
    project: &CanonicalProject,
    scene_id: &str,
    line: u32,
) -> Result<LockedLine, DubbingError> {
    let mut log = load(project, scene_id)?;
    let take = log
        .takes
        .iter()
        .find(|take| take.line == line && take.status == TakeStatus::Selected)
        .cloned()
        .ok_or(DubbingError::NoSelection(line))?;
    check_media(project, &take)?;
    let locked = LockedLine {
        line,
        artifact_id: take.artifact_id,
        content_hash: take.content_hash,
        media_path: take.media_path,
        duration_ms: take.duration_ms,
    };
    log.locks.retain(|lock| lock.line != line);
    log.locks.push(locked.clone());
    log.locks.sort_by_key(|lock| lock.line);
    save(project, &log)?;
    Ok(locked)
}

/// Mark the takes `artifact_ids` of `scene_id` as having let go of their
/// bytes. Their records stay, and the caller removes the files after.
pub fn evict(
    project: &CanonicalProject,
    scene_id: &str,
    artifact_ids: &[String],
) -> Result<(), DubbingError> {
    let mut log = load(project, scene_id)?;
    for artifact_id in artifact_ids {
        log.takes
            .iter_mut()
            .find(|take| &take.artifact_id == artifact_id)
            .ok_or_else(|| DubbingError::NoSuchTake(artifact_id.clone()))?
            .media_state = Some(MediaState::Evicted);
    }
    save(project, &log)
}

/// Every scene that has dubbing on disk, by ID.
pub fn scenes(project: &CanonicalProject) -> Result<Vec<String>, DubbingError> {
    let root = project
        .root()
        .resolve(&ProjectRelativePath::new("scenes").map_err(DubbingError::Path)?);
    let mut found = Vec::new();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
        Err(error) => return Err(DubbingError::Fs(ProjectFsError::Io(error))),
    };
    for entry in entries {
        let entry = entry.map_err(|error| DubbingError::Fs(ProjectFsError::Io(error)))?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if checked(&name).is_ok() && entry.path().join("voice/lines.json").is_file() {
            found.push(name);
        }
    }
    found.sort();
    Ok(found)
}
