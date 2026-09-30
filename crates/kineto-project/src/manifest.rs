use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    fs::OpenOptions,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};

use crate::fs::{
    ProjectFsError, ProjectPathError, ProjectRelativePath, ProjectRoot, SessionLock,
    SessionLockError,
};

pub const PROJECT_FORMAT_VERSION: u32 = 1;
const PROJECT_MANIFEST_PATH: &str = "project.toml";
static CREATE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectAccess {
    ReadWrite,
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub format_version: u32,
    pub project_id: String,
    pub title: String,
    pub created_at: String,
    pub source: ProjectSource,
    pub defaults: ProjectDefaults,
    pub workflow: ProjectWorkflow,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectSource {
    pub kind: String,
    pub path: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectDefaults {
    pub language: String,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectWorkflow {
    pub recipe_id: String,
    pub recipe_version: u32,
    #[serde(flatten)]
    pub extra: BTreeMap<String, toml::Value>,
}

impl ProjectManifest {
    pub fn from_toml(text: &str) -> Result<Self, ProjectManifestError> {
        let manifest: Self = toml::from_str(text).map_err(ProjectManifestError::Decode)?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn to_toml(&self) -> Result<String, ProjectManifestError> {
        self.validate_for_write()?;
        toml::to_string_pretty(self).map_err(ProjectManifestError::Encode)
    }

    /// Validate a manifest for reading.
    ///
    /// Future formats are refused. Older formats remain readable so an upgrade
    /// can expose them read-only until an explicit migration exists.
    pub fn validate(&self) -> Result<(), ProjectManifestError> {
        format_access(self.format_version, PROJECT_FORMAT_VERSION)?;
        require_non_empty("project_id", &self.project_id)?;
        require_non_empty("created_at", &self.created_at)?;
        if !is_rfc3339_timestamp(&self.created_at) {
            return Err(ProjectManifestError::InvalidField("created_at"));
        }
        require_non_empty("source.kind", &self.source.kind)?;
        require_non_empty("defaults.language", &self.defaults.language)?;
        require_non_empty("workflow.recipe_id", &self.workflow.recipe_id)?;
        if self.workflow.recipe_version == 0 {
            return Err(ProjectManifestError::InvalidField(
                "workflow.recipe_version",
            ));
        }

        let source_path = ProjectRelativePath::new(&self.source.path)
            .map_err(ProjectManifestError::InvalidSourcePath)?;
        let mut components = source_path.as_path().components();
        if matches!(components.next(), Some(Component::Normal(name)) if name == ".kineto")
            || source_path.as_path() == Path::new(PROJECT_MANIFEST_PATH)
        {
            return Err(ProjectManifestError::InvalidField("source.path"));
        }
        Ok(())
    }

    /// Validate that this build may write the manifest without migration.
    pub fn validate_for_write(&self) -> Result<(), ProjectManifestError> {
        self.validate()?;
        if self.access()? == ProjectAccess::ReadOnly {
            return Err(ProjectManifestError::ReadOnlyFormatVersion(
                self.format_version,
            ));
        }
        Ok(())
    }

    pub fn access(&self) -> Result<ProjectAccess, ProjectManifestError> {
        format_access(self.format_version, PROJECT_FORMAT_VERSION)
    }

    fn source_relative_path(&self) -> Result<ProjectRelativePath, ProjectManifestError> {
        ProjectRelativePath::new(&self.source.path).map_err(ProjectManifestError::InvalidSourcePath)
    }
}

fn format_access(
    format_version: u32,
    supported_version: u32,
) -> Result<ProjectAccess, ProjectManifestError> {
    if format_version == 0 {
        return Err(ProjectManifestError::InvalidField("format_version"));
    }
    if format_version > supported_version {
        return Err(ProjectManifestError::UnsupportedFormatVersion(
            format_version,
        ));
    }
    if format_version < supported_version {
        Ok(ProjectAccess::ReadOnly)
    } else {
        Ok(ProjectAccess::ReadWrite)
    }
}

/// `schemas/project.schema.json` declares `created_at` as `format: date-time`.
/// Accept exactly that shape so a project cannot be stamped with free text the
/// schema would reject.
fn is_rfc3339_timestamp(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !(digits(0..4)
        && bytes[4] == b'-'
        && digits(5..7)
        && bytes[7] == b'-'
        && digits(8..10)
        && matches!(bytes[10], b'T' | b't')
        && digits(11..13)
        && bytes[13] == b':'
        && digits(14..16)
        && bytes[16] == b':'
        && digits(17..19))
    {
        return false;
    }

    let mut rest = &bytes[19..];
    if rest.first() == Some(&b'.') {
        let fraction = rest[1..]
            .iter()
            .take_while(|byte| byte.is_ascii_digit())
            .count();
        if fraction == 0 {
            return false;
        }
        rest = &rest[1 + fraction..];
    }

    match rest {
        [b'Z' | b'z'] => true,
        [b'+' | b'-', ..] if rest.len() == 6 => {
            rest[1..3].iter().all(u8::is_ascii_digit)
                && rest[3] == b':'
                && rest[4..6].iter().all(u8::is_ascii_digit)
        }
        _ => false,
    }
}

fn require_non_empty(field: &'static str, value: &str) -> Result<(), ProjectManifestError> {
    if value.trim().is_empty() {
        Err(ProjectManifestError::InvalidField(field))
    } else {
        Ok(())
    }
}

#[derive(Debug)]
pub enum ProjectManifestError {
    Decode(toml::de::Error),
    Encode(toml::ser::Error),
    UnsupportedFormatVersion(u32),
    ReadOnlyFormatVersion(u32),
    InvalidField(&'static str),
    InvalidSourcePath(ProjectPathError),
}

impl fmt::Display for ProjectManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode(error) => write!(formatter, "invalid project.toml: {error}"),
            Self::Encode(error) => write!(formatter, "could not encode project.toml: {error}"),
            Self::UnsupportedFormatVersion(version) => write!(
                formatter,
                "unsupported project format version {version}; this build supports {PROJECT_FORMAT_VERSION}"
            ),
            Self::ReadOnlyFormatVersion(version) => write!(
                formatter,
                "project format version {version} is readable but requires migration before writing"
            ),
            Self::InvalidField(field) => {
                write!(formatter, "invalid project manifest field: {field}")
            }
            Self::InvalidSourcePath(error) => error.fmt(formatter),
        }
    }
}

impl Error for ProjectManifestError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Decode(error) => Some(error),
            Self::Encode(error) => Some(error),
            Self::InvalidSourcePath(error) => Some(error),
            Self::UnsupportedFormatVersion(_)
            | Self::ReadOnlyFormatVersion(_)
            | Self::InvalidField(_) => None,
        }
    }
}

