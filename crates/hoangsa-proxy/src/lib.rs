//! hoangsa-proxy — CLI output compressor for Claude Code.
//!
//! Wraps a child command, captures stdout/stderr, runs a filter pipeline
//! (built-in Rust handler or user Rhai script), and emits trimmed output
//! back to the caller. Exit code passes through 1:1. Fail-open: any filter
//! error falls back to raw passthrough.

pub mod ansi;
pub mod config;
pub mod doctor;
pub mod exec;
pub mod filters;
pub mod handlers;
pub mod init;
pub mod prefs;
pub mod registry;
pub mod report;
pub mod rhai_engine;
pub mod scope;
pub mod tty;
