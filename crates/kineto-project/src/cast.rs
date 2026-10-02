//! Casting's canonical state: how each character looks.
//!
//! `characters/<id>/casting.json` (`casting.schema.json`) records every
//! image made while casting a character. Portrait candidates (under
//! `candidates/`) are a `candidate`, the one `selected`, or `rejected`; the
//! reference pack (under `refs/`) is made from the selected portrait, and
//! each reference names the portrait it shows as its identity reference.
//! Nothing is deleted.
//!
//! `characters/<id>/character.json` (`character.schema.json`) is the lock:
//! the canonical portrait and the reference pack by stable artifact IDs and
//! content hashes, how the provider takes the identity reference, the
//! forbidden changes the bible names, and what the cast was made from (the
//! character's bible entry and the locked style), so its staleness can be
//! computed. Every change is one atomic file write.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactDependency, ArtifactId, ContentHash, InputHash,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError},
    revisions::DependencyRecord,
    shot::PromptProvenance,
    story::is_portable_id,
};

const SCHEMA_VERSION: u32 = 1;

/// The artifact ID prefix of a casting image, as in `cast.mira.0003`.
pub const CAST_PREFIX: &str = "cast.";

/// What a casting image shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CastImageKind {
    /// A portrait candidate; the selected one is the canonical portrait.
    Portrait,
    Front,
    ProfileLeft,
    ProfileRight,
    FullBody,
    Costume,
    ExpressionSmile,
    ExpressionTense,
}

impl CastImageKind {
    /// Every kind a reference pack can hold, in the order it is made.
    pub const PACK: [Self; 7] = [
        Self::Front,
        Self::ProfileLeft,
        Self::ProfileRight,
        Self::FullBody,
        Self::Costume,
        Self::ExpressionSmile,
        Self::ExpressionTense,
    ];

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Portrait => "portrait",
            Self::Front => "front",
            Self::ProfileLeft => "profile_left",
            Self::ProfileRight => "profile_right",
            Self::FullBody => "full_body",
            Self::Costume => "costume",
            Self::ExpressionSmile => "expression_smile",
            Self::ExpressionTense => "expression_tense",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortraitStatus {
    Candidate,
    /// The director's pick, which the reference pack is made from.
    Selected,
    Rejected,
}

/// One image made while casting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CastImage {
    pub artifact_id: String,
    pub kind: CastImageKind,
    /// Where the director stands on a portrait candidate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<PortraitStatus>,
    /// The portrait a reference shows as its identity reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_reference: Option<String>,
    pub content_hash: String,
    pub input_hash: String,
    pub provider: String,
    pub model: String,
    pub media_path: String,
    pub provenance: PromptProvenance,
    pub dependencies: Vec<DependencyRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CastingLog {
    schema_version: u32,
    character_id: String,
    images: Vec<CastImage>,
}

/// An image the lock names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockedImage {
    pub kind: CastImageKind,
    pub artifact_id: String,
    pub content_hash: String,
    pub media_path: String,
}

/// How the provider is given the character's identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderReference {
    pub provider: String,
    /// The request field the reference travels in, such as
    /// `subject_reference`.
    pub mechanism: String,
    pub role: String,
    /// The image sent.
    pub artifact_id: String,
}

/// A locked character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CharacterLock {
    pub schema_version: u32,
    pub character_id: String,
    pub name: String,
    pub portrait: LockedImage,
    pub references: Vec<LockedImage>,
    pub provider_reference: ProviderReference,
    pub forbidden_changes: Vec<String>,
    /// What the canonical portrait was made from.
    pub dependencies: Vec<DependencyRecord>,
}

/// What locking needs beyond the casting record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockRequest {
    pub name: String,
    pub forbidden_changes: Vec<String>,
    /// The reference kinds the pack must hold, as the provider can make
    /// them.
    pub pack: Vec<CastImageKind>,
    pub provider_reference: ProviderReference,
}

/// Whether attaching wrote anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CastAttach {
    Attached,
    AlreadyAttached,
}

#[derive(Debug)]
pub enum CastError {
    InvalidLog,
    InvalidCharacter(String),
    NoSuchImage(String),
    /// Only a portrait candidate can be selected or rejected.
    NotAPortrait(String),
    /// A rejected portrait cannot be selected.
    Rejected(String),
    /// The locked portrait cannot be rejected; lock another first.
    Locked(String),
    /// Locking needs a selected portrait.
    NoSelection,
    /// The reference pack of the selected portrait lacks this kind.
    MissingReference(CastImageKind),
    /// An image no longer holds the bytes its record names.
    Mismatch(String),
    /// An image with this ID is already recorded with other content.
    Conflict(String),
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Manifest(ProjectManifestError),
    Json(serde_json::Error),
}

