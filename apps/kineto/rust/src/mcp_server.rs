//! Kineto as an MCP server for agents outside it (ADR-0010, ADR-0011),
//! independent of any project session: turned on and off in Settings, kept
//! on across restarts, reading whichever project Kineto opened last and
//! changing it through its session while it is open.
//!
//! Agents on this computer connect to its local address. Agents in the
//! cloud connect through the director's tunnel to a second listener, which
//! takes OAuth tokens only; while it is on, Kineto runs the tunnel command
//! the director gave, and stops it when it is turned off.
//!
//! Calls that change the settings read and write `mcp-server.toml` and may
//! bind a port or start a program, so Dart makes them off the UI thread.
//! The inbox and the director's decisions only touch memory.

use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    ptr,
    sync::{
        Arc, LazyLock, Mutex, MutexGuard,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use kineto_core::session::ProjectSession;
use kineto_mcp_server::{
    AGENTS_FILE, Agents, ConnectedAgent, Desk, PublicServer, Server, ServerSettings, Waiting,
    public_origin, tunnel_words,
};

use crate::{
    contract::{
        AGENT_ASK_TEXTS, AGENT_ASK_VALUES, PROJECT_ERR_INVALID_ARGUMENT,
        PROJECT_ERR_MCP_PUBLIC_INVALID, PROJECT_ERR_MCP_SERVER_UNAVAILABLE, PROJECT_ERR_PANIC,
        PROJECT_ERR_SETTING_INVALID, PROJECT_ERR_SETTINGS_UNWRITABLE, PROJECT_OK, PUBLIC_TEXTS,
        PUBLIC_VALUES, name,
    },
    documents::at,
    guarded,
    project_session::{copy_utf8, utf8_owned},
};

/// The project agents reach, and what they asked.
static DESK: LazyLock<Arc<Desk>> = LazyLock::new(|| Arc::new(Desk::new()));

/// The session handle the desk writes through, by address; 0 for none.
static OPEN: Mutex<usize> = Mutex::new(0);

/// The server for agents on this computer while it listens, and its
/// address.
static LOCAL: Mutex<Option<(Server, String)>> = Mutex::new(None);

/// The server for agents in the cloud while it listens, and its tunnel.
static PUBLIC: Mutex<Option<Public>> = Mutex::new(None);

/// The agents allowed to connect from the internet, and the file they are
/// kept in.
static AGENTS: Mutex<Option<(PathBuf, Arc<Agents>)>> = Mutex::new(None);

/// How many of the tunnel's last words are kept.
const TUNNEL_WORDS: usize = 600;
/// How often the tunnel's watcher looks whether it ended or must stop.
const TUNNEL_WATCH: Duration = Duration::from_millis(200);
/// How long a tunnel asked to end may take before it is ended.
const TUNNEL_GRACE: Duration = Duration::from_secs(3);

struct Public {
    server: PublicServer,
    tunnel: Option<Tunnel>,
    /// Why the tunnel did not start.
    problem: Option<String>,
}

/// The director's tunnel command, running.
///
/// A thread of its own starts it and watches it for as long as it runs. On
/// Linux the kernel stops a child when the thread that started it ends, so
/// it is started by that thread, not by the short-lived thread that asked
/// for it; when Kineto ends, however it ends, the tunnel ends too.
struct Tunnel {
    /// What it last said on its error stream.
    said: Arc<Mutex<String>>,
    ended: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    watcher: Option<thread::JoinHandle<()>>,
}

impl Tunnel {
    fn start(words: &[String]) -> std::io::Result<Self> {
        let (program, arguments) = words
            .split_first()
            .ok_or_else(|| std::io::Error::other("no command"))?;
        let mut command = Command::new(program);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        #[cfg(target_os = "linux")]
        // SAFETY: `prctl` is async-signal-safe. It asks the kernel to stop
        // the tunnel when the thread that starts it, its watcher, ends.
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let said = Arc::new(Mutex::new(String::new()));
        let ended = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let (started, outcome) = mpsc::channel();
        let watcher = thread::Builder::new()
            .name("kineto-mcp-tunnel".to_owned())
            .spawn({
                let (said, ended, stop) =
                    (Arc::clone(&said), Arc::clone(&ended), Arc::clone(&stop));
                move || {
                    let mut child = match command.spawn() {
                        Ok(child) => child,
                        Err(error) => {
                            ended.store(true, Ordering::SeqCst);
                            let _ = started.send(Err(error));
                            return;
                        }
                    };
                    if let Some(stderr) = child.stderr.take() {
                        let _ = thread::Builder::new()
                            .name("kineto-mcp-tunnel-said".to_owned())
                            .spawn(move || {
                                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                                    let line = line.trim();
                                    if !line.is_empty() {
                                        *lock(&said) = line.chars().take(TUNNEL_WORDS).collect();
                                    }
                                }
                            });
                    }
                    let _ = started.send(Ok(()));
                    while !stop.load(Ordering::SeqCst) {
                        if !matches!(child.try_wait(), Ok(None)) {
                            break;
                        }
                        thread::sleep(TUNNEL_WATCH);
                    }
                    stop_gently(&mut child);
                    ended.store(true, Ordering::SeqCst);
                }
            })?;
        let mut tunnel = Self {
            said,
            ended,
            stop,
            watcher: Some(watcher),
        };
        match outcome.recv() {
            Ok(Ok(())) => Ok(tunnel),
            Ok(Err(error)) => Err(error),
            Err(_) => {
                tunnel.watcher.take();
                Err(std::io::Error::other("the tunnel's watcher ended"))
            }
        }
    }

    /// Whether it still runs.
    fn running(&self) -> bool {
        !self.ended.load(Ordering::SeqCst)
    }
}

