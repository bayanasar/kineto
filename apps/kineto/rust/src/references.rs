//! References from MCP tools (ADR-0007) across the C ABI: the commands that
//! ask a tool and keep, attach, detach, edit or remove its answer, reads of
//! the references the worker last published, and a listing of the MCP
//! servers the user registered with the tools they allow.
//!
//! A tool's arguments cross as typed name and value pairs, never as JSON;
//! the engine builds the object the tool is called with. The listing is
//! read from the user's configuration, not a project, and starts each
//! enabled server once to list its tools, so Dart loads it off the UI
//! thread.

use std::{ptr, slice};

use kineto_core::{
    references::{self as core_references, ParameterKind, ServerListing, ToolParameter},
    session::ProductionCommand,
};
use kineto_plugins::mcp::McpConfig;
use serde_json::{Map, Number, Value};

use crate::{
    contract::{name as name_of, *},
    documents::{at, session_ref},
    guarded,
    production::{KinetoText, submit_production},
    project_session::{KinetoProjectSession, copy_utf8, utf8_owned},
};

/// The most arguments or targets one request may carry.
const MAX_LIST: u64 = 256;

/// One argument of a tool call, as Dart lays it out.
#[repr(C)]
pub struct KinetoToolArgument {
    pub name: KinetoText,
    /// A contract `tool_argument_kind` code.
    pub kind: u32,
    /// The value as text: a number in decimal, a flag as `true` or `false`.
    pub value: KinetoText,
}

/// # Safety
/// `items` must be readable for `count` elements when `count` is non-zero.
unsafe fn list<'a, T>(items: *const T, count: u64) -> Result<&'a [T], i32> {
    if count == 0 {
        return Ok(&[]);
    }
    if items.is_null() || count > MAX_LIST {
        return Err(PROJECT_ERR_INVALID_ARGUMENT);
    }
    // SAFETY: the caller guarantees `count` readable elements.
    Ok(unsafe { slice::from_raw_parts(items, count as usize) })
}

/// # Safety
/// As for [`utf8_owned`].
unsafe fn text(value: &KinetoText) -> Result<String, i32> {
    // SAFETY: forwarded caller contract.
    unsafe { utf8_owned(value.ptr, value.len) }
}

/// # Safety
/// As for [`utf8_owned`]; the text must not be empty.
unsafe fn required(ptr: *const u8, len: u64) -> Result<String, i32> {
    // SAFETY: forwarded caller contract.
    match unsafe { utf8_owned(ptr, len) } {
        Ok(text) if !text.trim().is_empty() => Ok(text),
        Ok(_) => Err(PROJECT_ERR_INVALID_ARGUMENT),
        Err(code) => Err(code),
    }
}

