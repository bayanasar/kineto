//! Run one operation of a plugin.
//!
//! The plugin starts in a fresh scratch directory outside any project, with
//! the files the request needs copied under `inputs/` and an empty
//! `outputs/` for what it makes. Its environment is cleared: it gets a
//! minimal `PATH`, `HOME` and `KINETO_PLUGIN_DIR` set to the scratch
//! directory, and the secrets its entry names, nothing else. It reads one
//! request line and closes; Kineto reads at most [`Limits::max_answer`]
//! bytes of its stdout and discards its stderr. Past the deadline it is
//! killed. A file it answers with must lie under `outputs/`, and hold the
//! size and SHA-256 it declared, within [`Limits::max_file`]. The directory
//! is removed when the call ends.

use std::{
    fmt, fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    config::PluginEntry,
    protocol::{self, Answer, DeclaredFile},
};

/// How long and how much an operation may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub deadline: Duration,
    /// The most bytes of the answer read from stdout.
    pub max_answer: usize,
    /// The largest file an answer may name.
    pub max_file: u64,
}

impl Limits {
    /// For `describe`, `estimate`, `voices` and `reconcile`.
    pub const QUICK: Self = Self {
        deadline: Duration::from_secs(30),
        max_answer: 1024 * 1024,
        max_file: 64 * 1024 * 1024,
    };

    /// For `invoke`.
    pub const INVOKE: Self = Self {
        deadline: Duration::from_secs(15 * 60),
        max_answer: 1024 * 1024,
        max_file: 64 * 1024 * 1024,
    };
}

/// A file a request needs, under `inputs/` by `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub name: String,
    pub bytes: Vec<u8>,
}

/// A checked file of an answer.
#[derive(Clone, PartialEq, Eq)]
pub struct OutputFile {
    pub declared: DeclaredFile,
    pub bytes: Vec<u8>,
}

impl fmt::Debug for OutputFile {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OutputFile({}, {} bytes)",
            self.declared.path,
            self.bytes.len()
        )
    }
}

/// An operation's answer, and its files when it completed.
#[derive(Debug, Clone, PartialEq)]
pub struct Done {
    pub answer: Answer,
    pub files: Vec<OutputFile>,
}

#[derive(Debug)]
pub enum HostError {
    /// The plugin could not be started; it never saw the request.
    Spawn(std::io::Error),
    /// The scratch directory could not be prepared; nothing was started.
    Setup(std::io::Error),
    /// It ran past its deadline and was killed.
    Timeout,
    /// It exited without a readable answer.
    Exited(Option<i32>),
    /// Its answer was too long, not JSON, or not this protocol.
    Unreadable(String),
    /// A file it answered with is missing, outside `outputs/`, too large, or
    /// not what it declared.
    Output(String),
}