impl fmt::Display for CastError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLog => formatter.write_str("the casting record is invalid"),
            Self::InvalidCharacter(id) => write!(formatter, "{id:?} is not a character id"),
            Self::NoSuchImage(id) => write!(formatter, "there is no casting image {id}"),
            Self::NotAPortrait(id) => write!(formatter, "casting image {id} is not a portrait"),
            Self::Rejected(id) => write!(formatter, "portrait {id} was rejected"),
            Self::Locked(id) => write!(
                formatter,
                "portrait {id} is the locked portrait; lock another before rejecting it"
            ),
            Self::NoSelection => formatter.write_str("select a portrait first"),
            Self::MissingReference(kind) => write!(
                formatter,
                "the selected portrait has no {} reference yet",
                kind.as_str()
            ),
            Self::Mismatch(path) => write!(formatter, "{path} does not match its recorded hash"),
            Self::Conflict(id) => {
                write!(
                    formatter,
                    "casting image {id} is already recorded with other content"
                )
            }
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid casting JSON: {error}"),
        }
    }
}

impl Error for CastError {
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
fn checked(character_id: &str) -> Result<&str, CastError> {
    if is_portable_id(character_id) && character_id.len() <= 112 {
        Ok(character_id)
    } else {
        Err(CastError::InvalidCharacter(character_id.to_owned()))
    }
}

fn file(character_id: &str, name: &str) -> Result<ProjectRelativePath, CastError> {
    ProjectRelativePath::new(format!("characters/{}/{name}", checked(character_id)?))
        .map_err(CastError::Path)
}

fn read_optional(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
) -> Result<Option<Vec<u8>>, CastError> {
    match project.root().read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(CastError::Fs(error)),
    }
}

fn write_json<T: Serialize>(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
    value: &T,
) -> Result<(), CastError> {
    project.validate_for_write().map_err(CastError::Manifest)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(CastError::Json)?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(path, &bytes)
        .map_err(CastError::Fs)
}

/// The artifact ID of the `number`th image of `character_id`.
fn image_id(character_id: &str, number: usize) -> Result<ArtifactId, CastError> {
    ArtifactId::new(format!("{CAST_PREFIX}{character_id}.{number:04}"))
        .map_err(|_| CastError::InvalidCharacter(character_id.to_owned()))
}

fn load(project: &CanonicalProject, character_id: &str) -> Result<CastingLog, CastError> {
    let Some(bytes) = read_optional(project, &file(character_id, "casting.json")?)? else {
        return Ok(CastingLog {
            schema_version: SCHEMA_VERSION,
            character_id: character_id.to_owned(),
            images: Vec::new(),
        });
    };
    let log: CastingLog = serde_json::from_slice(&bytes).map_err(CastError::Json)?;
    let numbered = log.images.iter().enumerate().all(|(index, image)| {
        image_id(character_id, index + 1).is_ok_and(|id| id.as_str() == image.artifact_id)
    });
    let selected = log
        .images
        .iter()
        .filter(|image| image.status == Some(PortraitStatus::Selected))
        .count();
    let shapes = log.images.iter().all(|image| {
        (image.kind == CastImageKind::Portrait)
            == (image.status.is_some() && image.identity_reference.is_none())
    });
    if log.schema_version == 0
        || log.character_id != character_id
        || !numbered
        || selected > 1
        || !shapes
    {
        return Err(CastError::InvalidLog);
    }
    for image in &log.images {
        ContentHash::new(image.content_hash.clone()).map_err(|_| CastError::InvalidLog)?;
        InputHash::new(image.input_hash.clone()).map_err(|_| CastError::InvalidLog)?;
        ProjectRelativePath::new(&image.media_path).map_err(CastError::Path)?;
    }
    Ok(log)
}

/// Where image `artifact_id` of `character_id` is kept: portraits under
/// `candidates/`, references under `refs/`.
pub fn media_path(
    character_id: &str,
    artifact_id: &ArtifactId,
    kind: CastImageKind,
    extension: &str,
) -> Result<ProjectRelativePath, CastError> {
    if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return Err(CastError::InvalidLog);
    }
    let directory = if kind == CastImageKind::Portrait {
        "candidates"
    } else {
        "refs"
    };
    file(
        character_id,
        &format!("{directory}/{}.{extension}", artifact_id.as_str()),
    )
}

