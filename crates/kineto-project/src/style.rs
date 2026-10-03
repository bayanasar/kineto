//! Style exploration's canonical state: the sample images of the style
//! directions the director asked for, and the style they lock.
//!
//! `style/samples.json` (`style-samples.schema.json`) records every sample:
//! the preset it was made with and that preset's directives verbatim, the
//! director's note, the scene it shows, its image under `style/samples/`,
//! its prompt provenance and its dependencies. A sample is a `candidate`,
//! the one `selected`, or `rejected`; nothing is deleted.
//!
//! `bible/style.json` (`style.schema.json`) is the lock. It names the locked
//! sample by its stable artifact ID and the hash of its image, and carries
//! the style itself (preset, version, directives and note) verbatim, so
//! later visual work reads the locked style without the preset table of
//! whichever build it runs on. Every change is one atomic file write.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    ArtifactId, ContentHash, InputHash,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError},
    revisions::DependencyRecord,
    shot::PromptProvenance,
};

/// Where the samples are recorded.
pub const SAMPLES_PATH: &str = "style/samples.json";

/// Where the locked style is kept.
pub const LOCK_PATH: &str = "bible/style.json";

/// The artifact ID prefix of a sample, as in `style_s0003`.
pub const SAMPLE_PREFIX: &str = "style_s";

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SampleStatus {
    Candidate,
    /// The director's pick, which can be locked. At most one sample is.
    Selected,
    Rejected,
}

/// One sample of a style direction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleSample {
    pub artifact_id: String,
    pub preset: String,
    pub preset_version: u32,
    /// The preset's directives as they were compiled into the prompt.
    pub directives: String,
    /// The director's note on how this direction should differ.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The sample this one reworks with the note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refines: Option<String>,
    /// The screenplay scene the sample shows.
    pub scene_id: String,
    pub status: SampleStatus,
    pub content_hash: String,
    pub input_hash: String,
    pub provider: String,
    pub model: String,
    pub media_path: String,
    pub provenance: PromptProvenance,
    pub dependencies: Vec<DependencyRecord>,
    /// Whether its bytes are kept; `None` for a record made before eviction,
    /// which keeps them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_state: Option<crate::media::MediaState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SamplesLog {
    schema_version: u32,
    samples: Vec<StyleSample>,
}

/// The style production follows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StyleLock {
    pub schema_version: u32,
    /// The locked sample.
    pub artifact_id: String,
    /// The hash of the locked sample's image.
    pub content_hash: String,
    pub preset: String,
    pub preset_version: u32,
    pub directives: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Whether attaching wrote anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleAttach {
    Attached,
    AlreadyAttached,
}

#[derive(Debug)]
pub enum StyleError {
    InvalidLog,
    NoSuchSample(String),
    /// A rejected sample cannot be selected or locked.
    Rejected(String),
    /// Only the selected sample can be locked.
    NotSelected(String),
    /// The locked sample cannot be rejected; lock another first.
    Locked(String),
    /// A sample's image no longer holds the bytes its record names.
    Mismatch(String),
    /// A sample with this ID is already recorded with other content.
    Conflict(String),
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Manifest(ProjectManifestError),
    Json(serde_json::Error),
}

impl fmt::Display for StyleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLog => formatter.write_str("the style samples record is invalid"),
            Self::NoSuchSample(id) => write!(formatter, "there is no style sample {id}"),
            Self::Rejected(id) => write!(formatter, "style sample {id} was rejected"),
            Self::NotSelected(id) => {
                write!(
                    formatter,
                    "style sample {id} is not selected; select it to lock it"
                )
            }
            Self::Locked(id) => write!(
                formatter,
                "style sample {id} is the locked style; lock another before rejecting it"
            ),
            Self::Mismatch(path) => write!(formatter, "{path} does not match its recorded hash"),
            Self::Conflict(id) => {
                write!(
                    formatter,
                    "style sample {id} is already recorded with other content"
                )
            }
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid style JSON: {error}"),
        }
    }
}

impl Error for StyleError {
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

fn path(value: &str) -> ProjectRelativePath {
    ProjectRelativePath::new(value).expect("static style path is valid")
}

fn read_optional(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
) -> Result<Option<Vec<u8>>, StyleError> {
    match project.root().read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(StyleError::Fs(error)),
    }
}

fn write_json<T: Serialize>(
    project: &CanonicalProject,
    path: &ProjectRelativePath,
    value: &T,
) -> Result<(), StyleError> {
    project.validate_for_write().map_err(StyleError::Manifest)?;
    let mut bytes = serde_json::to_vec_pretty(value).map_err(StyleError::Json)?;
    bytes.push(b'\n');
    project
        .root()
        .write_atomic(path, &bytes)
        .map_err(StyleError::Fs)
}

