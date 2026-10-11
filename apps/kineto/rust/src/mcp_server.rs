//! Kineto as an MCP server for agents outside it (ADR-0010), independent
//! of any project session: turned on and off in Settings, kept on across
//! restarts, and reading whichever project Kineto opened last.
//!
//! Each call reads or writes `mcp-server.toml` and may bind a port, so Dart
//! makes them off the UI thread.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

use kineto_mcp_server::{ProjectSlot, Server, ServerSettings};

use crate::{
    contract::{
        PROJECT_ERR_MCP_SERVER_UNAVAILABLE, PROJECT_ERR_PANIC, PROJECT_ERR_SETTING_INVALID,
        PROJECT_ERR_SETTINGS_UNWRITABLE, PROJECT_OK,
    },
    guarded,
    project_session::copy_utf8,
};

/// The project the tools read.
static PROJECT: LazyLock<ProjectSlot> = LazyLock::new(|| Arc::new(Mutex::new(None)));

/// The server while it listens, and its address.
static SERVER: Mutex<Option<(Server, String)>> = Mutex::new(None);

/// The project Kineto opens is the one outside agents read.
pub(crate) fn set_project(path: &Path) {
    if let Ok(mut project) = PROJECT.lock() {
        *project = Some(path.to_path_buf());
    }
}

fn settings_path() -> Result<PathBuf, i32> {
    ServerSettings::default_path().ok_or(PROJECT_ERR_SETTINGS_UNWRITABLE)
}

fn load(path: &Path) -> Result<ServerSettings, i32> {
    ServerSettings::load(path).map_err(|_| PROJECT_ERR_SETTING_INVALID)
}

/// Listen as `settings` say, keeping the port it got and that it is on.
fn listen(path: &Path, settings: &mut ServerSettings) -> i32 {
    let Ok(mut server) = SERVER.lock() else {
        return PROJECT_ERR_PANIC;
    };
    // The port is free again before it is asked for.
    *server = None;
    if settings.token.is_empty() && settings.renew_token().is_err() {
        return PROJECT_ERR_MCP_SERVER_UNAVAILABLE;
    }
    let Ok(listening) = Server::start(settings.port, &settings.token, Arc::clone(&PROJECT)) else {
        return PROJECT_ERR_MCP_SERVER_UNAVAILABLE;
    };
    settings.port = listening.port();
    settings.enabled = true;
    if settings.save(path).is_err() {
        return PROJECT_ERR_SETTINGS_UNWRITABLE;
    }
    *server = Some((listening, settings.address()));
    PROJECT_OK
}

fn resume(path: &Path) -> i32 {
    if SERVER.lock().is_ok_and(|server| server.is_some()) {
        return PROJECT_OK;
    }
    match load(path) {
        Ok(mut settings) if settings.enabled => listen(path, &mut settings),
        Ok(_) => PROJECT_OK,
        Err(code) => code,
    }
}

fn set_enabled(path: &Path, enabled: bool) -> i32 {
    let mut settings = match load(path) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    if enabled {
        return listen(path, &mut settings);
    }
    if let Ok(mut server) = SERVER.lock() {
        *server = None;
    }
    settings.enabled = false;
    match settings.save(path) {
        Ok(()) => PROJECT_OK,
        Err(_) => PROJECT_ERR_SETTINGS_UNWRITABLE,
    }
}

fn renew(path: &Path) -> i32 {
    let mut settings = match load(path) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    if settings.renew_token().is_err() {
        return PROJECT_ERR_MCP_SERVER_UNAVAILABLE;
    }
    settings.port = 0;
    if settings.enabled {
        return listen(path, &mut settings);
    }
    match settings.save(path) {
        Ok(()) => PROJECT_OK,
        Err(_) => PROJECT_ERR_SETTINGS_UNWRITABLE,
    }
}

fn address() -> String {
    SERVER
        .lock()
        .ok()
        .and_then(|server| server.as_ref().map(|(_, address)| address.clone()))
        .unwrap_or_default()
}

/// Listen again if the director left it on and it is not listening: as
/// Kineto starts, and when Settings ask why it is not.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_mcp_server_resume() -> i32 {
    guarded(PROJECT_ERR_PANIC, || match settings_path() {
        Ok(path) => resume(&path),
        Err(code) => code,
    })
}

/// Turn it on (1), at the address it had, or a new one the first time; or
/// off (0), keeping the address for the next time it is turned on.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_mcp_server_set_enabled(enabled: u8) -> i32 {
    guarded(PROJECT_ERR_PANIC, || match settings_path() {
        Ok(path) => set_enabled(&path, enabled != 0),
        Err(code) => code,
    })
}

/// A new address, on a port free now: the old one stops working at once.
/// Listens at it when the server is on.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_mcp_server_renew() -> i32 {
    guarded(PROJECT_ERR_PANIC, || match settings_path() {
        Ok(path) => renew(&path),
        Err(code) => code,
    })
}

/// Copy the address it listens at; empty while it is off.
///
/// # Safety
/// `out_len` must be writable; `out_buf` writable for `out_cap` bytes when
/// `out_cap` is not 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_address_copy(
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        unsafe { copy_utf8(address().as_bytes(), out_buf, out_cap, out_len) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        net::TcpStream,
    };

    /// Ask the server at `address` for its tools. Not for a project: every
    /// test that opens one names it the project the server reads, so which
    /// one that is here depends on the tests running beside this one.
    fn list_tools(address: &str) -> String {
        let rest = address.strip_prefix("http://").unwrap();
        let (host, path) = rest.split_once('/').unwrap();
        let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let mut stream = TcpStream::connect(host).unwrap();
        write!(
            stream,
            "POST /{path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
        let mut answer = String::new();
        stream.read_to_string(&mut answer).unwrap();
        answer
    }

    #[test]
    fn it_is_turned_on_and_off_and_keeps_its_address_until_a_new_one_is_asked_for() {
        let directory =
            std::env::temp_dir().join(format!("kineto-native-mcp-{}", std::process::id()));
        let path = directory
            .join("kineto")
            .join(kineto_mcp_server::SERVER_FILE);
        assert_eq!(resume(&path), PROJECT_OK, "off: nothing to resume");
        assert_eq!(address(), "");
        assert_eq!(set_enabled(&path, true), PROJECT_OK);
        let first = address();
        assert!(first.starts_with("http://127.0.0.1:"), "{first}");
        let answer = list_tools(&first);
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.contains(r#""name":"get_shot""#), "{answer}");

        assert_eq!(set_enabled(&path, false), PROJECT_OK);
        assert_eq!(address(), "");
        assert!(!ServerSettings::load(&path).unwrap().enabled);
        assert_eq!(set_enabled(&path, true), PROJECT_OK);
        assert_eq!(address(), first, "the same address when on again");

        // Resuming what listens changes nothing; as Kineto starts again, it
        // is on at the same address.
        assert_eq!(resume(&path), PROJECT_OK);
        assert_eq!(address(), first);
        *SERVER.lock().unwrap() = None;
        assert_eq!(resume(&path), PROJECT_OK);
        assert_eq!(address(), first);

        assert_eq!(renew(&path), PROJECT_OK);
        let second = address();
        assert!(!second.is_empty() && second != first);
        assert_eq!(set_enabled(&path, false), PROJECT_OK);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
