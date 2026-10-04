//! Configured local processes: provider plugins (ADR-0006) and MCP servers
//! (ADR-0007), executables the user registers in Kineto's configuration,
//! driven over stdio.
//!
//! This crate knows the configuration files ([`config`], [`mcp`]), the
//! `kineto-plugin/1` messages ([`protocol`]), how to run one operation of a
//! plugin safely ([`host`]) and how to use an MCP server as a client
//! ([`mcp`]): only a configured absolute executable, never through a
//! shell, with a cleared environment, in a scratch directory outside any
//! project, under a deadline and an output cap, and with every file a
//! plugin answers with checked against its declared size and hash. What an
//! answer means is for the caller to decide; this crate holds no project
//! and makes no paid decision. [`demo`] and [`fake_mcp`] are a free plugin
//! and a small MCP server for tests and trials.

pub mod config;
pub mod demo;
pub mod fake_mcp;
pub mod host;
pub mod mcp;
pub mod protocol;

pub use config::{ConfigError, PluginConfig, PluginEntry};
pub use host::{Done, HostError, Input, Limits, OutputFile, run};
pub use protocol::{Answer, FailureClass, PROTOCOL};
