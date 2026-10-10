//! What every agent call shares: an empty working directory of its own, and
//! a process run to its end within a deadline that the caller can cut short.

use std::{
    ffi::OsString,
    fs,
    io::{self, BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

use crate::acp::AgentError;

/// How often a waiting call checks whether it was cancelled.
pub(crate) const CANCEL_CHECK: Duration = Duration::from_millis(100);
/// How much of what an agent writes to its error stream is kept, from the
/// end, to say why it failed.
const STDERR_TAIL: usize = 4096;
/// How long, after an agent exits, the rest of its output is waited for.
const EXIT_GRACE: Duration = Duration::from_secs(2);

pub(crate) fn is_cancelled(cancel: Option<&Arc<AtomicBool>>) -> bool {
    cancel.is_some_and(|cancel| cancel.load(Ordering::SeqCst))
}

/// An empty directory for one call, removed when dropped. Agents copy the
/// director's references into it, so only its owner may enter it.
pub(crate) struct Workspace(pub(crate) PathBuf);

impl Workspace {
    /// Create the directory under `root` with a random name that must not
    /// exist yet, readable only by this user: mode 0700 on Unix; on Windows
    /// it inherits the ACL of the user's own temporary directory.
    pub(crate) fn create(root: &Path) -> Result<Self, AgentError> {
        fs::create_dir_all(root).map_err(|error| AgentError::Setup(error.kind()))?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("kineto-agent-");
        #[cfg(unix)]
        builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o700));
        let directory = builder
            .tempdir_in(root)
            .map_err(|error| AgentError::Setup(error.kind()))?;
        Ok(Self(directory.keep()))
    }

    /// Write `contents` at `relative` inside the workspace.
    pub(crate) fn write(&self, relative: &str, contents: &[u8]) -> Result<PathBuf, AgentError> {
        let path = self.0.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| AgentError::Setup(error.kind()))?;
        }
        fs::write(&path, contents).map_err(|error| AgentError::Setup(error.kind()))?;
        Ok(path)
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The search path an agent is started with: its own directory first, so a
/// CLI installed beside its runtime (a Node script beside `node`) finds it
/// even when the desktop app was started without the user's shell path.
pub(crate) fn search_path(executable: &Path, extra: &[PathBuf]) -> Option<OsString> {
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let directories = executable
        .parent()
        .map(Path::to_path_buf)
        .into_iter()
        .chain(extra.iter().cloned())
        .chain(std::env::split_paths(&inherited));
    std::env::join_paths(directories).ok()
}

/// What a process wrote and how it ended.
pub(crate) struct Finished {
    pub(crate) lines: Vec<String>,
    /// The end of what it wrote to its error stream.
    pub(crate) stderr: String,
    pub(crate) status: ExitStatus,
}

/// Run `command` in `workspace` with `stdin` as its whole input, until it
/// exits, `deadline` passes or `cancel` is set; a process that overruns or
/// is cancelled is stopped.
pub(crate) fn run_to_end(
    command: &mut Command,
    workspace: &Workspace,
    stdin: Option<Vec<u8>>,
    deadline: Instant,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Finished, AgentError> {
    let mut child = command
        .current_dir(&workspace.0)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| AgentError::Spawn(error.kind()))?;
    if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
        // An agent that stops reading early is judged by its answer.
        thread::Builder::new()
            .name("kineto-agent-writer".to_owned())
            .spawn(move || {
                let _ = pipe.write_all(&input);
            })
            .map_err(|error| AgentError::Spawn(error.kind()))?;
    }
    let stdout = child
        .stdout
        .take()
        .ok_or(AgentError::Spawn(io::ErrorKind::Other))?;
    let (sender, lines) = mpsc::channel();
    thread::Builder::new()
        .name("kineto-agent-reader".to_owned())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if sender.send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|error| AgentError::Spawn(error.kind()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or(AgentError::Spawn(io::ErrorKind::Other))?;
    let (sender, errors) = mpsc::channel();
    thread::Builder::new()
        .name("kineto-agent-errors".to_owned())
        .spawn(move || {
            let mut tail = Vec::new();
            let mut buffer = [0_u8; 4096];
            while let Ok(read) = stderr.read(&mut buffer) {
                if read == 0 {
                    break;
                }
                tail.extend_from_slice(&buffer[..read]);
                if tail.len() > STDERR_TAIL {
                    tail.drain(..tail.len() - STDERR_TAIL);
                }
            }
            let _ = sender.send(String::from_utf8_lossy(&tail).into_owned());
        })
        .map_err(|error| AgentError::Spawn(error.kind()))?;

    let mut collected = Vec::new();
    let status = loop {
        if is_cancelled(cancel) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AgentError::Cancelled);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AgentError::Timeout);
        }
        match lines.recv_timeout(Duration::from_millis(20)) {
            Ok(line) => {
                collected.push(line);
                continue;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            // Its output closed; wait for it to exit.
            Err(mpsc::RecvTimeoutError::Disconnected) => thread::sleep(Duration::from_millis(20)),
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => return Err(AgentError::Spawn(error.kind())),
        }
    };
    // What it wrote before exiting is read to the end; a process it left
    // behind holding the pipe open is not waited for.
    let grace = Instant::now() + EXIT_GRACE;
    while let Some(left) = grace.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(left) {
            Ok(line) => collected.push(line),
            Err(_) => break,
        }
    }
    let stderr = errors
        .recv_timeout(grace.saturating_duration_since(Instant::now()))
        .unwrap_or_default();
    Ok(Finished {
        lines: collected,
        stderr,
        status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_is_new_private_and_removed() {
        let first = Workspace::create(&std::env::temp_dir()).unwrap();
        let second = Workspace::create(&std::env::temp_dir()).unwrap();
        assert_ne!(first.0, second.0);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&first.0).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "{mode:o}");
        }
        let reference = first.write("references/portrait.png", b"png").unwrap();
        assert!(reference.starts_with(&first.0));
        let path = first.0.clone();
        drop(first);
        assert!(!path.exists());
    }
}