/// Ask `child` to end, so a tunnel takes down what it set up, and end it
/// if it has not within [`TUNNEL_GRACE`].
fn stop_gently(child: &mut std::process::Child) {
    if !matches!(child.try_wait(), Ok(None)) {
        return;
    }
    #[cfg(unix)]
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: `kill` only sends a signal, to a child not yet reaped.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let deadline = std::time::Instant::now() + TUNNEL_GRACE;
        while std::time::Instant::now() < deadline {
            if !matches!(child.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(TUNNEL_WATCH);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(watcher) = self.watcher.take() {
            let _ = watcher.join();
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Kineto opened the project at `path` as `handle`: agents read it and
/// change it through `session`.
pub(crate) fn opened(path: &Path, session: &ProjectSession, handle: usize) {
    DESK.open(path, session.remote());
    *lock(&OPEN) = handle;
}

/// Kineto is closing session `handle`: agents change nothing until another
/// is open.
pub(crate) fn closing(handle: usize) {
    let mut open = lock(&OPEN);
    if *open == handle {
        *open = 0;
        DESK.close();
    }
}

fn settings_path() -> Result<PathBuf, i32> {
    ServerSettings::default_path().ok_or(PROJECT_ERR_SETTINGS_UNWRITABLE)
}

fn load(path: &Path) -> Result<ServerSettings, i32> {
    ServerSettings::load(path).map_err(|_| PROJECT_ERR_SETTING_INVALID)
}

fn save(path: &Path, settings: &ServerSettings) -> i32 {
    match settings.save(path) {
        Ok(()) => PROJECT_OK,
        Err(_) => PROJECT_ERR_SETTINGS_UNWRITABLE,
    }
}

/// The agents kept beside the settings at `path`.
fn agents(path: &Path) -> Result<Arc<Agents>, i32> {
    let file = path.with_file_name(AGENTS_FILE);
    let mut agents = lock(&AGENTS);
    if let Some((known, agents)) = agents.as_ref()
        && *known == file
    {
        return Ok(Arc::clone(agents));
    }
    let loaded = Arc::new(Agents::load(&file).map_err(|_| PROJECT_ERR_SETTING_INVALID)?);
    *agents = Some((file, Arc::clone(&loaded)));
    Ok(loaded)
}

/// Listen for agents on this computer as `settings` say, keeping the port
/// it got and that it is on.
fn listen(path: &Path, settings: &mut ServerSettings) -> i32 {
    let mut local = lock(&LOCAL);
    // The port is free again before it is asked for.
    *local = None;
    if settings.token.is_empty() && settings.renew_token().is_err() {
        return PROJECT_ERR_MCP_SERVER_UNAVAILABLE;
    }
    let Ok(listening) = Server::start(settings.port, &settings.token, Arc::clone(&DESK)) else {
        return PROJECT_ERR_MCP_SERVER_UNAVAILABLE;
    };
    settings.port = listening.port();
    settings.enabled = true;
    let saved = save(path, settings);
    if saved != PROJECT_OK {
        return saved;
    }
    *local = Some((listening, settings.address()));
    PROJECT_OK
}

/// Listen for agents in the cloud as `settings` say, and start the tunnel.
fn listen_publicly(path: &Path, settings: &mut ServerSettings) -> i32 {
    let mut public = lock(&PUBLIC);
    // The tunnel and the port are let go before they are asked for again.
    *public = None;
    if public_origin(&settings.public_url).is_err() {
        return PROJECT_ERR_MCP_PUBLIC_INVALID;
    }
    let agents = match agents(path) {
        Ok(agents) => agents,
        Err(code) => return code,
    };
    let Ok(server) = PublicServer::start(
        settings.public_port,
        &settings.public_url,
        Arc::clone(&DESK),
        agents,
    ) else {
        return PROJECT_ERR_MCP_SERVER_UNAVAILABLE;
    };
    settings.public_port = server.port();
    let saved = save(path, settings);
    if saved != PROJECT_OK {
        return saved;
    }
    let (tunnel, problem) = if settings.tunnel.is_empty() {
        (None, None)
    } else {
        match Tunnel::start(&settings.tunnel_command()) {
            Ok(tunnel) => (Some(tunnel), None),
            Err(error) => (
                None,
                Some(format!("The tunnel command could not start: {error}")),
            ),
        }
    };
    *public = Some(Public {
        server,
        tunnel,
        problem,
    });
    PROJECT_OK
}

fn stop_publicly() {
    lock(&PUBLIC).take();
}

fn resume(path: &Path) -> i32 {
    let mut settings = match load(path) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    if !settings.enabled {
        return PROJECT_OK;
    }
    if lock(&LOCAL).is_none() {
        let listened = listen(path, &mut settings);
        if listened != PROJECT_OK {
            return listened;
        }
    }
    if settings.public && lock(&PUBLIC).is_none() {
        return listen_publicly(path, &mut settings);
    }
    PROJECT_OK
}

fn set_enabled(path: &Path, enabled: bool) -> i32 {
    let mut settings = match load(path) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    if enabled {
        let listened = listen(path, &mut settings);
        if listened != PROJECT_OK || !settings.public {
            return listened;
        }
        return listen_publicly(path, &mut settings);
    }
    lock(&LOCAL).take();
    stop_publicly();
    settings.enabled = false;
    save(path, &settings)
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
    save(path, &settings)
}

/// Let agents in the cloud connect at `url` through the tunnel `tunnel`
/// opens, or not; listening at once when the server is on.
fn set_public(path: &Path, public: bool, url: &str, tunnel: &[String]) -> i32 {
    let mut settings = match load(path) {
        Ok(settings) => settings,
        Err(code) => return code,
    };
    let url = if url.trim().is_empty() {
        String::new()
    } else {
        match public_origin(url) {
            Ok(url) => url,
            Err(_) => return PROJECT_ERR_MCP_PUBLIC_INVALID,
        }
    };
    let Ok(tunnel) = tunnel_words(tunnel) else {
        return PROJECT_ERR_MCP_PUBLIC_INVALID;
    };
    if public && url.is_empty() {
        return PROJECT_ERR_MCP_PUBLIC_INVALID;
    }
    stop_publicly();
    settings.public = public;
    settings.public_url = url;
    settings.tunnel = tunnel;
    if public && settings.enabled {
        return listen_publicly(path, &mut settings);
    }
    save(path, &settings)
}

fn address() -> String {
    lock(&LOCAL)
        .as_ref()
        .map(|(_, address)| address.clone())
        .unwrap_or_default()
}

/// Where agents in the cloud stand, as Settings show it. Opaque to Dart,
/// which reads it field by field.
pub struct KinetoPublicServing {
    settings: ServerSettings,
    /// The port Kineto listens on for them, while it does.
    listening: Option<u16>,
    tunnel_running: bool,
    tunnel_said: String,
    problem: Option<String>,
    agents: Vec<ConnectedAgent>,
}

fn public_state(path: &Path) -> Result<KinetoPublicServing, i32> {
    let settings = load(path)?;
    let mut public = lock(&PUBLIC);
    let (listening, tunnel_running, tunnel_said, problem) = match public.as_mut() {
        Some(public) => (
            Some(public.server.port()),
            public.tunnel.as_ref().is_some_and(Tunnel::running),
            public
                .tunnel
                .as_ref()
                .map(|tunnel| lock(&tunnel.said).clone())
                .unwrap_or_default(),
            public.problem.clone(),
        ),
        None => (None, false, String::new(), None),
    };
    drop(public);
    let agents = agents(path)
        .map(|agents| agents.connected())
        .unwrap_or_default();
    Ok(KinetoPublicServing {
        settings,
        listening,
        tunnel_running,
        tunnel_said,
        problem,
        agents,
    })
}

fn public_value(public: &KinetoPublicServing, index: u32, field: &str) -> Option<u64> {
    Some(match field {
        "public" => u64::from(public.settings.public),
        "listening" => u64::from(public.listening.is_some()),
        "tunnel_running" => u64::from(public.tunnel_running),
        "port" => u64::from(public.listening.unwrap_or(public.settings.public_port)),
        "agents" => u64::try_from(public.agents.len()).unwrap_or(u64::MAX),
        "agent_since" => at(&public.agents, index)?.since,
        _ => return None,
    })
}

fn public_text(public: &KinetoPublicServing, index: u32, field: &str) -> Option<String> {
    Some(match field {
        "url" => public.settings.public_url.clone(),
        "address" if public.listening.is_some() => public.settings.public_address(),
        "address" => String::new(),
        "tunnel" => public.settings.tunnel.join("\n"),
        "tunnel_said" => public.tunnel_said.clone(),
        "problem" => public.problem.clone().unwrap_or_default(),
        "agent_id" => at(&public.agents, index)?.id.clone(),
        "agent_name" => at(&public.agents, index)?.name.clone(),
        "agent_site" => at(&public.agents, index)?.site.clone(),
        _ => return None,
    })
}

/// What waits for the director, as it stood when read. Opaque to Dart,
/// which reads it field by field.
pub struct KinetoAgentAsks {
    asks: Vec<Waiting>,
}

fn ask_value(ask: &Waiting, field: &str) -> Option<u64> {
    Some(match field {
        "id" => ask.id,
        "connect" => u64::from(ask.kind == "connect"),
        _ => return None,
    })
}

fn ask_text(ask: &Waiting, field: &str) -> Option<String> {
    Some(match field {
        "agent" => ask.agent.clone(),
        "what" => ask.what.clone(),
        "cost" => ask.cost.clone().unwrap_or_default(),
        _ => return None,
    })
}

fn disconnect(path: &Path, id: &str) -> i32 {
    match agents(path).map(|agents| agents.disconnect(id)) {
        Ok(Ok(true)) => PROJECT_OK,
        Ok(Ok(false)) => PROJECT_ERR_INVALID_ARGUMENT,
        Ok(Err(_)) => PROJECT_ERR_SETTINGS_UNWRITABLE,
        Err(code) => code,
    }
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
/// off (0), keeping the address for the next time it is turned on. The
/// public address follows it.
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

/// Let agents in the cloud connect (1) or not (0), at the HTTPS origin
/// `url`, through the tunnel the command `tunnel` opens, one word a line
/// (`{port}` for the port it forwards to); an empty command for a tunnel
/// the director runs some other way.
///
/// # Safety
/// Each non-empty buffer must be valid for reads of its length.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_set_public(
    public: u8,
    url_ptr: *const u8,
    url_len: u64,
    tunnel_ptr: *const u8,
    tunnel_len: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let url = match unsafe { utf8_owned(url_ptr, url_len) } {
            Ok(url) => url,
            Err(code) => return code,
        };
        // SAFETY: forwarded caller contract.
        let tunnel = match unsafe { utf8_owned(tunnel_ptr, tunnel_len) } {
            Ok(tunnel) => tunnel,
            Err(code) => return code,
        };
        let words: Vec<String> = tunnel
            .lines()
            .map(str::trim)
            .filter(|word| !word.is_empty())
            .map(str::to_owned)
            .collect();
        match settings_path() {
            Ok(path) => set_public(&path, public != 0, &url, &words),
            Err(code) => code,
        }
    })
}

/// Read where agents in the cloud stand: whether they may connect, the
/// public origin and address, the tunnel command, whether Kineto listens
/// and the tunnel runs, what it last said, why it did not start, and the
/// agents the director allowed. On `PROJECT_OK` a handle is written to
/// `out_public`; it must be destroyed. Reads files, so Dart calls it off
/// the UI thread.
///
/// # Safety
/// `out_public` must point to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_public_load(
    out_public: *mut *mut KinetoPublicServing,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_public.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { ptr::write(out_public, ptr::null_mut()) };
        match settings_path().and_then(|path| public_state(&path)) {
            Ok(public) => {
                // SAFETY: as above.
                unsafe { ptr::write(out_public, Box::into_raw(Box::new(public))) };
                PROJECT_OK
            }
            Err(code) => code,
        }
    })
}