#[derive(Debug)]
pub struct CanonicalProject {
    root: ProjectRoot,
    manifest: ProjectManifest,
    access: ProjectAccess,
    // A read-write session holds the project's single-writer lock for its whole
    // life. Without it, two sessions each write from their own cached state and
    // the later writer silently discards the earlier one's approvals.
    _session_lock: Option<SessionLock>,
}

impl CanonicalProject {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProjectStoreError> {
        Self::open_with_supported_version(path.as_ref(), PROJECT_FORMAT_VERSION)
    }

    fn open_with_supported_version(
        path: &Path,
        supported_version: u32,
    ) -> Result<Self, ProjectStoreError> {
        let root = ProjectRoot::open(path).map_err(ProjectStoreError::Fs)?;
        let manifest_path = ProjectRelativePath::new(PROJECT_MANIFEST_PATH)
            .expect("static canonical project manifest path is valid");
        let manifest_bytes = root.read(&manifest_path).map_err(ProjectStoreError::Fs)?;
        let manifest_text =
            std::str::from_utf8(&manifest_bytes).map_err(ProjectStoreError::Utf8)?;
        let manifest =
            ProjectManifest::from_toml(manifest_text).map_err(ProjectStoreError::Manifest)?;
        let access = format_access(manifest.format_version, supported_version)
            .map_err(ProjectStoreError::Manifest)?;
        let source_path = manifest
            .source_relative_path()
            .map_err(ProjectStoreError::Manifest)?;

        // Opening a project proves that its declared canonical source resolves
        // within the same symlink-refusing project filesystem boundary. Prove
        // it by resolving the path, never by reading a source of unknown size.
        root.ensure_file(&source_path)
            .map_err(ProjectStoreError::Fs)?;

        let session_lock = match access {
            ProjectAccess::ReadWrite => root.lock_session().map_err(|error| match error {
                SessionLockError::Busy(path) => ProjectStoreError::Busy(path),
                SessionLockError::Fs(error) => ProjectStoreError::Fs(error),
            })?,
            ProjectAccess::ReadOnly => None,
        };

        Ok(Self {
            root,
            manifest,
            access,
            _session_lock: session_lock,
        })
    }