/// The artifact ID of the `number`th sample.
fn sample_id(number: usize) -> Result<ArtifactId, StyleError> {
    ArtifactId::new(format!("{SAMPLE_PREFIX}{number:04}")).map_err(|_| StyleError::InvalidLog)
}

fn load(project: &CanonicalProject) -> Result<SamplesLog, StyleError> {
    let Some(bytes) = read_optional(project, &path(SAMPLES_PATH))? else {
        return Ok(SamplesLog {
            schema_version: SCHEMA_VERSION,
            samples: Vec::new(),
        });
    };
    let log: SamplesLog = serde_json::from_slice(&bytes).map_err(StyleError::Json)?;
    let numbered = log.samples.iter().enumerate().all(|(index, sample)| {
        sample_id(index + 1).is_ok_and(|id| id.as_str() == sample.artifact_id)
    });
    let selected = log
        .samples
        .iter()
        .filter(|sample| sample.status == SampleStatus::Selected)
        .count();
    if log.schema_version == 0 || !numbered || selected > 1 {
        return Err(StyleError::InvalidLog);
    }
    for sample in &log.samples {
        ContentHash::new(sample.content_hash.clone()).map_err(|_| StyleError::InvalidLog)?;
        InputHash::new(sample.input_hash.clone()).map_err(|_| StyleError::InvalidLog)?;
        ProjectRelativePath::new(&sample.media_path).map_err(StyleError::Path)?;
    }
    Ok(log)
}

/// Where the image of sample `artifact_id` is kept.
pub fn sample_media_path(
    artifact_id: &ArtifactId,
    extension: &str,
) -> Result<ProjectRelativePath, StyleError> {
    if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_lowercase()) {
        return Err(StyleError::InvalidLog);
    }
    ProjectRelativePath::new(format!(
        "style/samples/{}.{extension}",
        artifact_id.as_str()
    ))
    .map_err(StyleError::Path)
}

/// Every sample, oldest first.
pub fn samples(project: &CanonicalProject) -> Result<Vec<StyleSample>, StyleError> {
    load(project).map(|log| log.samples)
}

/// The artifact ID the next sample gets.
pub fn next_artifact_id(project: &CanonicalProject) -> Result<ArtifactId, StyleError> {
    sample_id(load(project)?.samples.len() + 1)
}

fn check_media(project: &CanonicalProject, sample: &StyleSample) -> Result<(), StyleError> {
    let media = ProjectRelativePath::new(&sample.media_path).map_err(StyleError::Path)?;
    let bytes = project.root().read(&media).map_err(StyleError::Fs)?;
    if ContentHash::of(&bytes).as_str() == sample.content_hash {
        Ok(())
    } else {
        Err(StyleError::Mismatch(sample.media_path.clone()))
    }
}

/// Record `sample` as the next sample, a candidate. Idempotent: a sample
/// already recorded exactly as given changes nothing; one that differs is
/// refused. Its image must already hold the bytes its record names.
pub fn attach(project: &CanonicalProject, sample: StyleSample) -> Result<SampleAttach, StyleError> {
    project.validate_for_write().map_err(StyleError::Manifest)?;
    let mut log = load(project)?;
    if let Some(existing) = log
        .samples
        .iter()
        .find(|existing| existing.artifact_id == sample.artifact_id)
    {
        let mut expected = existing.clone();
        // The director may since have selected or rejected it.
        expected.status = sample.status;
        return if expected == sample {
            Ok(SampleAttach::AlreadyAttached)
        } else {
            Err(StyleError::Conflict(sample.artifact_id))
        };
    }
    if sample_id(log.samples.len() + 1)?.as_str() != sample.artifact_id
        || sample.status != SampleStatus::Candidate
    {
        return Err(StyleError::InvalidLog);
    }
    check_media(project, &sample)?;
    log.samples.push(sample);
    write_json(project, &path(SAMPLES_PATH), &log)?;
    Ok(SampleAttach::Attached)
}

fn change(
    project: &CanonicalProject,
    artifact_id: &str,
    apply: impl FnOnce(&mut SamplesLog, usize) -> Result<(), StyleError>,
) -> Result<(), StyleError> {
    let mut log = load(project)?;
    let index = log
        .samples
        .iter()
        .position(|sample| sample.artifact_id == artifact_id)
        .ok_or_else(|| StyleError::NoSuchSample(artifact_id.to_owned()))?;
    let before = log.clone();
    apply(&mut log, index)?;
    if log != before {
        write_json(project, &path(SAMPLES_PATH), &log)?;
    }
    Ok(())
}