/// Every image of `character_id`, oldest first.
pub fn images(project: &CanonicalProject, character_id: &str) -> Result<Vec<CastImage>, CastError> {
    load(project, character_id).map(|log| log.images)
}

/// The artifact ID the next image of `character_id` gets.
pub fn next_artifact_id(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<ArtifactId, CastError> {
    image_id(character_id, load(project, character_id)?.images.len() + 1)
}

/// The character an image ID belongs to, as in `mira` for `cast.mira.0003`.
#[must_use]
pub fn character_of(artifact_id: &str) -> Option<&str> {
    let rest = artifact_id.strip_prefix(CAST_PREFIX)?;
    let (character, number) = rest.rsplit_once('.')?;
    (number.len() == 4 && number.bytes().all(|byte| byte.is_ascii_digit())).then_some(character)
}

fn check_media(project: &CanonicalProject, image: &CastImage) -> Result<(), CastError> {
    let media = ProjectRelativePath::new(&image.media_path).map_err(CastError::Path)?;
    let bytes = project.root().read(&media).map_err(CastError::Fs)?;
    if ContentHash::of(&bytes).as_str() == image.content_hash {
        Ok(())
    } else {
        Err(CastError::Mismatch(image.media_path.clone()))
    }
}

/// The bytes of `image`, checked against its hash.
pub fn read_image(project: &CanonicalProject, image: &CastImage) -> Result<Vec<u8>, CastError> {
    let media = ProjectRelativePath::new(&image.media_path).map_err(CastError::Path)?;
    let bytes = project.root().read(&media).map_err(CastError::Fs)?;
    if ContentHash::of(&bytes).as_str() == image.content_hash {
        Ok(bytes)
    } else {
        Err(CastError::Mismatch(image.media_path.clone()))
    }
}

/// Record `image` as the next image of `character_id`. Idempotent: an image
/// already recorded exactly as given changes nothing; one that differs is
/// refused. A portrait arrives as a candidate; a reference must name a
/// recorded portrait. Its file must already hold the bytes its record
/// names.
pub fn attach(
    project: &CanonicalProject,
    character_id: &str,
    image: CastImage,
) -> Result<CastAttach, CastError> {
    project.validate_for_write().map_err(CastError::Manifest)?;
    let mut log = load(project, character_id)?;
    if let Some(existing) = log
        .images
        .iter()
        .find(|existing| existing.artifact_id == image.artifact_id)
    {
        let mut expected = existing.clone();
        // The director may since have selected or rejected it.
        expected.status = image.status;
        return if expected == image {
            Ok(CastAttach::AlreadyAttached)
        } else {
            Err(CastError::Conflict(image.artifact_id))
        };
    }
    let shaped = match image.kind {
        CastImageKind::Portrait => {
            image.status == Some(PortraitStatus::Candidate) && image.identity_reference.is_none()
        }
        _ => {
            image.status.is_none()
                && image.identity_reference.as_ref().is_some_and(|portrait| {
                    log.images.iter().any(|recorded| {
                        &recorded.artifact_id == portrait
                            && recorded.kind == CastImageKind::Portrait
                    })
                })
        }
    };
    if image_id(character_id, log.images.len() + 1)?.as_str() != image.artifact_id || !shaped {
        return Err(CastError::InvalidLog);
    }
    check_media(project, &image)?;
    log.images.push(image);
    write_json(project, &file(character_id, "casting.json")?, &log)?;
    Ok(CastAttach::Attached)
}

fn change(
    project: &CanonicalProject,
    character_id: &str,
    artifact_id: &str,
    apply: impl FnOnce(&mut CastingLog, usize) -> Result<(), CastError>,
) -> Result<(), CastError> {
    let mut log = load(project, character_id)?;
    let index = log
        .images
        .iter()
        .position(|image| image.artifact_id == artifact_id)
        .ok_or_else(|| CastError::NoSuchImage(artifact_id.to_owned()))?;
    if log.images[index].kind != CastImageKind::Portrait {
        return Err(CastError::NotAPortrait(artifact_id.to_owned()));
    }
    let before = log.clone();
    apply(&mut log, index)?;
    if log != before {
        write_json(project, &file(character_id, "casting.json")?, &log)?;
    }
    Ok(())
}

/// Select portrait `artifact_id`; a previously selected portrait becomes a
/// candidate again.
pub fn select(
    project: &CanonicalProject,
    character_id: &str,
    artifact_id: &str,
) -> Result<(), CastError> {
    change(project, character_id, artifact_id, |log, index| {
        if log.images[index].status == Some(PortraitStatus::Rejected) {
            return Err(CastError::Rejected(artifact_id.to_owned()));
        }
        for image in &mut log.images {
            if image.status == Some(PortraitStatus::Selected) {
                image.status = Some(PortraitStatus::Candidate);
            }
        }
        log.images[index].status = Some(PortraitStatus::Selected);
        Ok(())
    })
}

/// Reject portrait `artifact_id`. It stays recorded, with its image.
pub fn reject(
    project: &CanonicalProject,
    character_id: &str,
    artifact_id: &str,
) -> Result<(), CastError> {
    if locked(project, character_id)?.is_some_and(|lock| lock.portrait.artifact_id == artifact_id) {
        return Err(CastError::Locked(artifact_id.to_owned()));
    }
    change(project, character_id, artifact_id, |log, index| {
        log.images[index].status = Some(PortraitStatus::Rejected);
        Ok(())
    })
}

/// The selected portrait of `character_id`, if one is.
pub fn selected_portrait(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<Option<CastImage>, CastError> {
    Ok(images(project, character_id)?
        .into_iter()
        .find(|image| image.status == Some(PortraitStatus::Selected)))
}

/// The newest reference of each kind made from `portrait`.
pub fn pack(
    project: &CanonicalProject,
    character_id: &str,
    portrait: &str,
) -> Result<Vec<CastImage>, CastError> {
    let mut pack: Vec<CastImage> = Vec::new();
    for image in images(project, character_id)?.into_iter().rev() {
        if image.identity_reference.as_deref() == Some(portrait)
            && !pack.iter().any(|kept| kept.kind == image.kind)
        {
            pack.push(image);
        }
    }
    pack.sort_by_key(|image| image.kind);
    Ok(pack)
}

fn locked_image(image: &CastImage) -> LockedImage {
    LockedImage {
        kind: image.kind,
        artifact_id: image.artifact_id.clone(),
        content_hash: image.content_hash.clone(),
        media_path: image.media_path.clone(),
    }
}

/// Lock `character_id` as cast: its selected portrait and that portrait's
/// reference pack, which must hold every kind `request.pack` names. Every
/// image must still hold the bytes its record names.
pub fn lock(
    project: &CanonicalProject,
    character_id: &str,
    request: LockRequest,
) -> Result<CharacterLock, CastError> {
    let portrait = selected_portrait(project, character_id)?.ok_or(CastError::NoSelection)?;
    check_media(project, &portrait)?;
    let made = pack(project, character_id, &portrait.artifact_id)?;
    let mut references = Vec::new();
    for kind in request.pack {
        let reference = made
            .iter()
            .find(|image| image.kind == kind)
            .ok_or(CastError::MissingReference(kind))?;
        check_media(project, reference)?;
        references.push(locked_image(reference));
    }
    let lock = CharacterLock {
        schema_version: SCHEMA_VERSION,
        character_id: character_id.to_owned(),
        name: request.name,
        portrait: locked_image(&portrait),
        references,
        provider_reference: request.provider_reference,
        forbidden_changes: request.forbidden_changes,
        dependencies: portrait.dependencies,
    };
    write_json(project, &file(character_id, "character.json")?, &lock)?;
    Ok(lock)
}

/// The lock of `character_id`, if it is cast.
pub fn locked(
    project: &CanonicalProject,
    character_id: &str,
) -> Result<Option<CharacterLock>, CastError> {
    let Some(bytes) = read_optional(project, &file(character_id, "character.json")?)? else {
        return Ok(None);
    };
    let lock: CharacterLock = serde_json::from_slice(&bytes).map_err(CastError::Json)?;
    if lock.schema_version == 0 || lock.character_id != character_id {
        return Err(CastError::InvalidLog);
    }
    for image in std::iter::once(&lock.portrait).chain(&lock.references) {
        ContentHash::new(image.content_hash.clone()).map_err(|_| CastError::InvalidLog)?;
    }
    Ok(Some(lock))
}

/// What in `recorded` no longer matches `current`, the dependencies the same
/// work would record now: a parent changed or gone, or a new one. Each is
/// named by artifact ID.
#[must_use]
pub fn stale_parents(recorded: &[DependencyRecord], current: &[ArtifactDependency]) -> Vec<String> {
    let mut causes = Vec::new();
    for dependency in recorded {
        if !current.iter().any(|now| {
            now.artifact_id.as_str() == dependency.artifact_id
                && now.content_hash.as_str() == dependency.content_hash
        }) {
            causes.push(dependency.artifact_id.clone());
        }
    }
    for now in current {
        if !recorded
            .iter()
            .any(|dependency| dependency.artifact_id == now.artifact_id.as_str())
        {
            causes.push(now.artifact_id.as_str().to_owned());
        }
    }
    causes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        DependencyImpact,
        manifest::{
            PROJECT_FORMAT_VERSION, ProjectDefaults, ProjectManifest, ProjectSource,
            ProjectWorkflow,
        },
    };
    use std::{collections::BTreeMap, fs, path::PathBuf};

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kineto-cast-doc-test-{}-{}",
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

    fn project(target: &std::path::Path) -> CanonicalProject {
        CanonicalProject::create(
            target,
            ProjectManifest {
                format_version: PROJECT_FORMAT_VERSION,
                project_id: "cast_doc_test".to_owned(),
                title: "Cast Test".to_owned(),
                created_at: "2026-10-02T00:00:00Z".to_owned(),
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
            b"A filmmaker waits.\n",
        )
        .unwrap()
    }

    fn parent(id: &str, hash: &str) -> DependencyRecord {
        DependencyRecord {
            artifact_id: id.to_owned(),
            content_hash: ContentHash::of(hash.as_bytes()).as_str().to_owned(),
            impact: DependencyImpact::Semantic,
            field: None,
        }
    }

    /// Write an image and record it as the next image of the filmmaker.
    fn add(
        project: &CanonicalProject,
        kind: CastImageKind,
        portrait: Option<&str>,
        bytes: &[u8],
    ) -> CastImage {
        let id = next_artifact_id(project, "filmmaker").unwrap();
        let media = media_path("filmmaker", &id, kind, "jpg").unwrap();
        project.root().write_atomic(&media, bytes).unwrap();
        let image = CastImage {
            artifact_id: id.as_str().to_owned(),
            kind,
            status: (kind == CastImageKind::Portrait).then_some(PortraitStatus::Candidate),
            identity_reference: portrait.map(str::to_owned),
            content_hash: ContentHash::of(bytes).as_str().to_owned(),
            input_hash: format!("sha256:{}", "a".repeat(64)),
            provider: "minimax".to_owned(),
            model: "image-01".to_owned(),
            media_path: media.as_path().to_string_lossy().into_owned(),
            provenance: PromptProvenance {
                compiler_version: "cast-portrait/1".to_owned(),
                compiled_prompt: "prompt".to_owned(),
                prompt_hash: format!("sha256:{}", "b".repeat(64)),
                applied_constraints: Vec::new(),
                dropped_constraints: Vec::new(),
            },
            dependencies: vec![parent("bible_character.filmmaker", "entry")],
        };
        assert_eq!(
            attach(project, "filmmaker", image.clone()).unwrap(),
            CastAttach::Attached
        );
        image
    }

    fn request(pack: Vec<CastImageKind>) -> LockRequest {
        LockRequest {
            name: "The Filmmaker".to_owned(),
            forbidden_changes: vec!["Grey at the temples.".to_owned()],
            pack,
            provider_reference: ProviderReference {
                provider: "minimax".to_owned(),
                mechanism: "subject_reference".to_owned(),
                role: "identity".to_owned(),
                artifact_id: String::new(),
            },
        }
    }

    #[test]
    fn portraits_are_selected_and_the_pack_of_the_selection_is_locked() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let first = add(&project, CastImageKind::Portrait, None, b"\xff\xd8one");
        let second = add(&project, CastImageKind::Portrait, None, b"\xff\xd8two");
        add(&project, CastImageKind::Portrait, None, b"\xff\xd8three");
        assert_eq!(first.artifact_id, "cast.filmmaker.0001");
        assert_eq!(
            first.media_path,
            "characters/filmmaker/candidates/cast.filmmaker.0001.jpg"
        );
        assert_eq!(character_of("cast.filmmaker.0001"), Some("filmmaker"));

        assert!(matches!(
            lock(&project, "filmmaker", request(vec![CastImageKind::Front])),
            Err(CastError::NoSelection)
        ));
        select(&project, "filmmaker", &first.artifact_id).unwrap();
        // A pack made from the first portrait, then a newer front.
        add(
            &project,
            CastImageKind::Front,
            Some(&first.artifact_id),
            b"\xff\xd8f1",
        );
        let front = add(
            &project,
            CastImageKind::Front,
            Some(&first.artifact_id),
            b"\xff\xd8f2",
        );
        assert_eq!(
            front.media_path,
            "characters/filmmaker/refs/cast.filmmaker.0005.jpg"
        );
        assert!(matches!(
            lock(
                &project,
                "filmmaker",
                request(vec![CastImageKind::Front, CastImageKind::FullBody])
            ),
            Err(CastError::MissingReference(CastImageKind::FullBody))
        ));
        let body = add(
            &project,
            CastImageKind::FullBody,
            Some(&first.artifact_id),
            b"\xff\xd8b",
        );
        let locked_cast = lock(
            &project,
            "filmmaker",
            request(vec![CastImageKind::Front, CastImageKind::FullBody]),
        )
        .unwrap();
        assert_eq!(locked_cast.portrait.artifact_id, first.artifact_id);
        assert_eq!(
            locked_cast
                .references
                .iter()
                .map(|image| image.artifact_id.as_str())
                .collect::<Vec<_>>(),
            [front.artifact_id.as_str(), body.artifact_id.as_str()],
            "the newest of each kind"
        );
        assert!(matches!(
            reject(&project, "filmmaker", &first.artifact_id),
            Err(CastError::Locked(_))
        ));
        assert!(matches!(
            select(&project, "filmmaker", &front.artifact_id),
            Err(CastError::NotAPortrait(_))
        ));
        reject(&project, "filmmaker", &second.artifact_id).unwrap();
        assert!(matches!(
            select(&project, "filmmaker", &second.artifact_id),
            Err(CastError::Rejected(_))
        ));
        drop(project);

        fs::remove_dir_all(target.join(".kineto")).unwrap();
        let project = CanonicalProject::open(&target).unwrap();
        assert_eq!(locked(&project, "filmmaker").unwrap(), Some(locked_cast));
        assert_eq!(images(&project, "filmmaker").unwrap().len(), 6);
    }

    #[test]
    fn a_reference_must_name_a_portrait_and_a_changed_image_is_not_locked() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        let portrait = add(&project, CastImageKind::Portrait, None, b"\xff\xd8one");
        // A reference naming no recorded portrait is refused.
        let id = next_artifact_id(&project, "filmmaker").unwrap();
        let media = media_path("filmmaker", &id, CastImageKind::Front, "jpg").unwrap();
        project.root().write_atomic(&media, b"\xff\xd8f").unwrap();
        let mut stray = portrait.clone();
        stray.artifact_id = id.as_str().to_owned();
        stray.kind = CastImageKind::Front;
        stray.status = None;
        stray.identity_reference = Some("cast.filmmaker.0009".to_owned());
        stray.media_path = media.as_path().to_string_lossy().into_owned();
        stray.content_hash = ContentHash::of(b"\xff\xd8f").as_str().to_owned();
        assert!(matches!(
            attach(&project, "filmmaker", stray),
            Err(CastError::InvalidLog)
        ));

        select(&project, "filmmaker", &portrait.artifact_id).unwrap();
        fs::write(target.join(&portrait.media_path), b"edited").unwrap();
        assert!(matches!(
            lock(&project, "filmmaker", request(Vec::new())),
            Err(CastError::Mismatch(_))
        ));
        assert!(locked(&project, "filmmaker").unwrap().is_none());
        assert!(matches!(
            images(&project, "../filmmaker"),
            Err(CastError::InvalidCharacter(_))
        ));
    }

    #[test]
    fn stale_parents_name_what_changed_went_or_came() {
        let recorded = vec![
            parent("bible_character.filmmaker", "entry"),
            parent("style_s0003", "lock"),
        ];
        let now = |id: &str, hash: &str| ArtifactDependency {
            artifact_id: ArtifactId::new(id).unwrap(),
            content_hash: ContentHash::of(hash.as_bytes()),
            impact: DependencyImpact::Semantic,
            field: None,
        };
        assert!(
            stale_parents(
                &recorded,
                &[
                    now("bible_character.filmmaker", "entry"),
                    now("style_s0003", "lock")
                ]
            )
            .is_empty()
        );
        assert_eq!(
            stale_parents(
                &recorded,
                &[
                    now("bible_character.filmmaker", "edited"),
                    now("bible_world", "w")
                ]
            ),
            ["bible_character.filmmaker", "style_s0003", "bible_world"]
        );
    }
}