/// Number field `field` (a contract `public_value` code); agent `index`'s
/// for an agent's field.
///
/// # Safety
/// `public` must be a live handle and `out_value` writable storage for one
/// `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_public_value(
    public: *const KinetoPublicServing,
    index: u32,
    field: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(public) = (unsafe { public.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(found) = name(&PUBLIC_VALUES, field)
            .and_then(|field| public_value(public, index, field))
            .filter(|_| !out_value.is_null())
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// Copy text field `field` (a contract `public_text` code); agent
/// `index`'s for an agent's field. The tunnel command is one word a line.
///
/// # Safety
/// `public` must be a live handle; `out_len` writable; `out_buf` writable
/// for `out_cap` bytes when `out_cap` is not 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_public_text_copy(
    public: *const KinetoPublicServing,
    index: u32,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(public) = (unsafe { public.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(text) =
            name(&PUBLIC_TEXTS, field).and_then(|field| public_text(public, index, field))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller contract.
        unsafe { copy_utf8(text.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Free a handle `kineto_mcp_server_public_load` wrote. Null is ignored.
///
/// # Safety
/// `public` must be null or a handle not destroyed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_public_destroy(public: *mut KinetoPublicServing) {
    if public.is_null() {
        return;
    }
    guarded((), || {
        // SAFETY: the caller contract requires ownership of this allocation.
        unsafe { drop(Box::from_raw(public)) };
    });
}

/// Forget an agent the director allowed, by its ID: it must be allowed
/// again.
///
/// # Safety
/// When `id_len` is not 0, `id_ptr` must be valid for reads of it.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_disconnect(id_ptr: *const u8, id_len: u64) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let id = match unsafe { utf8_owned(id_ptr, id_len) } {
            Ok(id) => id,
            Err(code) => return code,
        };
        match settings_path() {
            Ok(path) => disconnect(&path, &id),
            Err(code) => code,
        }
    })
}

/// Read what waits for the director, oldest first: on `PROJECT_OK` a
/// handle is written to `out_asks`; it must be destroyed. Touches only
/// memory.
///
/// # Safety
/// `out_asks` must point to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_asks_load(out_asks: *mut *mut KinetoAgentAsks) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_asks.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let asks = Box::new(KinetoAgentAsks {
            asks: DESK.waiting(),
        });
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { ptr::write(out_asks, Box::into_raw(asks)) };
        PROJECT_OK
    })
}