    #[cfg(test)]
    pub(crate) fn open_with_supported_version_for_test(
        path: impl AsRef<Path>,
        supported_version: u32,
    ) -> Result<Self, ProjectStoreError> {
        Self::open_with_supported_version(path.as_ref(), supported_version)
    }

    pub fn create(
        path: impl AsRef<Path>,
        manifest: ProjectManifest,
        source_bytes: &[u8],
    ) -> Result<Self, ProjectStoreError> {
        manifest
            .validate_for_write()
            .map_err(ProjectStoreError::Manifest)?;
        let source_path = manifest
            .source_relative_path()
            .map_err(ProjectStoreError::Manifest)?;
        let target = path.as_ref();
        if fs::symlink_metadata(target).is_ok() {
            return Err(ProjectStoreError::AlreadyExists(target.to_path_buf()));
        }

        let parent = target
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent)
            .map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?;
        if !parent.is_dir() {
            return Err(ProjectStoreError::Fs(ProjectFsError::NotDirectory(parent)));
        }
        let file_name = target
            .file_name()
            .ok_or_else(|| ProjectStoreError::InvalidTarget(target.to_path_buf()))?;
        let final_path = parent.join(file_name);
        if fs::symlink_metadata(&final_path).is_ok() {
            return Err(ProjectStoreError::AlreadyExists(final_path));
        }

        let staging = create_staging_directory(&parent)?;
        let result = (|| {
            let root = ProjectRoot::open(&staging).map_err(ProjectStoreError::Fs)?;
            write_new(&root, &source_path, source_bytes)?;
            let encoded = manifest.to_toml().map_err(ProjectStoreError::Manifest)?;
            let manifest_path = ProjectRelativePath::new(PROJECT_MANIFEST_PATH)
                .expect("static canonical project manifest path is valid");
            write_new(&root, &manifest_path, encoded.as_bytes())?;

            // File contents are already fsynced by `write_new`. Flush the
            // directory entries that name them, then publish the project with
            // one rename, then flush the parent so the rename itself is
            // durable. Without the parent flush a crash can leave a project
            // whose bytes are on disk under a name nothing points at.
            sync_tree(&staging)?;
            fs::rename(&staging, &final_path)
                .map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?;
            ProjectRoot::sync_directory(&parent).map_err(ProjectStoreError::Fs)?;
            Ok(())
        })();

        if let Err(error) = result {
            let _ = fs::remove_dir_all(&staging);
            return Err(error);
        }

