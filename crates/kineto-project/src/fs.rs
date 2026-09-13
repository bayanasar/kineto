use std::{
    collections::BTreeMap,
    error::Error,
    fmt, fs,
    fs::OpenOptions,
    io::Write,
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static WRITE_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectRelativePath(PathBuf);

impl ProjectRelativePath {
    pub fn new(path: impl Into<PathBuf>) -> Result<Self, ProjectPathError> {
        let path = path.into();
        let text = path.to_str().ok_or(ProjectPathError::NonUtf8)?;

        // Canonical project paths are portable UTF-8 paths using `/` as the
        // separator. Reject alternate separators and Windows-invalid filename
        // characters even when running on Unix so a project cannot become
        // platform-dependent by accident.
        if text.is_empty() || text.starts_with('/') || text.contains('\\') {
            return Err(ProjectPathError::Invalid);
        }

        let mut saw_segment = false;
        for segment in text.split('/') {
            if segment.is_empty() || matches!(segment, "." | "..") {
                return Err(ProjectPathError::Invalid);
            }
            if segment.chars().any(|character| {
                character.is_control()
                    || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
            }) {
                return Err(ProjectPathError::Invalid);
            }
            saw_segment = true;
        }

        if !saw_segment || path.is_absolute() {
            return Err(ProjectPathError::Invalid);
        }

        // Keep an OS-level check as defense in depth. This catches prefixes
        // such as `C:` on Windows even if the textual rules evolve later.
        if path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        }) {
            return Err(ProjectPathError::Invalid);
        }

        Ok(Self(path))
    }

    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectPathError {
    Invalid,
    NonUtf8,
}

impl fmt::Display for ProjectPathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Invalid => {
                "project path must be a portable, non-empty, traversal-free relative path"
            }
            Self::NonUtf8 => "project paths must be valid UTF-8",
        })
    }
}

impl Error for ProjectPathError {}

#[derive(Debug, Clone)]
pub struct ProjectRoot {
    root: PathBuf,
}

impl ProjectRoot {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, ProjectFsError> {
        let root = fs::canonicalize(path).map_err(ProjectFsError::Io)?;
        if !root.is_dir() {
            return Err(ProjectFsError::NotDirectory(root));
        }
        Ok(Self { root })
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn resolve(&self, path: &ProjectRelativePath) -> PathBuf {
        self.root.join(path.as_path())
    }

    pub fn read(&self, path: &ProjectRelativePath) -> Result<Vec<u8>, ProjectFsError> {
        let resolved = self.resolve_existing(path)?;
        fs::read(resolved).map_err(ProjectFsError::Io)
    }

    /// Replace one canonical project file without truncating its published path.
    ///
    /// Bytes are written to a reserved sibling temp file, fsynced, published with
    /// a same-directory rename, then the parent directory is flushed. A crash can
    /// leave an unpublished temp file, but never a partially truncated canonical
    /// file. Reserved temp files are excluded from canonical snapshots.
    pub fn write_atomic(
        &self,
        path: &ProjectRelativePath,
        bytes: &[u8],
    ) -> Result<(), ProjectFsError> {
        let resolved = self.resolve(path);
        let parent = resolved.parent().ok_or_else(|| {
            ProjectFsError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "canonical file has no parent directory",
            ))
        })?;
        ensure_directory(self, parent)?;