fn argument_value(kind: &str, value: String) -> Option<Value> {
    match kind {
        "text" => Some(Value::String(value)),
        "number" => {
            let trimmed = value.trim();
            trimmed
                .parse::<i64>()
                .ok()
                .map(Number::from)
                .or_else(|| trimmed.parse::<f64>().ok().and_then(Number::from_f64))
                .map(Value::Number)
        }
        "flag" => match value.trim() {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        _ => None,
    }
}

/// Queue calling `tool` of the registered MCP server `server` with
/// `arguments`, and keeping its text as a reference about `targets`
/// (`bible_world` or bible entry IDs; none to attach it later). Answers with
/// a `COMPLETED` message whose shot is -1. Nothing is started for a server
/// or tool that is not registered, enabled and allowed.
///
/// # Safety
/// `session` must be a live project-session handle. Each non-empty text must
/// be readable for its length and hold UTF-8; `arguments` and `targets` must
/// be readable for their counts.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_ask_reference(
    session: *const KinetoProjectSession,
    server_ptr: *const u8,
    server_len: u64,
    tool_ptr: *const u8,
    tool_len: u64,
    arguments: *const KinetoToolArgument,
    argument_count: u64,
    targets: *const KinetoText,
    target_count: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (server, tool) = match unsafe {
            (
                required(server_ptr, server_len),
                required(tool_ptr, tool_len),
            )
        } {
            (Ok(server), Ok(tool)) => (server, tool),
            (Err(code), _) | (_, Err(code)) => return code,
        };
        // SAFETY: forwarded caller contract.
        let (raw_arguments, raw_targets) =
            match unsafe { (list(arguments, argument_count), list(targets, target_count)) } {
                (Ok(arguments), Ok(targets)) => (arguments, targets),
                (Err(code), _) | (_, Err(code)) => return code,
            };
        let mut built = Map::new();
        for argument in raw_arguments {
            // SAFETY: forwarded caller contract.
            let (name, value) = match unsafe { (text(&argument.name), text(&argument.value)) } {
                (Ok(name), Ok(value)) if !name.is_empty() => (name, value),
                (Err(code), _) | (_, Err(code)) => return code,
                _ => return PROJECT_ERR_INVALID_ARGUMENT,
            };
            let Some(value) = name_of(&TOOL_ARGUMENT_KINDS, argument.kind)
                .and_then(|kind| argument_value(kind, value))
            else {
                return PROJECT_ERR_INVALID_ARGUMENT;
            };
            if built.insert(name, value).is_some() {
                return PROJECT_ERR_INVALID_ARGUMENT;
            }
        }
        let mut attached_to = Vec::with_capacity(raw_targets.len());
        for target in raw_targets {
            // SAFETY: forwarded caller contract.
            match unsafe { text(target) } {
                Ok(target) if !target.is_empty() => attached_to.push(target),
                Ok(_) => return PROJECT_ERR_INVALID_ARGUMENT,
                Err(code) => return code,
            }
        }
        let command = ProductionCommand::AskReference {
            server,
            tool,
            arguments: built,
            attached_to,
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue replacing reference `id`'s text with `text`, the director's own.
///
/// # Safety
/// As for [`kineto_project_ask_reference`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_edit_reference(
    session: *const KinetoProjectSession,
    id_ptr: *const u8,
    id_len: u64,
    text_ptr: *const u8,
    text_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (reference, text) =
            match unsafe { (required(id_ptr, id_len), required(text_ptr, text_len)) } {
                (Ok(reference), Ok(text)) => (reference, text),
                (Err(code), _) | (_, Err(code)) => return code,
            };
        let command = ProductionCommand::EditReference { reference, text };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue attaching reference `id` to `target` (`bible_world` or a bible
/// entry ID) when `attach`, or detaching it otherwise.
///
/// # Safety
/// As for [`kineto_project_ask_reference`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_attach_reference(
    session: *const KinetoProjectSession,
    id_ptr: *const u8,
    id_len: u64,
    target_ptr: *const u8,
    target_len: u64,
    attach: bool,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let (reference, target) =
            match unsafe { (required(id_ptr, id_len), required(target_ptr, target_len)) } {
                (Ok(reference), Ok(target)) => (reference, target),
                (Err(code), _) | (_, Err(code)) => return code,
            };
        let command = if attach {
            ProductionCommand::AttachReference { reference, target }
        } else {
            ProductionCommand::DetachReference { reference, target }
        };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// Queue removing reference `id`. Its ID is not given to another.
///
/// # Safety
/// As for [`kineto_project_ask_reference`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_remove_reference(
    session: *const KinetoProjectSession,
    id_ptr: *const u8,
    id_len: u64,
    token: u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let reference = match unsafe { required(id_ptr, id_len) } {
            Ok(reference) => reference,
            Err(code) => return code,
        };
        let command = ProductionCommand::RemoveReference { reference };
        // SAFETY: forwarded caller contract.
        unsafe { submit_production(session, command, false, token) }
    })
}

/// How many references the worker last published; 0 for a null handle or
/// before the project opened.
///
/// # Safety
/// `session` must be null or a live project-session handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_reference_count(
    session: *const KinetoProjectSession,
) -> u32 {
    guarded(0, || {
        // SAFETY: forwarded caller contract.
        unsafe { session_ref(session) }
            .and_then(|session| session.production().ok())
            .map_or(0, |production| {
                u32::try_from(production.references.len()).unwrap_or(u32::MAX)
            })
    })
}

