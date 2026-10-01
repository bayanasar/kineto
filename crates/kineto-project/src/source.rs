//! The project's source text, kept as revisions.
//!
//! The source a project was created with is revision 1, at the path its
//! manifest names. Importing text again never overwrites it: each import is
//! normalised and written as a new revision under `source/revisions/`, and
//! `source/source.json` lists every revision with its hash. Work derived from
//! the source records the hash of the revision it read, so a later import
//! makes that work stale instead of silently changing what it was made from.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::{
    ContentHash,
    fs::{ProjectFsError, ProjectPathError, ProjectRelativePath},
    manifest::{CanonicalProject, ProjectManifestError},
};

const SOURCE_MANIFEST_PATH: &str = "source/source.json";
const SOURCE_SCHEMA_VERSION: u32 = 1;
/// A source beyond this size is not a story to adapt but a library.
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
const MAX_ORIGINAL_NAME_CHARS: usize = 255;

/// How the source text is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Text,
    Markdown,
}

impl SourceFormat {
    const fn extension(self) -> &'static str {
        match self {
            Self::Text => "txt",
            Self::Markdown => "md",
        }
    }
}

/// One revision of the source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRevision {
    pub revision: u32,
    pub path: String,
    pub format: SourceFormat,
    pub content_hash: String,
    /// Characters, after normalisation.
    pub chars: u64,
    /// The name of the file it was imported from, if it came from one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_name: Option<String>,
}

impl SourceRevision {
    /// The revision's content hash, checked.
    pub fn hash(&self) -> Result<ContentHash, SourceError> {
        ContentHash::new(self.content_hash.clone()).map_err(|_| SourceError::InvalidManifest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SourceManifest {
    schema_version: u32,
    revisions: Vec<SourceRevision>,
}

#[derive(Debug)]
pub enum SourceError {
    /// The bytes are not UTF-8 text.
    NotText,
    /// There is no text once whitespace is set aside.
    Empty,
    TooLarge,
    InvalidManifest,
    Path(ProjectPathError),
    Fs(ProjectFsError),
    Manifest(ProjectManifestError),
    Json(serde_json::Error),
}

impl fmt::Display for SourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotText => formatter.write_str("the source is not UTF-8 text"),
            Self::Empty => formatter.write_str("the source has no text"),
            Self::TooLarge => write!(
                formatter,
                "the source is larger than {} MiB",
                MAX_SOURCE_BYTES / 1024 / 1024
            ),
            Self::InvalidManifest => formatter.write_str("source/source.json is invalid"),
            Self::Path(error) => error.fmt(formatter),
            Self::Fs(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Json(error) => write!(formatter, "invalid source manifest JSON: {error}"),
        }
    }
}

impl Error for SourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Path(error) => Some(error),
            Self::Fs(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Json(error) => Some(error),
            Self::NotText | Self::Empty | Self::TooLarge | Self::InvalidManifest => None,
        }
    }
}

/// Normalise imported bytes: UTF-8 only, no byte-order mark, `\n` line
/// endings, no trailing whitespace at line ends, exactly one final newline.
pub fn normalize(bytes: &[u8]) -> Result<String, SourceError> {
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(SourceError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| SourceError::NotText)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.contains('\0') {
        return Err(SourceError::NotText);
    }
    let unified = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut normalized = unified
        .split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .trim_start_matches('\n')
        .to_owned();
    if normalized.trim().is_empty() {
        return Err(SourceError::Empty);
    }
    normalized.push('\n');
    Ok(normalized)
}

fn manifest_path() -> ProjectRelativePath {
    ProjectRelativePath::new(SOURCE_MANIFEST_PATH).expect("static source manifest path is valid")
}

fn chars(text: &str) -> u64 {
    u64::try_from(text.chars().count()).unwrap_or(u64::MAX)
}

/// The revision the project was created with, from its manifest.
fn first_revision(project: &CanonicalProject) -> Result<SourceRevision, SourceError> {
    let path = project
        .manifest()
        .source_relative_path()
        .map_err(SourceError::Manifest)?;
    let bytes = project.root().read(&path).map_err(SourceError::Fs)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| SourceError::NotText)?;
    Ok(SourceRevision {
        revision: 1,
        path: path.as_path().to_string_lossy().replace('\\', "/"),
        format: if path.as_path().extension().is_some_and(|ext| ext == "md") {
            SourceFormat::Markdown
        } else {
            SourceFormat::Text
        },
        content_hash: ContentHash::of(&bytes).as_str().to_owned(),
        chars: chars(text),
        original_name: None,
    })
}

fn load(project: &CanonicalProject) -> Result<Option<SourceManifest>, SourceError> {
    let bytes = match project.root().read(&manifest_path()) {
        Ok(bytes) => bytes,
        Err(ProjectFsError::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(None);
        }
        Err(error) => return Err(SourceError::Fs(error)),
    };
    let manifest: SourceManifest = serde_json::from_slice(&bytes).map_err(SourceError::Json)?;
    let numbered = manifest
        .revisions
        .iter()
        .enumerate()
        .all(|(index, revision)| u32::try_from(index + 1) == Ok(revision.revision));
    if manifest.schema_version == 0 || manifest.revisions.is_empty() || !numbered {
        return Err(SourceError::InvalidManifest);
    }
    Ok(Some(manifest))
}

/// Every revision of the source, oldest first.
pub fn revisions(project: &CanonicalProject) -> Result<Vec<SourceRevision>, SourceError> {
    match load(project)? {
        Some(manifest) => Ok(manifest.revisions),
        None => Ok(vec![first_revision(project)?]),
    }
}