        match fs::symlink_metadata(&resolved) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(ProjectFsError::Symlink(resolved));
                }
                if !metadata.is_file() {
                    return Err(ProjectFsError::NotFile(resolved));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ProjectFsError::Io(error)),
        }

        for _ in 0..32 {
            let nonce = WRITE_NONCE.fetch_add(1, Ordering::Relaxed);
            let temp = parent.join(format!(".kineto-write-{}-{nonce}.tmp", std::process::id()));
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
                Ok(file) => file,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(ProjectFsError::Io(error)),
            };

            let result = (|| -> std::io::Result<()> {
                file.write_all(bytes)?;
                file.sync_all()?;
                drop(file);
                fs::rename(&temp, &resolved)?;
                sync_directory_impl(parent)
            })();

            if let Err(error) = result {
                let _ = fs::remove_file(&temp);
                return Err(ProjectFsError::Io(error));
            }
            return Ok(());
        }

        Err(ProjectFsError::Io(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not allocate a canonical write temp file",
        )))
    }

    /// Prove that a project-relative path resolves to a regular file inside the
    /// boundary without reading its contents.
    ///
    /// Opening a project must validate that its declared source resolves; it
    /// must not pay for the source's size to do so.
    pub fn ensure_file(&self, path: &ProjectRelativePath) -> Result<(), ProjectFsError> {
        let resolved = self.resolve_existing(path)?;
        let metadata = fs::metadata(&resolved).map_err(ProjectFsError::Io)?;
        if metadata.is_file() {
            Ok(())
        } else {
            Err(ProjectFsError::NotFile(resolved))
        }
    }

    /// Flush this directory's own entries so a rename or creation performed
    /// inside it survives a crash. Unix only; other platforms rely on the
    /// filesystem's own ordering guarantees.
    pub fn sync_directory(path: &Path) -> Result<(), ProjectFsError> {
        sync_directory_impl(path).map_err(ProjectFsError::Io)
    }

    pub fn canonical_snapshot(&self) -> Result<CanonicalSnapshot, ProjectFsError> {
        let mut files = BTreeMap::new();
        scan_directory(&self.root, &self.root, &mut files)?;
        Ok(CanonicalSnapshot { files })
    }

    fn resolve_existing(&self, path: &ProjectRelativePath) -> Result<PathBuf, ProjectFsError> {
        let mut current = self.root.clone();

        for component in path.as_path().components() {
            let Component::Normal(segment) = component else {
                return Err(ProjectFsError::EscapedRoot(self.resolve(path)));
            };
            current.push(segment);

            // `canonicalize` alone would follow a symlink and only tell us the
            // final path escaped. Reject every symlink component explicitly so
            // project reads have the same policy as canonical scans.
            let metadata = fs::symlink_metadata(&current).map_err(ProjectFsError::Io)?;
            if metadata.file_type().is_symlink() {
                return Err(ProjectFsError::Symlink(current));
            }
        }

        let canonical = fs::canonicalize(&current).map_err(ProjectFsError::Io)?;
        if !canonical.starts_with(&self.root) {
            return Err(ProjectFsError::EscapedRoot(canonical));
        }
        Ok(canonical)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CanonicalSnapshot {
    pub files: BTreeMap<PathBuf, Vec<u8>>,
}

#[derive(Debug)]
pub enum ProjectFsError {
    Io(std::io::Error),
    NotDirectory(PathBuf),
    NotFile(PathBuf),
    Symlink(PathBuf),
    EscapedRoot(PathBuf),
}

impl fmt::Display for ProjectFsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "project filesystem error: {error}"),
            Self::NotDirectory(path) => {
                write!(
                    formatter,
                    "project root is not a directory: {}",
                    path.display()
                )
            }
            Self::NotFile(path) => {
                write!(
                    formatter,
                    "project path is not a regular file: {}",
                    path.display()
                )
            }
            Self::Symlink(path) => {
                write!(
                    formatter,
                    "project filesystem boundary refuses symlink: {}",
                    path.display()
                )
            }
            Self::EscapedRoot(path) => {
                write!(formatter, "project access escaped root: {}", path.display())
            }
        }
    }
}

impl Error for ProjectFsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::NotDirectory(_) | Self::NotFile(_) | Self::Symlink(_) | Self::EscapedRoot(_) => {
                None
            }
        }
    }
}

#[cfg(unix)]
fn sync_directory_impl(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory_impl(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn ensure_directory(root: &ProjectRoot, directory: &Path) -> Result<(), ProjectFsError> {
    let relative = directory
        .strip_prefix(root.root())
        .map_err(|_| ProjectFsError::EscapedRoot(directory.to_path_buf()))?;
    let mut current = root.root().to_path_buf();

    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(ProjectFsError::EscapedRoot(directory.to_path_buf()));
        };
        current.push(segment);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(ProjectFsError::Symlink(current));
                }
                if !metadata.is_dir() {
                    return Err(ProjectFsError::NotDirectory(current));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(ProjectFsError::Io)?;
            }
            Err(error) => return Err(ProjectFsError::Io(error)),
        }
    }
    Ok(())
}