/// Copy text field `field` (a contract `reference_text` code) of reference
/// `index`. Lists (arguments as `name: value`, what it is about) are one
/// item per line. Same sizing contract as project metadata. Never does I/O.
///
/// # Safety
/// `session` must be live. `out_len` must point to writable `u64` storage. If
/// `out_cap` is non-zero, `out_buf` must be writable for at least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_project_reference_text_copy(
    session: *const KinetoProjectSession,
    index: u32,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: forwarded caller contract.
        let Some(session) = (unsafe { session_ref(session) }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Ok(production) = session.production() else {
            return PROJECT_ERR_NOT_OPEN;
        };
        let Some(reference) = at(&production.references, index) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let found = match name_of(&REFERENCE_TEXTS, field) {
            Some("id") => reference.reference_id.clone(),
            Some("server") => reference.server.clone(),
            Some("tool") => reference.tool.clone(),
            Some("arguments") => reference
                .arguments
                .iter()
                .map(|(name, value)| match value {
                    Value::String(text) => format!("{name}: {text}"),
                    other => format!("{name}: {other}"),
                })
                .collect::<Vec<_>>()
                .join("\n"),
            Some("text") => reference.text.clone(),
            Some("attached_to") => reference.attached_to.join("\n"),
            Some("content_hash") => reference.content_hash.clone(),
            _ => return PROJECT_ERR_INVALID_ARGUMENT,
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Opaque listing handle. Dart never dereferences this allocation.
pub struct KinetoMcpServers {
    listed: Vec<ServerListing>,
}

/// Read the user's MCP configuration and list each enabled server's
/// allowed tools. On `PROJECT_OK` a handle is written to `out_servers`; it
/// must be destroyed. An unreadable or invalid configuration answers
/// `plugin_config_invalid`; no configuration lists nothing.
///
/// # Safety
/// `out_servers` must point to writable storage for one pointer.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_load(out_servers: *mut *mut KinetoMcpServers) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        if out_servers.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { ptr::write(out_servers, ptr::null_mut()) };
        let Ok(config) = McpConfig::load_default() else {
            return PROJECT_ERR_PLUGIN_CONFIG_INVALID;
        };
        crate::keys::read_plugin_secrets_from_storage();
        let servers = Box::new(KinetoMcpServers {
            listed: core_references::list_servers(&config),
        });
        // SAFETY: as above.
        unsafe { ptr::write(out_servers, Box::into_raw(servers)) };
        PROJECT_OK
    })
}

/// No server, tool or parameter: the level above is meant.
const NONE: u32 = u32::MAX;

fn tools(listing: &ServerListing) -> &[kineto_plugins::mcp::ToolInfo] {
    match &listing.tools {
        Some(Ok(tools)) => tools,
        _ => &[],
    }
}

/// How many servers there are (`server` = `u32::MAX`), how many tools
/// server `server` allows and offers (`tool` = `u32::MAX`), or how many
/// parameters tool `tool` of it takes. 0 for anything that does not exist.
///
/// # Safety
/// `servers` must be null or a live listing handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_count(
    servers: *const KinetoMcpServers,
    server: u32,
    tool: u32,
) -> u32 {
    guarded(0, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(servers) = (unsafe { servers.as_ref() }) else {
            return 0;
        };
        let count = if server == NONE {
            servers.listed.len()
        } else {
            let Some(listing) = at(&servers.listed, server) else {
                return 0;
            };
            if tool == NONE {
                tools(listing).len()
            } else {
                at(tools(listing), tool).map_or(0, |tool| core_references::parameters(tool).len())
            }
        };
        u32::try_from(count).unwrap_or(u32::MAX)
    })
}

fn parameter(
    servers: &KinetoMcpServers,
    server: u32,
    tool: u32,
    index: u32,
) -> Option<ToolParameter> {
    let listing = at(&servers.listed, server)?;
    let tool = at(tools(listing), tool)?;
    let parameters = core_references::parameters(tool);
    at(&parameters, index).cloned()
}