/// The revision work is made from now: the latest one.
pub fn current(project: &CanonicalProject) -> Result<SourceRevision, SourceError> {
    revisions(project)?
        .pop()
        .ok_or(SourceError::InvalidManifest)
}

/// The text of `revision`, checked against its hash.
pub fn read(project: &CanonicalProject, revision: &SourceRevision) -> Result<String, SourceError> {
    let path = ProjectRelativePath::new(&revision.path).map_err(SourceError::Path)?;
    let bytes = project.root().read(&path).map_err(SourceError::Fs)?;
    if ContentHash::of(&bytes) != revision.hash()? {
        return Err(SourceError::InvalidManifest);
    }
    String::from_utf8(bytes).map_err(|_| SourceError::NotText)
}

/// Import `bytes` as the next revision of the source. Nothing earlier is
/// changed. Importing the same text as the current revision changes nothing
/// and returns the current revision.
pub fn import(
    project: &CanonicalProject,
    bytes: &[u8],
    format: SourceFormat,
    original_name: Option<&str>,
) -> Result<SourceRevision, SourceError> {
    project
        .validate_for_write()
        .map_err(SourceError::Manifest)?;
    let text = normalize(bytes)?;
    let hash = ContentHash::of(text.as_bytes());
    let mut revisions = revisions(project)?;
    let latest = revisions.last().ok_or(SourceError::InvalidManifest)?;
    if latest.content_hash == hash.as_str() {
        return Ok(latest.clone());
    }
    let revision = latest.revision + 1;
    let path = ProjectRelativePath::new(format!(
        "source/revisions/r{revision:04}.{}",
        format.extension()
    ))
    .map_err(SourceError::Path)?;
    let next = SourceRevision {
        revision,
        path: path.as_path().to_string_lossy().replace('\\', "/"),
        format,
        content_hash: hash.as_str().to_owned(),
        chars: chars(&text),
        original_name: original_name
            .map(|name| {
                name.chars()
                    .filter(|character| !character.is_control())
                    .take(MAX_ORIGINAL_NAME_CHARS)
                    .collect::<String>()
            })
            .filter(|name| !name.trim().is_empty()),
    };
    // The text is published before the manifest that names it; a crash in
    // between leaves an unnamed file and the previous current revision.
    project
        .root()
        .write_atomic(&path, text.as_bytes())
        .map_err(SourceError::Fs)?;
    revisions.push(next.clone());
    let mut encoded = serde_json::to_vec_pretty(&SourceManifest {
        schema_version: SOURCE_SCHEMA_VERSION,
        revisions,
    })
    .map_err(SourceError::Json)?;
    encoded.push(b'\n');
    project
        .root()
        .write_atomic(&manifest_path(), &encoded)
        .map_err(SourceError::Fs)?;
    Ok(next)
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
                "kineto-source-test-{}-{}",
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
                project_id: "source_test".to_owned(),
                title: "Source Test".to_owned(),
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
            b"A filmmaker waits.\n",
        )
        .unwrap()
    }

    #[test]
    fn normalisation_is_deterministic() {
        assert_eq!(
            normalize(b"\xef\xbb\xbf\r\n\r\nFirst line.  \r\nSecond\rThird\n\n\n").unwrap(),
            "First line.\nSecond\nThird\n"
        );
        assert_eq!(normalize("雨が降る\n".as_bytes()).unwrap(), "雨が降る\n");
        assert!(matches!(normalize(b"\xff\xfe"), Err(SourceError::NotText)));
        assert!(matches!(normalize(b"a\0b"), Err(SourceError::NotText)));
        assert!(matches!(normalize(b" \n\t\r\n"), Err(SourceError::Empty)));
        assert!(matches!(
            normalize(&vec![b'a'; MAX_SOURCE_BYTES + 1]),
            Err(SourceError::TooLarge)
        ));
    }

    #[test]
    fn the_created_source_is_revision_one() {
        let temp = TempDir::new();
        let project = project(&temp);
        let current = current(&project).unwrap();
        assert_eq!(current.revision, 1);
        assert_eq!(current.path, "source/story.txt");
        assert_eq!(current.chars, 19);
        assert_eq!(read(&project, &current).unwrap(), "A filmmaker waits.\n");
        assert!(
            !temp.0.join("film/source/source.json").exists(),
            "reading writes nothing"
        );
    }

    #[test]
    fn importing_adds_a_revision_and_keeps_the_earlier_ones() {
        let temp = TempDir::new();
        let project = project(&temp);
        let second = import(
            &project,
            b"# Chapter One\r\n\r\nThe final line lands.\r\n",
            SourceFormat::Markdown,
            Some("chapter-one.md"),
        )
        .unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.path, "source/revisions/r0002.md");
        assert_eq!(second.original_name.as_deref(), Some("chapter-one.md"));
        assert_eq!(
            read(&project, &second).unwrap(),
            "# Chapter One\n\nThe final line lands.\n"
        );
        // The same text again is not a new revision.
        assert_eq!(
            import(
                &project,
                b"# Chapter One\n\nThe final line lands.\n",
                SourceFormat::Markdown,
                None
            )
            .unwrap(),
            second
        );
        let all = revisions(&project).unwrap();
        assert_eq!(all.len(), 2);
        assert_eq!(read(&project, &all[0]).unwrap(), "A filmmaker waits.\n");
        assert_eq!(current(&project).unwrap(), second);

        // A revision whose bytes changed is refused rather than read.
        fs::write(temp.0.join("film/source/revisions/r0002.md"), "edited\n").unwrap();
        assert!(matches!(
            read(&project, &second),
            Err(SourceError::InvalidManifest)
        ));
    }
}