        Self::open(final_path)
    }

    #[must_use]
    pub fn root(&self) -> &ProjectRoot {
        &self.root
    }

    #[must_use]
    pub fn manifest(&self) -> &ProjectManifest {
        &self.manifest
    }

    #[must_use]
    pub const fn access(&self) -> ProjectAccess {
        self.access
    }

    #[must_use]
    pub const fn is_read_only(&self) -> bool {
        matches!(self.access, ProjectAccess::ReadOnly)
    }

    pub(crate) fn validate_for_write(&self) -> Result<(), ProjectManifestError> {
        self.manifest.validate()?;
        if self.is_read_only() {
            return Err(ProjectManifestError::ReadOnlyFormatVersion(
                self.manifest.format_version,
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub enum ProjectStoreError {
    Fs(ProjectFsError),
    Manifest(ProjectManifestError),
    Utf8(std::str::Utf8Error),
    AlreadyExists(PathBuf),
    InvalidTarget(PathBuf),
    /// The project is already open for writing by another live session.
    Busy(PathBuf),
}

impl fmt::Display for ProjectStoreError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Fs(error) => error.fmt(formatter),
            Self::Manifest(error) => error.fmt(formatter),
            Self::Utf8(error) => write!(formatter, "project.toml must be UTF-8: {error}"),
            Self::AlreadyExists(path) => write!(
                formatter,
                "project target already exists: {}",
                path.display()
            ),
            Self::InvalidTarget(path) => {
                write!(formatter, "invalid project target: {}", path.display())
            }
            Self::Busy(path) => write!(
                formatter,
                "project is already open for writing in another Kineto session: {}",
                path.display()
            ),
        }
    }
}

impl Error for ProjectStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Fs(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::Utf8(error) => Some(error),
            Self::AlreadyExists(_) | Self::InvalidTarget(_) | Self::Busy(_) => None,
        }
    }
}

fn create_staging_directory(parent: &Path) -> Result<PathBuf, ProjectStoreError> {
    for _ in 0..32 {
        let nonce = CREATE_NONCE.fetch_add(1, Ordering::Relaxed);
        let staging = parent.join(format!(".kineto-create-{}-{nonce}", std::process::id()));
        match fs::create_dir(&staging) {
            Ok(()) => return Ok(staging),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(ProjectStoreError::Fs(ProjectFsError::Io(error))),
        }
    }
    Err(ProjectStoreError::InvalidTarget(parent.to_path_buf()))
}

/// Flush every directory in a freshly staged project so the names created
/// inside it survive a crash that happens right after the publishing rename.
fn sync_tree(directory: &Path) -> Result<(), ProjectStoreError> {
    for entry in
        fs::read_dir(directory).map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?
    {
        let entry = entry.map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?;
        if entry
            .file_type()
            .map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?
            .is_dir()
        {
            sync_tree(&entry.path())?;
        }
    }
    ProjectRoot::sync_directory(directory).map_err(ProjectStoreError::Fs)
}

fn write_new(
    root: &ProjectRoot,
    path: &ProjectRelativePath,
    bytes: &[u8],
) -> Result<(), ProjectStoreError> {
    let resolved = root.resolve(path);
    let parent = resolved
        .parent()
        .ok_or_else(|| ProjectStoreError::InvalidTarget(resolved.clone()))?;
    ensure_directory(root, parent)?;

    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&resolved)
        .map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?;
    file.write_all(bytes)
        .map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?;
    file.sync_all()
        .map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))
}