/// How many requests wait; 0 for a null handle.
///
/// # Safety
/// `asks` must be null or a live handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_asks_count(asks: *const KinetoAgentAsks) -> u32 {
    guarded(0, || {
        // SAFETY: the caller guarantees a live handle or null.
        unsafe { asks.as_ref() }
            .map_or(0, |asks| u32::try_from(asks.asks.len()).unwrap_or(u32::MAX))
    })
}

/// Number field `field` (a contract `agent_ask_value` code) of request
/// `index`: its ID, and whether an agent asks to connect.
///
/// # Safety
/// `asks` must be a live handle and `out_value` writable storage for one
/// `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_asks_value(
    asks: *const KinetoAgentAsks,
    index: u32,
    field: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(asks) = (unsafe { asks.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(found) = at(&asks.asks, index)
            .zip(name(&AGENT_ASK_VALUES, field))
            .and_then(|(ask, field)| ask_value(ask, field))
            .filter(|_| !out_value.is_null())
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// Copy text field `field` (a contract `agent_ask_text` code) of request
/// `index`: the agent, what it asks, and a paid command's cost.
///
/// # Safety
/// `asks` must be a live handle; `out_len` writable; `out_buf` writable for
/// `out_cap` bytes when `out_cap` is not 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_asks_text_copy(
    asks: *const KinetoAgentAsks,
    index: u32,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(asks) = (unsafe { asks.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(text) = at(&asks.asks, index)
            .zip(name(&AGENT_ASK_TEXTS, field))
            .and_then(|(ask, field)| ask_text(ask, field))
        else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller contract.
        unsafe { copy_utf8(text.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Free a handle `kineto_mcp_server_asks_load` wrote. Null is ignored.
///
/// # Safety
/// `asks` must be null or a handle not destroyed before.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_server_asks_destroy(asks: *mut KinetoAgentAsks) {
    if asks.is_null() {
        return;
    }
    guarded((), || {
        // SAFETY: the caller contract requires ownership of this allocation.
        unsafe { drop(Box::from_raw(asks)) };
    });
}

/// The director allows (1) or declines (0) request `id`: a command then
/// runs on the open session, or an agent may connect. Refused when it no
/// longer waits.
#[unsafe(no_mangle)]
pub extern "C" fn kineto_mcp_server_decide(id: u64, allow: u8) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if DESK.decide(id, allow != 0) {
            PROJECT_OK
        } else {
            PROJECT_ERR_INVALID_ARGUMENT
        }
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

    /// GET `path` from the public listener at `port`.
    fn get(port: u16, path: &str) -> String {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: kineto.example\r\nContent-Length: 0\r\n\r\n"
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
        *LOCAL.lock().unwrap() = None;
        assert_eq!(resume(&path), PROJECT_OK);
        assert_eq!(address(), first);

        assert_eq!(renew(&path), PROJECT_OK);
        let second = address();
        assert!(!second.is_empty() && second != first);

        // Agents in the cloud: an HTTPS origin, and a tunnel that runs while
        // the public address is on.
        assert_eq!(
            set_public(&path, true, "http://kineto.example", &[]),
            PROJECT_ERR_MCP_PUBLIC_INVALID
        );
        assert_eq!(
            set_public(&path, true, "", &[]),
            PROJECT_ERR_MCP_PUBLIC_INVALID
        );
        let tunnel = ["sleep".to_owned(), "600".to_owned()];
        assert_eq!(
            set_public(&path, true, "https://kineto.example/", &tunnel),
            PROJECT_OK
        );
        let state = |field: &str| {
            let public = public_state(&path).unwrap();
            public_text(&public, 0, field)
                .or_else(|| public_value(&public, 0, field).map(|value| value.to_string()))
                .unwrap()
        };
        assert_eq!(state("url"), "https://kineto.example");
        assert_eq!(state("address"), "https://kineto.example/mcp");
        assert_eq!(state("tunnel"), "sleep\n600");
        assert_eq!(state("listening"), "1");
        assert_eq!(state("tunnel_running"), "1");
        assert_eq!(state("agents"), "0");
        // Dart asks from a thread that soon ends; the tunnel outlives it.
        let asking = path.clone();
        let asked = tunnel.clone();
        let started =
            std::thread::spawn(move || set_public(&asking, true, "https://kineto.example", &asked));
        assert_eq!(started.join().unwrap(), PROJECT_OK);
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert_eq!(
            state("tunnel_running"),
            "1",
            "the tunnel outlives the thread that asked"
        );
        let port: u16 = state("port").parse().unwrap();
        let metadata = get(port, "/.well-known/oauth-authorization-server");
        assert!(metadata.starts_with("HTTP/1.1 200"), "{metadata}");
        assert!(metadata.contains(r#""issuer":"https://kineto.example""#));

        // Off, the tunnel stops with it; on again, both come back.
        assert_eq!(set_enabled(&path, false), PROJECT_OK);
        assert_eq!(state("listening"), "0");
        assert_eq!(state("address"), "");
        assert!(TcpStream::connect(("127.0.0.1", port)).is_err());
        assert_eq!(set_enabled(&path, true), PROJECT_OK);
        assert_eq!(state("port"), port.to_string(), "the tunnel's port is kept");
        assert_eq!(state("tunnel_running"), "1");

        // A command that cannot start says so, and Kineto still listens.
        let missing = ["kineto-no-such-tunnel-program".to_owned()];
        assert_eq!(
            set_public(&path, true, "https://kineto.example", &missing),
            PROJECT_OK
        );
        assert_eq!(state("listening"), "1");
        assert!(state("problem").starts_with("The tunnel command could not start"));
        assert_eq!(state("tunnel_running"), "0");
        assert_eq!(
            set_public(&path, false, "https://kineto.example", &[]),
            PROJECT_OK
        );
        assert_eq!(state("listening"), "0");
        assert_eq!(disconnect(&path, "nobody"), PROJECT_ERR_INVALID_ARGUMENT);

        assert_eq!(set_enabled(&path, false), PROJECT_OK);
        let _ = std::fs::remove_dir_all(&directory);
    }
}
