//! Voice casting's canonical state: how each character sounds.
//!
//! `characters/<id>/voice_auditions.json` (`voice-auditions.schema.json`)
//! records every audition made while casting a character's voice, as
//! `voice.<character>.NNNN` in order, its audio under `voice/`. Each is a
//! `candidate`, the one `selected`, or `rejected`. Nothing is deleted.
//!
//! `characters/<id>/voice.json` (`voice.schema.json`) is the lock: the
//! selected audition's provider, model and voice, its sample by stable
//! artifact ID and content hash, and what it was made from (the character's
//! bible entry and locked cast), so its staleness can be computed. Every
//! change is one atomic file write.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactDependency, ArtifactId, ContentHash, InputHash,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError},
    media::MediaState,
    revisions::DependencyRecord,
    story::is_portable_id,
};

const SCHEMA_VERSION: u32 = 1;

/// The artifact ID prefix of a voice audition, as in `voice.mira.0003`.
pub const VOICE_PREFIX: &str = "voice.";

/// How a voice speaks, as its provider is asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceSpec {
    /// The provider's ID of the voice.
    pub voice_id: String,
    /// ISO 639-1 code of the language it speaks.
    pub language: String,
    /// The delivery it was chosen with, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emotion: Option<String>,
    /// In thousandths of the provider's normal rate.
    pub speed_permille: u16,
    pub pitch_semitones: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditionStatus {
    Candidate,
    /// The director's pick, which locking makes the character's voice.
    Selected,
    Rejected,
}

/// One audition: a voice saying a line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Audition {
    pub artifact_id: String,
    pub status: AuditionStatus,
    pub voice: VoiceSpec,
    /// The voice's name as its provider lists it.
    pub voice_name: String,
    /// What it says.
    pub line: String,
    pub content_hash: String,
    pub input_hash: String,
    pub provider: String,
    pub model: String,
    /// The provider parameters it was asked with.
    pub parameters: String,
    pub media_path: String,
    pub duration_ms: u32,
    pub dependencies: Vec<DependencyRecord>,
    /// Whether its bytes are kept; `None` for a record made before eviction,
    /// which keeps them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_state: Option<MediaState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct AuditionLog {
    schema_version: u32,
    character_id: String,
    auditions: Vec<Audition>,
}

/// A character's locked voice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceLock {
    pub schema_version: u32,
    pub character_id: String,
    /// The audition locked.
    pub artifact_id: String,
    pub provider: String,
    pub model: String,
    pub voice: VoiceSpec,
    pub voice_name: String,
    /// The audition's audio, a sample of how the character sounds.
    pub content_hash: String,
    pub media_path: String,
    /// What the audition was made from.
    pub dependencies: Vec<DependencyRecord>,
}

/// `schemas/voice-recommendations.schema.json`: a model's answer, and the
/// revision that keeps it.
pub const RECOMMENDATIONS_SCHEMA: &str =
    include_str!("../../../schemas/voice-recommendations.schema.json");

/// A voice a model recommends for a character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecommendedVoice {
    pub voice_id: String,
    /// The voice's name as its provider lists it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub reason: String,
}

/// The voices recommended for one character, best first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterVoices {
    pub character_id: String,
    pub voices: Vec<RecommendedVoice>,
}

/// One revision of voice recommendations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VoiceRecommendations {
    pub characters: Vec<CharacterVoices>,
}

/// Whether attaching wrote anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceAttach {
    Attached,
    AlreadyAttached,
}

#[derive(Debug)]
pub enum VoiceError {
    InvalidLog,
    InvalidCharacter(String),
    NoSuchAudition(String),
    /// A rejected audition cannot be selected.
    Rejected(String),
    /// The locked audition cannot be rejected; lock another first.
    Locked(String),
    /// Locking needs a selected audition.
    NoSelection,
    /// The selected audition was made from a bible entry or cast that has
    /// since changed; audition again.
    StaleSelection,
    /// An audition no longer holds the bytes its record names.
    Mismatch(String),
    /// An audition with this ID is already recorded with other content.
    Conflict(String),
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Manifest(ProjectManifestError),
    Json(serde_json::Error),
}

