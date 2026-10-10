//! References the director asks an MCP tool for (ADR-0007).
//!
//! Only the director calls a tool: they choose a registered server, one of
//! the tools it allows, and its arguments, and say what the answer is
//! about. The tool's text is kept as a reference in the project
//! ([`kineto_project::references`]); the bible reads it as material the
//! next time it is written, and goes stale when it changes. Nothing a tool
//! answers is ever acted on: it is text, kept and shown.

use std::{error::Error, fmt};

use kineto_jobs::ErrorClass;
use kineto_plugins::mcp::{self, McpConfig, McpError, McpLimits, McpServerEntry, ToolInfo};
use kineto_project::{
    manifest::CanonicalProject,
    references::{self as document, NewReference, Reference},
};
use serde_json::{Map, Value};

use crate::{documents::document_error, generation::GenerationError};

/// Why a tool's answer was not kept.
#[derive(Debug)]
pub enum ReferenceCallError {
    /// The server or tool is not registered, enabled and allowed, or the
    /// configuration cannot be read. Nothing was started.
    NotAllowed(String),
    /// The server failed or broke the protocol.
    Server(McpError),
    /// The tool said the call failed; what it said.
    ToolFailed(String),
    /// The tool answered no text.
    NoText,
}

impl fmt::Display for ReferenceCallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAllowed(reason) => write!(formatter, "not allowed: {reason}"),
            Self::Server(error) => error.fmt(formatter),
            Self::ToolFailed(said) => write!(formatter, "the tool failed: {said}"),
            Self::NoText => formatter.write_str("the tool answered no text"),
        }
    }
}

impl Error for ReferenceCallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Server(error) => Some(error),
            _ => None,
        }
    }
}

fn failed(error: ReferenceCallError) -> GenerationError {
    let class = match &error {
        ReferenceCallError::Server(McpError::Timeout | McpError::Exited) => ErrorClass::Retryable,
        _ => ErrorClass::Terminal,
    };
    GenerationError::Provider {
        source: Box::new(error),
        class,
    }
}

/// The user's registered MCP servers.
pub fn servers() -> Result<McpConfig, GenerationError> {
    McpConfig::load_default()
        .map_err(|error| failed(ReferenceCallError::NotAllowed(error.to_string())))
}

/// The enabled server `id`.
pub fn server(id: &str) -> Result<McpServerEntry, GenerationError> {
    servers()?.enabled(id).cloned().ok_or_else(|| {
        failed(ReferenceCallError::NotAllowed(format!(
            "no enabled MCP server is registered as {id:?}"
        )))
    })
}

/// The tools `server` offers that it allows.
pub fn tools(server: &McpServerEntry) -> Result<Vec<ToolInfo>, GenerationError> {
    mcp::list_tools(server, &crate::plugin::plugin_secret, McpLimits::DEFAULT)
        .map_err(|error| failed(ReferenceCallError::Server(error)))
}

/// What the director asks for.
#[derive(Debug, Clone, PartialEq)]
pub struct Ask<'a> {
    pub tool: &'a str,
    pub arguments: Map<String, Value>,
    /// What the answer is about: `bible_world` or bible entry IDs. May be
    /// empty, to read it first and attach it later.
    pub attached_to: Vec<String>,
}

/// Call `ask.tool` of `server` and keep its text as the next reference.
pub fn ask(
    project: &CanonicalProject,
    server: &McpServerEntry,
    ask: Ask<'_>,
) -> Result<Reference, GenerationError> {
    ask_within(project, server, ask, McpLimits::DEFAULT)
}

/// [`ask`] within `limits`.
pub fn ask_within(
    project: &CanonicalProject,
    server: &McpServerEntry,
    ask: Ask<'_>,
    limits: McpLimits,
) -> Result<Reference, GenerationError> {
    // Refuse before the server starts: what nothing could keep is not
    // asked for.
    if project.is_read_only() {
        return Err(document_error(document::ReferenceError::Manifest(
            kineto_project::manifest::ProjectManifestError::ReadOnlyFormatVersion(
                project.manifest().format_version,
            ),
        )));
    }
    if let Some(target) = ask
        .attached_to
        .iter()
        .find(|target| !document::is_target(target))
    {
        return Err(failed(ReferenceCallError::NotAllowed(format!(
            "{target:?} is not the world or a bible entry"
        ))));
    }
    if !server.allows(ask.tool) {
        return Err(failed(ReferenceCallError::NotAllowed(format!(
            "server {:?} does not allow tool {:?}",
            server.id, ask.tool
        ))));
    }
    // The project is free for other requests while the server answers.
    let answered = crate::turn::outside(|| {
        mcp::call_tool(
            server,
            &crate::plugin::plugin_secret,
            ask.tool,
            &Value::Object(ask.arguments.clone()),
            limits,
        )
    })
    .map_err(|error| failed(ReferenceCallError::Server(error)))?;
    let text = answered.text.join("\n\n");
    if answered.is_error {
        return Err(failed(ReferenceCallError::ToolFailed(text)));
    }
    if text.trim().is_empty() {
        return Err(failed(ReferenceCallError::NoText));
    }
    document::add(
        project,
        NewReference {
            server: server.id.clone(),
            tool: ask.tool.to_owned(),
            arguments: ask.arguments,
            text,
            attached_to: ask.attached_to,
        },
    )
    .map_err(document_error)
}

/// How a tool takes one of its arguments, as its input schema says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterKind {
    Text,
    Number,
    Flag,
    /// Anything else, which the director cannot give in a field.
    Other,
}

/// One argument a tool takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolParameter {
    pub name: String,
    pub kind: ParameterKind,
    pub description: String,
    pub required: bool,
}

/// The arguments `tool` takes, in its schema's order.
#[must_use]
pub fn parameters(tool: &ToolInfo) -> Vec<ToolParameter> {
    let required: Vec<&str> = tool.input_schema["required"]
        .as_array()
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    tool.input_schema["properties"]
        .as_object()
        .map(|properties| {
            properties
                .iter()
                .map(|(name, schema)| ToolParameter {
                    name: name.clone(),
                    kind: match schema["type"].as_str() {
                        Some("string") => ParameterKind::Text,
                        Some("number" | "integer") => ParameterKind::Number,
                        Some("boolean") => ParameterKind::Flag,
                        _ => ParameterKind::Other,
                    },
                    description: schema["description"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                    required: required.contains(&name.as_str()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A registered server as settings show it.
#[derive(Debug)]
pub struct ServerListing {
    pub entry: McpServerEntry,
    /// `None` when it is disabled; otherwise the tools it offers that it
    /// allows, or why they could not be listed.
    pub tools: Option<Result<Vec<ToolInfo>, String>>,
}

/// Every server `config` registers, in its order, each enabled one started
/// once to list its tools.
#[must_use]
pub fn list_servers(config: &McpConfig) -> Vec<ServerListing> {
    config
        .servers()
        .iter()
        .map(|entry| ServerListing {
            entry: entry.clone(),
            tools: entry.enabled.then(|| {
                mcp::list_tools(entry, &crate::plugin::plugin_secret, McpLimits::DEFAULT)
                    .map_err(|error| error.to_string())
            }),
        })
        .collect()
}