fn scan_directory(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
) -> Result<(), ProjectFsError> {
    for entry in fs::read_dir(directory).map_err(ProjectFsError::Io)? {
        let entry = entry.map_err(ProjectFsError::Io)?;
        let path = entry.path();
        let file_type = entry.file_type().map_err(ProjectFsError::Io)?;

        if entry.file_name() == ".kineto"
            || entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with(".kineto-write-"))
        {
            continue;
        }
        if file_type.is_symlink() {
            return Err(ProjectFsError::Symlink(path));
        }
        if file_type.is_dir() {
            scan_directory(root, &path, files)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .map_err(|_| ProjectFsError::EscapedRoot(path.clone()))?
            .to_path_buf();
        let bytes = fs::read(&path).map_err(ProjectFsError::Io)?;
        files.insert(relative, bytes);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new() -> Self {
            let nonce = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "kineto-project-test-{}-{nonce}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn project_relative_path_is_cross_platform_and_traversal_free() {
        assert!(ProjectRelativePath::new("source/story.txt").is_ok());
        assert!(ProjectRelativePath::new("characters/爱丽丝/selection.json").is_ok());
        assert!(ProjectRelativePath::new("../secret").is_err());
        assert!(ProjectRelativePath::new("scene/../../secret").is_err());
        assert!(ProjectRelativePath::new("./source").is_err());
        assert!(ProjectRelativePath::new("source//story.txt").is_err());
        assert!(ProjectRelativePath::new("C:/secret.txt").is_err());
        assert!(ProjectRelativePath::new(r"..\secret").is_err());
        assert!(ProjectRelativePath::new("").is_err());
        assert!(ProjectRelativePath::new(std::env::temp_dir()).is_err());
    }

    #[test]
    fn deleting_dot_kineto_does_not_change_canonical_snapshot() {
        let temp = TempProject::new();
        fs::create_dir_all(temp.path.join("characters/alice")).unwrap();
        fs::create_dir_all(temp.path.join(".kineto")).unwrap();
        fs::write(
            temp.path.join("characters/alice/selection.json"),
            br#"{"selected_artifact_id":"alice_candidate_001"}"#,
        )
        .unwrap();
        fs::write(
            temp.path.join("characters/alice/artifact.json"),
            br#"{"artifact_id":"alice_candidate_001","status":"locked"}"#,
        )
        .unwrap();
        fs::write(temp.path.join(".kineto/project.db"), b"derived-index").unwrap();

        let before = ProjectRoot::open(&temp.path)
            .unwrap()
            .canonical_snapshot()
            .unwrap();
        fs::remove_dir_all(temp.path.join(".kineto")).unwrap();
        let after = ProjectRoot::open(&temp.path)
            .unwrap()
            .canonical_snapshot()
            .unwrap();

        assert_eq!(before, after);
        assert!(
            after
                .files
                .contains_key(Path::new("characters/alice/selection.json"))
        );
        assert!(after.files.values().any(|bytes| {
            bytes
                .windows(b"locked".len())
                .any(|window| window == b"locked")
        }));
    }

    #[cfg(unix)]
    #[test]
    fn canonical_scan_and_direct_read_both_refuse_symlink_escape() {
        use std::os::unix::fs::symlink;

        let temp = TempProject::new();
        let outside = TempProject::new();
        fs::write(outside.path.join("secret.txt"), b"secret").unwrap();
        symlink(&outside.path, temp.path.join("escape")).unwrap();

        let root = ProjectRoot::open(&temp.path).unwrap();
        let scan_error = root.canonical_snapshot().unwrap_err();
        assert!(matches!(scan_error, ProjectFsError::Symlink(_)));

        let read_path = ProjectRelativePath::new("escape/secret.txt").unwrap();
        let read_error = root.read(&read_path).unwrap_err();
        assert!(matches!(read_error, ProjectFsError::Symlink(_)));
    }

    #[test]
    fn atomic_write_replaces_published_bytes_and_creates_parents() {
        let temp = TempProject::new();
        let root = ProjectRoot::open(&temp.path).unwrap();
        let path = ProjectRelativePath::new("scenes/scene_001/shot.json").unwrap();

        root.write_atomic(&path, b"old\n").unwrap();
        assert_eq!(root.read(&path).unwrap(), b"old\n");

        root.write_atomic(&path, b"new canonical bytes\n").unwrap();
        assert_eq!(root.read(&path).unwrap(), b"new canonical bytes\n");
    }

    #[test]
    fn canonical_scan_ignores_interrupted_atomic_write_temp_files() {
        let temp = TempProject::new();
        fs::write(temp.path.join("project.toml"), b"canonical").unwrap();
        fs::write(temp.path.join(".kineto-write-999-1.tmp"), b"unpublished").unwrap();

        let snapshot = ProjectRoot::open(&temp.path)
            .unwrap()
            .canonical_snapshot()
            .unwrap();
        assert_eq!(snapshot.files.len(), 1);
        assert_eq!(snapshot.files[Path::new("project.toml")], b"canonical");
    }
}