impl fmt::Display for VoiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLog => formatter.write_str("the voice casting record is invalid"),
            Self::InvalidCharacter(id) => write!(formatter, "{id:?} is not a character id"),
            Self::NoSuchAudition(id) => write!(formatter, "there is no voice audition {id}"),
            Self::Rejected(id) => write!(formatter, "voice audition {id} was rejected"),
            Self::Locked(id) => write!(
                formatter,
                "voice audition {id} is the locked voice; lock another before rejecting it"
            ),
            Self::NoSelection => formatter.write_str("select a voice first"),
            Self::StaleSelection => formatter
                .write_str("the selected voice was cast for a character that has since changed"),
            Self::Mismatch(path) => write!(formatter, "{path} does not match its recorded hash"),
            Self::Conflict(id) => write!(
                formatter,
                "voice audition {id} is already recorded with other content"
            ),
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid voice casting JSON: {error}"),
        }
    }
}

impl Error for VoiceError {
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

/// Character IDs are bible entry IDs, at most 112 characters.
fn checked(character_id: &str) -> Result<&str, VoiceError> {
    if is_portable_id(character_id) && character_id.len() <= 112 {
        Ok(character_id)
    } else {
        Err(VoiceError::InvalidCharacter(character_id.to_owned()))
    }
}

fn file(character_id: &str, name: &str) -> Result<ProjectRelativePath, VoiceError> {
    ProjectRelativePath::new(format!("characters/{}/{name}", checked(character_id)?))
        .map_err(VoiceError::Path)
}

fn read_optional(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
) -> Result<Option<Vec<u8>>, VoiceError> {
    match project.root().read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(VoiceError::Fs(error)),
    }
}

fn write_json<T: Serialize>(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
    value: &T,
) -> Result<(), VoiceError> {
    project.validate_for_write().map_err(VoiceError::Manifest)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(VoiceError::Json)?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(path, &bytes)
        .map_err(VoiceError::Fs)
}

/// The artifact ID of the `number`th audition of `character_id`.
fn audition_id(character_id: &str, number: usize) -> Result<ArtifactId, VoiceError> {
    ArtifactId::new(format!("{VOICE_PREFIX}{character_id}.{number:04}"))
        .map_err(|_| VoiceError::InvalidCharacter(character_id.to_owned()))
}

fn load(project: &CanonicalProject, character_id: &str) -> Result<AuditionLog, VoiceError> {
    let Some(bytes) = read_optional(project, &file(character_id, "voice_auditions.json")?)? else {
        return Ok(AuditionLog {
            schema_version: SCHEMA_VERSION,
            character_id: character_id.to_owned(),
            auditions: Vec::new(),
        });
    };
    let log: AuditionLog = serde_json::from_slice(&bytes).map_err(VoiceError::Json)?;
    let numbered = log.auditions.iter().enumerate().all(|(index, audition)| {
        audition_id(character_id, index + 1).is_ok_and(|id| id.as_str() == audition.artifact_id)
    });
    let selected = log
        .auditions
        .iter()
        .filter(|audition| audition.status == AuditionStatus::Selected)
        .count();
    if log.schema_version == 0 || log.character_id != character_id || !numbered || selected > 1 {
        return Err(VoiceError::InvalidLog);
    }
    for audition in &log.auditions {
        ContentHash::new(audition.content_hash.clone()).map_err(|_| VoiceError::InvalidLog)?;
        InputHash::new(audition.input_hash.clone()).map_err(|_| VoiceError::InvalidLog)?;
        ProjectRelativePath::new(&audition.media_path).map_err(VoiceError::Path)?;
    }
    Ok(log)
}

/// The directory a character's auditions are kept in.
pub fn media_dir(character_id: &str) -> Result<String, VoiceError> {
    Ok(format!("characters/{}/voice", checked(character_id)?))
}