/// Mark the samples `artifact_ids` as having let go of their bytes. Their
/// records stay, and the caller removes the files after.
pub fn evict(project: &CanonicalProject, artifact_ids: &[String]) -> Result<(), StyleError> {
    project.validate_for_write().map_err(StyleError::Manifest)?;
    let mut log = load(project)?;
    for artifact_id in artifact_ids {
        log.samples
            .iter_mut()
            .find(|sample| &sample.artifact_id == artifact_id)
            .ok_or_else(|| StyleError::NoSuchSample(artifact_id.clone()))?
            .media_state = Some(crate::media::MediaState::Evicted);
    }
    write_json(project, &path(SAMPLES_PATH), &log)
}

/// Select `artifact_id` as the director's pick; a previously selected
/// sample becomes a candidate again.
pub fn select(project: &CanonicalProject, artifact_id: &str) -> Result<(), StyleError> {
    change(project, artifact_id, |log, index| {
        if log.samples[index].status == SampleStatus::Rejected {
            return Err(StyleError::Rejected(artifact_id.to_owned()));
        }
        for sample in &mut log.samples {
            if sample.status == SampleStatus::Selected {
                sample.status = SampleStatus::Candidate;
            }
        }
        log.samples[index].status = SampleStatus::Selected;
        Ok(())
    })
}

/// Reject `artifact_id`. It stays recorded, with its image.
pub fn reject(project: &CanonicalProject, artifact_id: &str) -> Result<(), StyleError> {
    if locked(project)?.is_some_and(|lock| lock.artifact_id == artifact_id) {
        return Err(StyleError::Locked(artifact_id.to_owned()));
    }
    change(project, artifact_id, |log, index| {
        log.samples[index].status = SampleStatus::Rejected;
        Ok(())
    })
}

/// Lock the selected sample `artifact_id` as the style production follows,
/// in place of any style locked before.
pub fn lock(project: &CanonicalProject, artifact_id: &str) -> Result<StyleLock, StyleError> {
    let sample = samples(project)?
        .into_iter()
        .find(|sample| sample.artifact_id == artifact_id)
        .ok_or_else(|| StyleError::NoSuchSample(artifact_id.to_owned()))?;
    match sample.status {
        SampleStatus::Selected => {}
        SampleStatus::Rejected => return Err(StyleError::Rejected(sample.artifact_id)),
        SampleStatus::Candidate => return Err(StyleError::NotSelected(sample.artifact_id)),
    }
    check_media(project, &sample)?;
    let lock = StyleLock {
        schema_version: SCHEMA_VERSION,
        artifact_id: sample.artifact_id,
        content_hash: sample.content_hash,
        preset: sample.preset,
        preset_version: sample.preset_version,
        directives: sample.directives,
        note: sample.note,
    };
    write_json(project, &path(LOCK_PATH), &lock)?;
    Ok(lock)
}

/// The dependency work made in the locked style records on it: the locked
/// sample, and the hash of the lock, which changes with the style itself.
#[must_use]
pub fn lock_dependency(lock: &StyleLock) -> Option<crate::ArtifactDependency> {
    Some(crate::ArtifactDependency {
        artifact_id: ArtifactId::new(lock.artifact_id.clone()).ok()?,
        content_hash: ContentHash::of(&serde_json::to_vec(lock).ok()?),
        impact: crate::DependencyImpact::Semantic,
        field: None,
    })
}

