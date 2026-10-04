//! Provider plugins: executables the user registers in Kineto's
//! configuration, driven over stdio (ADR-0006).
//!
//! This crate knows the configuration file ([`config`]), the
//! `kineto-plugin/1` messages ([`protocol`]) and how to run one operation
//! of a plugin safely ([`host`]): only a configured absolute executable,
//! never through a shell, with a cleared environment, in a scratch
//! directory outside any project, under a deadline and an output cap, and
//! with every file it answers with checked against its declared size and
//! hash. What an answer means for a capability is for the caller to decide;
//! this crate holds no project and makes no paid decision.

pub mod config;
pub mod demo;
pub mod host;
pub mod protocol;

pub use config::{ConfigError, PluginConfig, PluginEntry};
pub use host::{Done, HostError, Input, Limits, OutputFile, run};
pub use protocol::{Answer, FailureClass, PROTOCOL};