/// Number field `field` (a contract `mcp_value` code): of server `server`
/// (`enabled`, `listed`), or of parameter `parameter` of its tool `tool`
/// (`required`, `kind`, a contract `tool_argument_kind` code, 0 for one the
/// director cannot give).
///
/// # Safety
/// `servers` must be a live listing handle and `out_value` writable storage
/// for one `u64`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_value(
    servers: *const KinetoMcpServers,
    server: u32,
    tool: u32,
    parameter_index: u32,
    field: u32,
    out_value: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(servers) = (unsafe { servers.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        if out_value.is_null() {
            return PROJECT_ERR_INVALID_ARGUMENT;
        }
        let Some(listing) = at(&servers.listed, server) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let found = match name_of(&MCP_VALUES, field) {
            Some("enabled") => Some(u64::from(listing.entry.enabled)),
            Some("listed") => Some(u64::from(matches!(listing.tools, Some(Ok(_))))),
            Some("required") => parameter(servers, server, tool, parameter_index)
                .map(|parameter| u64::from(parameter.required)),
            Some("kind") => parameter(servers, server, tool, parameter_index).map(|parameter| {
                u64::from(match parameter.kind {
                    ParameterKind::Text => code(&TOOL_ARGUMENT_KINDS, "text"),
                    ParameterKind::Number => code(&TOOL_ARGUMENT_KINDS, "number"),
                    ParameterKind::Flag => code(&TOOL_ARGUMENT_KINDS, "flag"),
                    ParameterKind::Other => 0,
                })
            }),
            _ => None,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: checked non-null; the caller guarantees it is writable.
        unsafe { out_value.write(found) };
        PROJECT_OK
    })
}

/// Copy text field `field` (a contract `mcp_text` code): of server
/// `server` (`id`, `executable`, `secrets` and `allowed` one per line,
/// `problem`), of its tool `tool` (`tool_name`, `tool_description`), or of
/// that tool's parameter `parameter` (`parameter_name`,
/// `parameter_description`). Same sizing contract as project metadata.
///
/// # Safety
/// `servers` must be a live listing handle. `out_len` must point to writable
/// `u64` storage. If `out_cap` is non-zero, `out_buf` must be writable for at
/// least `out_cap` bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_text_copy(
    servers: *const KinetoMcpServers,
    server: u32,
    tool: u32,
    parameter_index: u32,
    field: u32,
    out_buf: *mut u8,
    out_cap: u64,
    out_len: *mut u64,
) -> i32 {
    guarded(PROJECT_ERR_PANIC, || {
        // SAFETY: the caller guarantees a live handle or null.
        let Some(servers) = (unsafe { servers.as_ref() }) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let Some(listing) = at(&servers.listed, server) else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        let tool_info = at(tools(listing), tool);
        let found = match name_of(&MCP_TEXTS, field) {
            Some("id") => Some(listing.entry.id.clone()),
            Some("executable") => Some(listing.entry.executable.to_string_lossy().into_owned()),
            Some("secrets") => Some(listing.entry.secrets.join("\n")),
            Some("allowed") => Some(listing.entry.tools.join("\n")),
            Some("problem") => Some(match &listing.tools {
                Some(Err(problem)) => problem.clone(),
                _ => String::new(),
            }),
            Some("tool_name") => tool_info.map(|tool| tool.name.clone()),
            Some("tool_description") => tool_info.map(|tool| tool.description.clone()),
            Some("parameter_name") => {
                parameter(servers, server, tool, parameter_index).map(|parameter| parameter.name)
            }
            Some("parameter_description") => parameter(servers, server, tool, parameter_index)
                .map(|parameter| parameter.description),
            _ => None,
        };
        let Some(found) = found else {
            return PROJECT_ERR_INVALID_ARGUMENT;
        };
        // SAFETY: forwarded caller-owned output contract.
        unsafe { copy_utf8(found.as_bytes(), out_buf, out_cap, out_len) }
    })
}

/// Release a listing.
///
/// # Safety
/// `servers` must be null or a handle from [`kineto_mcp_load`] not yet
/// destroyed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kineto_mcp_destroy(servers: *mut KinetoMcpServers) {
    if servers.is_null() {
        return;
    }
    guarded((), || {
        // SAFETY: the caller contract makes this the one release of a live
        // allocation from `kineto_mcp_load`.
        unsafe { drop(Box::from_raw(servers)) };
    });
}