fn ensure_directory(root: &ProjectRoot, directory: &Path) -> Result<(), ProjectStoreError> {
    let relative = directory
        .strip_prefix(root.root())
        .map_err(|_| ProjectStoreError::Fs(ProjectFsError::EscapedRoot(directory.to_path_buf())))?;
    let mut current = root.root().to_path_buf();

    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(ProjectStoreError::Fs(ProjectFsError::EscapedRoot(
                directory.to_path_buf(),
            )));
        };
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(ProjectStoreError::Fs(ProjectFsError::Symlink(current)));
                }
                if !metadata.is_dir() {
                    return Err(ProjectStoreError::Fs(ProjectFsError::NotDirectory(current)));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current)
                    .map_err(|error| ProjectStoreError::Fs(ProjectFsError::Io(error)))?;
            }
            Err(error) => return Err(ProjectStoreError::Fs(ProjectFsError::Io(error))),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kineto-manifest-test-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn manifest() -> ProjectManifest {
        ProjectManifest {
            format_version: PROJECT_FORMAT_VERSION,
            project_id: "project_test_001".to_owned(),
            title: "Test Film".to_owned(),
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
        }
    }

    #[test]
    fn committed_golden_manifest_decodes() {
        let decoded = ProjectManifest::from_toml(include_str!(
            "../../../fixtures/projects/minimal/project.toml"
        ))
        .unwrap();
        assert_eq!(decoded.project_id, "golden_minimal");
        assert_eq!(decoded.source.path, "source/story.txt");
    }

    #[test]
    fn unknown_fields_survive_manifest_round_trip() {
        let input = r#"
format_version = 1
project_id = "round_trip"
title = "Round Trip"
created_at = "2026-09-10T00:00:00Z"
future_top_level = "keep-me"

[source]
kind = "text"
path = "source/story.txt"
future_source_flag = true

[defaults]
language = "en"
future_default = 42

[workflow]
recipe_id = "default-film"
recipe_version = 1
future_workflow = "keep-me-too"
"#;
        let first = ProjectManifest::from_toml(input).unwrap();
        let encoded = first.to_toml().unwrap();
        let second = ProjectManifest::from_toml(&encoded).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            second.extra.get("future_top_level"),
            Some(&toml::Value::String("keep-me".to_owned()))
        );
        assert_eq!(
            second.source.extra.get("future_source_flag"),
            Some(&toml::Value::Boolean(true))
        );
    }

    #[test]
    fn future_project_format_is_refused_for_this_build() {
        let mut manifest = manifest();
        manifest.format_version = PROJECT_FORMAT_VERSION + 1;
        assert!(matches!(
            manifest.validate(),
            Err(ProjectManifestError::UnsupportedFormatVersion(_))
        ));
    }

    #[test]
    fn older_project_formats_are_read_only_until_migrated() {
        assert_eq!(format_access(1, 2).unwrap(), ProjectAccess::ReadOnly);
        assert_eq!(format_access(2, 2).unwrap(), ProjectAccess::ReadWrite);
        assert!(matches!(
            format_access(3, 2),
            Err(ProjectManifestError::UnsupportedFormatVersion(3))
        ));
        assert!(matches!(
            format_access(0, 2),
            Err(ProjectManifestError::InvalidField("format_version"))
        ));
    }

    #[test]
    fn source_must_be_canonical_and_cannot_live_in_dot_kineto() {
        let mut manifest = manifest();
        manifest.source.path = ".kineto/source.txt".to_owned();
        assert!(manifest.validate().is_err());

        manifest.source.path = "../outside.txt".to_owned();
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn create_then_open_round_trips_canonical_project() {
        let temp = TempDir::new();
        let target = temp.path.join("film");
        let source = b"A filmmaker waits across the table.\n";

        let created = CanonicalProject::create(&target, manifest(), source).unwrap();
        assert_eq!(created.manifest().project_id, "project_test_001");
        assert_eq!(created.access(), ProjectAccess::ReadWrite);
        assert!(!created.is_read_only());
        assert_eq!(
            created
                .root()
                .read(&ProjectRelativePath::new("source/story.txt").unwrap())
                .unwrap(),
            source
        );

        drop(created);
        let reopened = CanonicalProject::open(&target).unwrap();
        assert_eq!(reopened.manifest().title, "Test Film");
        assert_eq!(reopened.access(), ProjectAccess::ReadWrite);
        assert!(target.join("project.toml").is_file());
    }

    #[test]
    fn static_manifest_path_is_always_a_valid_project_path() {
        // `CanonicalProject::open`/`create` assert this invariant with `expect`.
        // Prove it here so the assertion can never become a runtime abort.
        assert!(ProjectRelativePath::new(PROJECT_MANIFEST_PATH).is_ok());
    }

    #[test]
    fn created_at_must_match_the_schema_date_time_format() {
        let mut manifest = manifest();
        for accepted in [
            "2026-09-10T00:00:00Z",
            "2026-09-10t00:00:00z",
            "2026-09-10T00:00:00.125Z",
            "2026-09-10T00:00:00+08:00",
            "2026-09-10T00:00:00-05:00",
        ] {
            manifest.created_at = accepted.to_owned();
            assert!(manifest.validate().is_ok(), "{accepted} should be accepted");
        }

        for rejected in [
            "yesterday",
            "2026-09-10",
            "2026-09-10T00:00:00",
            "2026-09-10T00:00:00.Z",
            "2026-09-10T00:00:00+0800",
            "2026-13-10T00:00:00Zextra",
        ] {
            manifest.created_at = rejected.to_owned();
            assert!(
                matches!(
                    manifest.validate(),
                    Err(ProjectManifestError::InvalidField("created_at"))
                ),
                "{rejected} should be rejected"
            );
        }
    }

    #[test]
    fn open_does_not_read_the_declared_source_file() {
        let temp = TempDir::new();
        let target = temp.path.join("film");
        CanonicalProject::create(&target, manifest(), b"source bytes\n").unwrap();

        // A source that is a directory rather than a file must be refused by
        // path resolution alone; no read of its contents is possible.
        fs::remove_file(target.join("source/story.txt")).unwrap();
        fs::create_dir(target.join("source/story.txt")).unwrap();

        assert!(matches!(
            CanonicalProject::open(&target),
            Err(ProjectStoreError::Fs(ProjectFsError::NotFile(_)))
        ));
    }

    #[test]
    fn second_read_write_session_is_refused_until_the_first_closes() {
        let temp = TempDir::new();
        let target = temp.path.join("film");
        let first = CanonicalProject::create(&target, manifest(), b"source\n").unwrap();

        assert!(matches!(
            CanonicalProject::open(&target),
            Err(ProjectStoreError::Busy(_))
        ));

        drop(first);
        let reopened = CanonicalProject::open(&target).unwrap();
        assert_eq!(reopened.access(), ProjectAccess::ReadWrite);
    }

    #[test]
    fn read_only_sessions_do_not_take_the_writer_lock() {
        let temp = TempDir::new();
        let target = temp.path.join("film");
        let writer = CanonicalProject::create(&target, manifest(), b"source\n").unwrap();

        // A build that treats this format as older opens it read-only, and a
        // read-only session never writes, so it neither needs nor blocks the lock.
        let first_reader =
            CanonicalProject::open_with_supported_version_for_test(&target, 2).unwrap();
        let second_reader =
            CanonicalProject::open_with_supported_version_for_test(&target, 2).unwrap();
        assert!(first_reader.is_read_only() && second_reader.is_read_only());
        assert!(!writer.is_read_only());
    }

    #[test]
    fn session_lock_lives_outside_canonical_state() {
        let temp = TempDir::new();
        let target = temp.path.join("film");
        let project = CanonicalProject::create(&target, manifest(), b"source\n").unwrap();

        assert!(target.join(".kineto/session.lock").is_file());
        let snapshot = project.root().canonical_snapshot().unwrap();
        assert!(
            snapshot
                .files
                .keys()
                .all(|path| !path.starts_with(".kineto"))
        );
    }

    #[test]
    fn create_refuses_to_replace_existing_directory() {
        let temp = TempDir::new();
        let target = temp.path.join("existing");
        fs::create_dir(&target).unwrap();

        assert!(matches!(
            CanonicalProject::create(&target, manifest(), b"source"),
            Err(ProjectStoreError::AlreadyExists(_))
        ));
    }
}