impl HostError {
    /// Whether the plugin certainly never received the request.
    #[must_use]
    pub const fn unsent(&self) -> bool {
        matches!(self, Self::Spawn(_) | Self::Setup(_))
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(error) => write!(formatter, "the plugin could not be started: {error}"),
            Self::Setup(error) => write!(
                formatter,
                "the plugin's directory could not be prepared: {error}"
            ),
            Self::Timeout => {
                formatter.write_str("the plugin ran past its deadline and was stopped")
            }
            Self::Exited(Some(code)) => write!(
                formatter,
                "the plugin exited with status {code} and no answer"
            ),
            Self::Exited(None) => formatter.write_str("the plugin was killed before it answered"),
            Self::Unreadable(reason) | Self::Output(reason) => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for HostError {}

/// The scratch directory of one call, removed when dropped.
pub(crate) struct Scratch(pub(crate) PathBuf);

impl Scratch {
    pub(crate) fn new() -> std::io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let path = std::env::temp_dir().join(format!(
            "kineto-plugin-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("inputs"))?;
        fs::create_dir_all(path.join("outputs"))?;
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A file name that stays in its directory.
fn plain_name(name: &str) -> bool {
    let path = Path::new(name);
    !name.is_empty()
        && path.components().count() == 1
        && matches!(path.components().next(), Some(Component::Normal(_)))
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The minimal search path a plugin is given.
pub(crate) fn search_path() -> &'static str {
    if cfg!(windows) {
        r"C:\Windows\System32;C:\Windows"
    } else {
        "/usr/local/bin:/usr/bin:/bin"
    }
}

/// Run `operation` of `entry` with `fields` beside the protocol, `inputs`
/// copied in, and the secrets `secret` gives for the names the entry lists.
pub fn run(
    entry: &PluginEntry,
    secret: &dyn Fn(&str) -> Option<String>,
    operation: &str,
    fields: Value,
    inputs: &[Input],
    limits: Limits,
) -> Result<Done, HostError> {
    let scratch = Scratch::new().map_err(HostError::Setup)?;
    let mut listed = Vec::with_capacity(inputs.len());
    for input in inputs {
        if !plain_name(&input.name) {
            return Err(HostError::Setup(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{:?} is not a file name", input.name),
            )));
        }
        fs::write(scratch.0.join("inputs").join(&input.name), &input.bytes)
            .map_err(HostError::Setup)?;
        listed.push(serde_json::json!({
            "path": format!("inputs/{}", input.name),
            "size": input.bytes.len(),
            "sha256": sha256(&input.bytes),
        }));
    }
    let mut request = protocol::request(operation, fields);
    if let Some(object) = request.as_object_mut() {
        object.insert("inputs".to_owned(), Value::Array(listed));
    }
    let mut line = serde_json::to_vec(&request).map_err(|error| HostError::Setup(error.into()))?;
    line.push(b'\n');

    let mut command = Command::new(&entry.executable);
    command
        .args(&entry.args)
        .env_clear()
        .env("PATH", search_path())
        .env("HOME", &scratch.0)
        .env("KINETO_PLUGIN_DIR", &scratch.0)
        .current_dir(&scratch.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for name in &entry.secrets {
        if let Some(value) = secret(name) {
            command.env(name, value);
        }
    }
    let mut child = command.spawn().map_err(HostError::Spawn)?;
    if let Some(mut stdin) = child.stdin.take() {
        // A plugin that exits without reading is judged by its answer.
        let _ = stdin.write_all(&line);
    }
    let mut stdout = child.stdout.take().ok_or(HostError::Exited(None))?;
    let cap = limits.max_answer;
    let reader = thread::spawn(move || {
        let mut answer = Vec::new();
        let read = (&mut stdout)
            .take(u64::try_from(cap).unwrap_or(u64::MAX).saturating_add(1))
            .read_to_end(&mut answer);
        read.map(|_| answer)
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= limits.deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(HostError::Timeout);
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                let _ = child.kill();
                return Err(HostError::Exited(None));
            }
        }
    };
    let answer = reader
        .join()
        .ok()
        .and_then(Result::ok)
        .ok_or(HostError::Exited(status.code()))?;
    if answer.len() > cap {
        return Err(HostError::Unreadable(format!(
            "the plugin answered with more than {cap} bytes"
        )));
    }
    let text = String::from_utf8(answer)
        .map_err(|_| HostError::Unreadable("the answer is not UTF-8".to_owned()))?;
    let Some(first) = text.lines().find(|line| !line.trim().is_empty()) else {
        return Err(HostError::Exited(status.code()));
    };
    let answer = protocol::answer(first).map_err(HostError::Unreadable)?;
    let files = match &answer {
        Answer::Completed { outputs, .. } => outputs
            .iter()
            .map(|declared| read_output(&scratch.0, declared, limits.max_file))
            .collect::<Result<Vec<_>, _>>()?,
        _ => Vec::new(),
    };
    Ok(Done { answer, files })
}

/// The file `declared` names, checked against what it declares.
fn read_output(scratch: &Path, declared: &DeclaredFile, max: u64) -> Result<OutputFile, HostError> {
    let relative = Path::new(&declared.path);
    let mut components = relative.components();
    let inside = matches!(components.next(), Some(Component::Normal(first)) if first == "outputs")
        && components.clone().count() > 0
        && components.all(|component| matches!(component, Component::Normal(_)));
    if !inside {
        return Err(HostError::Output(format!(
            "{:?} is not a file under outputs/",
            declared.path
        )));
    }
    if declared.size > max {
        return Err(HostError::Output(format!(
            "{} declares {} bytes; at most {max} are taken",
            declared.path, declared.size
        )));
    }
    let path = scratch.join(relative);
    let metadata = fs::symlink_metadata(&path)
        .map_err(|_| HostError::Output(format!("{} is missing", declared.path)))?;
    if !metadata.is_file() {
        return Err(HostError::Output(format!(
            "{} is not a plain file",
            declared.path
        )));
    }
    if metadata.len() != declared.size {
        return Err(HostError::Output(format!(
            "{} holds {} bytes, not the {} declared",
            declared.path,
            metadata.len(),
            declared.size
        )));
    }
    let bytes = fs::read(&path)
        .map_err(|_| HostError::Output(format!("{} is unreadable", declared.path)))?;
    if sha256(&bytes) != declared.sha256.trim_start_matches("sha256:") {
        return Err(HostError::Output(format!(
            "{} is not the file declared",
            declared.path
        )));
    }
    Ok(OutputFile {
        declared: declared.clone(),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_and_files_under_outputs_are_taken() {
        assert!(plain_name("identity-0.jpg"));
        for name in ["", "../x", "a/b", "/etc/passwd", "."] {
            assert!(!plain_name(name), "{name}");
        }
        let scratch = Scratch::new().unwrap();
        fs::write(scratch.0.join("outputs/image.jpg"), b"jpeg").unwrap();
        let good = DeclaredFile {
            path: "outputs/image.jpg".to_owned(),
            size: 4,
            sha256: sha256(b"jpeg"),
        };
        assert_eq!(read_output(&scratch.0, &good, 10).unwrap().bytes, b"jpeg");
        for (path, size, hash, why) in [
            (
                "../escape.jpg",
                4,
                sha256(b"jpeg"),
                "not a file under outputs/",
            ),
            (
                "inputs/image.jpg",
                4,
                sha256(b"jpeg"),
                "not a file under outputs/",
            ),
            ("outputs", 4, sha256(b"jpeg"), "not a file under outputs/"),
            ("outputs/missing.jpg", 4, sha256(b"jpeg"), "is missing"),
            (
                "outputs/image.jpg",
                5,
                sha256(b"jpeg"),
                "not the 5 declared",
            ),
            (
                "outputs/image.jpg",
                4,
                sha256(b"png!"),
                "not the file declared",
            ),
            ("outputs/image.jpg", 40, sha256(b"jpeg"), "at most 10"),
        ] {
            let declared = DeclaredFile {
                path: path.to_owned(),
                size,
                sha256: hash,
            };
            let error = read_output(&scratch.0, &declared, 10)
                .unwrap_err()
                .to_string();
            assert!(error.contains(why), "{path}: {error}");
        }
        let directory = scratch.0.clone();
        drop(scratch);
        assert!(
            !directory.exists(),
            "the scratch directory goes with the call"
        );
    }
}