/// The locked style, if there is one.
pub fn locked(project: &CanonicalProject) -> Result<Option<StyleLock>, StyleError> {
    let Some(bytes) = read_optional(project, &path(LOCK_PATH))? else {
        return Ok(None);
    };
    let lock: StyleLock = serde_json::from_slice(&bytes).map_err(StyleError::Json)?;
    if lock.schema_version == 0
        || ArtifactId::new(lock.artifact_id.clone()).is_err()
        || ContentHash::new(lock.content_hash.clone()).is_err()
        || lock.directives.trim().is_empty()
    {
        return Err(StyleError::InvalidLog);
    }
    Ok(Some(lock))
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
                "kineto-style-doc-test-{}-{}",
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
                project_id: "style_doc_test".to_owned(),
                title: "Style Test".to_owned(),
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

    /// Write an image and record it as the next sample of `preset`.
    fn add(project: &CanonicalProject, preset: &str, bytes: &[u8]) -> StyleSample {
        let id = next_artifact_id(project).unwrap();
        let media = sample_media_path(&id, "jpg").unwrap();
        project.root().write_atomic(&media, bytes).unwrap();
        let sample = StyleSample {
            artifact_id: id.as_str().to_owned(),
            preset: preset.to_owned(),
            preset_version: 1,
            directives: format!("{preset} directives"),
            note: None,
            refines: None,
            scene_id: "office_night".to_owned(),
            status: SampleStatus::Candidate,
            content_hash: ContentHash::of(bytes).as_str().to_owned(),
            input_hash: format!("sha256:{}", "a".repeat(64)),
            provider: "minimax".to_owned(),
            model: "image-01".to_owned(),
            media_path: media.as_path().to_string_lossy().into_owned(),
            provenance: PromptProvenance {
                compiler_version: "style-sample/1".to_owned(),
                compiled_prompt: "prompt".to_owned(),
                prompt_hash: format!("sha256:{}", "b".repeat(64)),
                applied_constraints: vec!["style.preset".to_owned()],
                dropped_constraints: Vec::new(),
            },
            dependencies: Vec::new(),
            media_state: None,
        };
        assert_eq!(
            attach(project, sample.clone()).unwrap(),
            SampleAttach::Attached
        );
        sample
    }

    fn status(project: &CanonicalProject) -> Vec<SampleStatus> {
        samples(project)
            .unwrap()
            .iter()
            .map(|sample| sample.status)
            .collect()
    }

    #[test]
    fn samples_are_numbered_and_attaching_is_idempotent() {
        let temp = TempDir::new();
        let project = project(&temp.0.join("film"));
        let first = add(&project, "film_noir", b"\xff\xd8noir");
        assert_eq!(first.artifact_id, "style_s0001");
        assert_eq!(first.media_path, "style/samples/style_s0001.jpg");
        assert_eq!(
            attach(&project, first.clone()).unwrap(),
            SampleAttach::AlreadyAttached
        );
        let mut other = first.clone();
        other.note = Some("warmer".to_owned());
        assert!(matches!(
            attach(&project, other),
            Err(StyleError::Conflict(_))
        ));
        assert_eq!(next_artifact_id(&project).unwrap().as_str(), "style_s0002");

        // An image that does not match its record is not attached.
        let id = next_artifact_id(&project).unwrap();
        let mut forged = first.clone();
        forged.artifact_id = id.as_str().to_owned();
        forged.media_path = sample_media_path(&id, "jpg")
            .unwrap()
            .as_path()
            .to_string_lossy()
            .into_owned();
        project
            .root()
            .write_atomic(&sample_media_path(&id, "jpg").unwrap(), b"other")
            .unwrap();
        assert!(matches!(
            attach(&project, forged),
            Err(StyleError::Mismatch(_))
        ));
    }

    #[test]
    fn the_director_selects_rejects_and_locks_and_the_lock_survives_losing_the_cache() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        add(&project, "film_noir", b"\xff\xd8noir");
        add(&project, "film_realism", b"\xff\xd8real");
        add(&project, "soft_fairy_tale", b"\xff\xd8fairy");

        assert!(matches!(
            lock(&project, "style_s0001"),
            Err(StyleError::NotSelected(_))
        ));
        select(&project, "style_s0001").unwrap();
        select(&project, "style_s0002").unwrap();
        reject(&project, "style_s0003").unwrap();
        assert_eq!(
            status(&project),
            [
                SampleStatus::Candidate,
                SampleStatus::Selected,
                SampleStatus::Rejected
            ]
        );
        assert!(matches!(
            select(&project, "style_s0003"),
            Err(StyleError::Rejected(_))
        ));
        assert!(matches!(
            select(&project, "style_s0009"),
            Err(StyleError::NoSuchSample(_))
        ));

        let locked_style = lock(&project, "style_s0002").unwrap();
        assert_eq!(locked_style.preset, "film_realism");
        assert_eq!(locked_style.directives, "film_realism directives");
        assert!(matches!(
            reject(&project, "style_s0002"),
            Err(StyleError::Locked(_))
        ));
        drop(project);

        fs::remove_dir_all(target.join(".kineto")).unwrap();
        let project = CanonicalProject::open(&target).unwrap();
        assert_eq!(locked(&project).unwrap(), Some(locked_style));
        assert_eq!(
            status(&project),
            [
                SampleStatus::Candidate,
                SampleStatus::Selected,
                SampleStatus::Rejected
            ]
        );
        assert!(
            target.join("style/samples/style_s0003.jpg").exists(),
            "a rejected sample keeps its image"
        );
    }

    #[test]
    fn a_sample_whose_image_changed_is_not_locked() {
        let temp = TempDir::new();
        let target = temp.0.join("film");
        let project = project(&target);
        add(&project, "film_noir", b"\xff\xd8noir");
        select(&project, "style_s0001").unwrap();
        fs::write(target.join("style/samples/style_s0001.jpg"), b"edited").unwrap();
        assert!(matches!(
            lock(&project, "style_s0001"),
            Err(StyleError::Mismatch(_))
        ));
        assert_eq!(locked(&project).unwrap(), None);
    }
}