/// Every audition of `character_id`, oldest first.
pub fn auditions(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<Vec<Audition>, VoiceError> {
    load(project, character_id).map(|log| log.auditions)
}

/// The artifact ID the next audition of `character_id` gets.
pub fn next_artifact_id(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<ArtifactId, VoiceError> {
    audition_id(
        character_id,
        load(project, character_id)?.auditions.len() + 1,
    )
}

/// The character an audition ID belongs to, as in `mira` for
/// `voice.mira.0003`.
#[must_use]
pub fn character_of(artifact_id: &str) -> Option<&str> {
    let rest = artifact_id.strip_prefix(VOICE_PREFIX)?;
    let (character, number) = rest.rsplit_once('.')?;
    (number.len() == 4 && number.bytes().all(|byte| byte.is_ascii_digit())).then_some(character)
}

fn check_media(project: &CanonicalProject, audition: &Audition) -> Result<(), VoiceError> {
    let media = ProjectRelativePath::new(&audition.media_path).map_err(VoiceError::Path)?;
    let bytes = project.root().read(&media).map_err(VoiceError::Fs)?;
    if ContentHash::of(&bytes).as_str() == audition.content_hash {
        Ok(())
    } else {
        Err(VoiceError::Mismatch(audition.media_path.clone()))
    }
}

/// Record `audition` of `character_id`. Idempotent: an audition already
/// recorded exactly as given changes nothing; one that differs is refused.
/// It arrives as a candidate, numbered right after every earlier audition,
/// and its file must already hold the bytes its record names.
pub fn attach(
    project: &CanonicalProject,
    character_id: &str,
    audition: Audition,
) -> Result<VoiceAttach, VoiceError> {
    project.validate_for_write().map_err(VoiceError::Manifest)?;
    let mut log = load(project, character_id)?;
    if let Some(existing) = log
        .auditions
        .iter()
        .find(|existing| existing.artifact_id == audition.artifact_id)
    {
        let mut expected = existing.clone();
        // The director may since have selected or rejected it.
        expected.status = audition.status;
        return if expected == audition {
            Ok(VoiceAttach::AlreadyAttached)
        } else {
            Err(VoiceError::Conflict(audition.artifact_id))
        };
    }
    if audition_id(character_id, log.auditions.len() + 1)?.as_str() != audition.artifact_id
        || audition.status != AuditionStatus::Candidate
    {
        return Err(VoiceError::InvalidLog);
    }
    check_media(project, &audition)?;
    log.auditions.push(audition);
    write_json(project, &file(character_id, "voice_auditions.json")?, &log)?;
    Ok(VoiceAttach::Attached)
}

fn change(
    project: &CanonicalProject,
    character_id: &str,
    artifact_id: &str,
    apply: impl FnOnce(&mut AuditionLog, usize) -> Result<(), VoiceError>,
) -> Result<(), VoiceError> {
    let mut log = load(project, character_id)?;
    let index = log
        .auditions
        .iter()
        .position(|audition| audition.artifact_id == artifact_id)
        .ok_or_else(|| VoiceError::NoSuchAudition(artifact_id.to_owned()))?;
    let before = log.clone();
    apply(&mut log, index)?;
    if log != before {
        write_json(project, &file(character_id, "voice_auditions.json")?, &log)?;
    }
    Ok(())
}

/// Select audition `artifact_id`; a previously selected one becomes a
/// candidate again.
pub fn select(
    project: &CanonicalProject,
    character_id: &str,
    artifact_id: &str,
) -> Result<(), VoiceError> {
    change(project, character_id, artifact_id, |log, index| {
        if log.auditions[index].status == AuditionStatus::Rejected {
            return Err(VoiceError::Rejected(artifact_id.to_owned()));
        }
        for audition in &mut log.auditions {
            if audition.status == AuditionStatus::Selected {
                audition.status = AuditionStatus::Candidate;
            }
        }
        log.auditions[index].status = AuditionStatus::Selected;
        Ok(())
    })
}

/// Reject audition `artifact_id`. It stays recorded, with its audio.
pub fn reject(
    project: &CanonicalProject,
    character_id: &str,
    artifact_id: &str,
) -> Result<(), VoiceError> {
    if locked(project, character_id)?.is_some_and(|lock| lock.artifact_id == artifact_id) {
        return Err(VoiceError::Locked(artifact_id.to_owned()));
    }
    change(project, character_id, artifact_id, |log, index| {
        log.auditions[index].status = AuditionStatus::Rejected;
        Ok(())
    })
}

/// The selected audition of `character_id`, if one is.
pub fn selected(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<Option<Audition>, VoiceError> {
    Ok(auditions(project, character_id)?
        .into_iter()
        .find(|audition| audition.status == AuditionStatus::Selected))
}

/// Mark the auditions `artifact_ids` of `character_id` as having let go of
/// their bytes. Their records stay, and the caller removes the files after.
pub fn evict(
    project: &CanonicalProject,
    character_id: &str,
    artifact_ids: &[String],
) -> Result<(), VoiceError> {
    project.validate_for_write().map_err(VoiceError::Manifest)?;
    let mut log = load(project, character_id)?;
    for artifact_id in artifact_ids {
        log.auditions
            .iter_mut()
            .find(|audition| &audition.artifact_id == artifact_id)
            .ok_or_else(|| VoiceError::NoSuchAudition(artifact_id.clone()))?
            .media_state = Some(MediaState::Evicted);
    }
    write_json(project, &file(character_id, "voice_auditions.json")?, &log)
}

/// Lock `character_id`'s selected audition as its voice. Its audio must
/// still hold the bytes its record names.
pub fn lock(project: &CanonicalProject, character_id: &str) -> Result<VoiceLock, VoiceError> {
    let audition = selected(project, character_id)?.ok_or(VoiceError::NoSelection)?;
    check_media(project, &audition)?;
    let lock = VoiceLock {
        schema_version: SCHEMA_VERSION,
        character_id: character_id.to_owned(),
        artifact_id: audition.artifact_id,
        provider: audition.provider,
        model: audition.model,
        voice: audition.voice,
        voice_name: audition.voice_name,
        content_hash: audition.content_hash,
        media_path: audition.media_path,
        dependencies: audition.dependencies,
    };
    write_json(project, &file(character_id, "voice.json")?, &lock)?;
    Ok(lock)
}

/// The locked voice of `character_id`, if it has one.
pub fn locked(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<Option<VoiceLock>, VoiceError> {
    let Some(bytes) = read_optional(project, &file(character_id, "voice.json")?)? else {
        return Ok(None);
    };
    let lock: VoiceLock = serde_json::from_slice(&bytes).map_err(VoiceError::Json)?;
    if lock.schema_version == 0 || lock.character_id != character_id {
        return Err(VoiceError::InvalidLog);
    }
    ContentHash::new(lock.content_hash.clone()).map_err(|_| VoiceError::InvalidLog)?;
    Ok(Some(lock))
}

/// The dependency work spoken in a character's voice records on its lock:
/// the locked audition, and the hash of the lock, which changes with the
/// voice, its provider and its settings.
#[must_use]
pub fn lock_dependency(lock: &VoiceLock) -> Option<ArtifactDependency> {
    Some(ArtifactDependency {
        artifact_id: ArtifactId::new(lock.artifact_id.clone()).ok()?,
        content_hash: ContentHash::of(&serde_json::to_vec(lock).ok()?),
        impact: crate::DependencyImpact::Semantic,
        field: Some(format!("voice.{}", lock.character_id)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource, ProjectWorkflow,
    };
    use std::{
        collections::BTreeMap,
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::{SystemTime, UNIX_EPOCH},
    };

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            Self(std::env::temp_dir().join(format!(
                "kineto-voice-test-{}-{nonce}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            )))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn project(target: &std::path::Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "voice_test".to_owned(),
                title: "Voice Test".to_owned(),
                created_at: "2026-10-03T00:00:00Z".to_owned(),
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

    /// An audition of `mira` by `voice_id`, its audio written.
    fn audition(project: &CanonicalProject, number: usize, voice_id: &str) -> Audition {
        let artifact_id = format!("voice.mira.{number:04}");
        let media_path = format!("characters/mira/voice/{artifact_id}.wav");
        let audio = format!("audio of {voice_id}").into_bytes();
        project
            .root()
            .write_atomic(&ProjectRelativePath::new(&media_path).unwrap(), &audio)
            .unwrap();
        Audition {
            artifact_id,
            status: AuditionStatus::Candidate,
            voice: VoiceSpec {
                voice_id: voice_id.to_owned(),
                language: "en".to_owned(),
                emotion: Some("calm".to_owned()),
                speed_permille: 1000,
                pitch_semitones: 0,
            },
            voice_name: voice_id.to_uppercase(),
            line: "I waited for you.".to_owned(),
            content_hash: ContentHash::of(&audio).as_str().to_owned(),
            input_hash: ContentHash::of(voice_id.as_bytes()).as_str().to_owned(),
            provider: "local-demo-speech".to_owned(),
            model: "demo-tone".to_owned(),
            parameters: format!("voice={voice_id}"),
            media_path,
            duration_ms: 1520,
            dependencies: vec![DependencyRecord {
                artifact_id: "cast.mira.0001".to_owned(),
                content_hash: ContentHash::of(b"cast").as_str().to_owned(),
                impact: crate::DependencyImpact::Semantic,
                field: Some("character.mira".to_owned()),
            }],
            media_state: None,
        }
    }

    #[test]
    fn auditions_are_numbered_chosen_and_locked_as_the_characters_voice() {
        let dir = TempDir::new();
        let project = project(&dir.0);
        assert!(auditions(&project, "mira").unwrap().is_empty());
        assert_eq!(
            next_artifact_id(&project, "mira").unwrap().as_str(),
            "voice.mira.0001"
        );
        let first = audition(&project, 1, "low");
        assert_eq!(
            attach(&project, "mira", first.clone()).unwrap(),
            VoiceAttach::Attached
        );
        assert_eq!(
            attach(&project, "mira", first.clone()).unwrap(),
            VoiceAttach::AlreadyAttached
        );
        // Out of order, or as another recording of the same ID, is refused.
        assert!(matches!(
            attach(&project, "mira", audition(&project, 3, "high")),
            Err(VoiceError::InvalidLog)
        ));
        let mut changed = first.clone();
        changed.line = "Something else.".to_owned();
        assert!(matches!(
            attach(&project, "mira", changed),
            Err(VoiceError::Conflict(_))
        ));
        attach(&project, "mira", audition(&project, 2, "high")).unwrap();

        assert!(matches!(
            lock(&project, "mira"),
            Err(VoiceError::NoSelection)
        ));
        select(&project, "mira", "voice.mira.0002").unwrap();
        select(&project, "mira", "voice.mira.0001").unwrap();
        let statuses: Vec<_> = auditions(&project, "mira")
            .unwrap()
            .iter()
            .map(|audition| audition.status)
            .collect();
        assert_eq!(
            statuses,
            [AuditionStatus::Selected, AuditionStatus::Candidate]
        );
        reject(&project, "mira", "voice.mira.0002").unwrap();
        assert!(matches!(
            select(&project, "mira", "voice.mira.0002"),
            Err(VoiceError::Rejected(_))
        ));

        let voice = lock(&project, "mira").unwrap();
        assert_eq!(voice.artifact_id, "voice.mira.0001");
        assert_eq!(voice.voice.voice_id, "low");
        assert_eq!(voice.dependencies, first.dependencies);
        assert_eq!(locked(&project, "mira").unwrap(), Some(voice.clone()));
        assert!(matches!(
            reject(&project, "mira", "voice.mira.0001"),
            Err(VoiceError::Locked(_))
        ));
        let dependency = lock_dependency(&voice).unwrap();
        assert_eq!(dependency.artifact_id.as_str(), "voice.mira.0001");
        assert_eq!(dependency.field.as_deref(), Some("voice.mira"));
        assert_eq!(character_of("voice.mira.0001"), Some("mira"));
        assert_eq!(character_of("voice.mira.1"), None);
        assert_eq!(character_of("cast.mira.0001"), None);
        assert_eq!(media_dir("mira").unwrap(), "characters/mira/voice");
        assert!(media_dir("../mira").is_err());
    }

    #[test]
    fn a_voice_whose_audio_changed_is_not_locked_and_eviction_keeps_the_record() {
        let dir = TempDir::new();
        let project = project(&dir.0);
        let first = audition(&project, 1, "low");
        attach(&project, "mira", first.clone()).unwrap();
        select(&project, "mira", "voice.mira.0001").unwrap();
        project
            .root()
            .write_atomic(
                &ProjectRelativePath::new(&first.media_path).unwrap(),
                b"tampered",
            )
            .unwrap();
        assert!(matches!(
            lock(&project, "mira"),
            Err(VoiceError::Mismatch(_))
        ));

        evict(&project, "mira", &["voice.mira.0001".to_owned()]).unwrap();
        let kept = auditions(&project, "mira").unwrap();
        assert_eq!(kept[0].media_state, Some(MediaState::Evicted));
        assert_eq!(kept[0].line, first.line);
        assert!(matches!(
            evict(&project, "mira", &["voice.mira.0009".to_owned()]),
            Err(VoiceError::NoSuchAudition(_))
        ));
    }
}
