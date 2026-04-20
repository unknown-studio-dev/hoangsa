//! # thoth-mcp
//!
//! MCP (Model Context Protocol) stdio server that exposes Thoth's recall,
//! indexing, and memory-curation capabilities to any MCP-aware client
//! (Claude Agent SDK, Claude Code, Cowork, Cursor, Zed, ...).
//!
//! The server speaks **newline-delimited JSON-RPC 2.0** on stdin/stdout, as
//! specified by the 2024-11-05 MCP schema. It implements:
//!
//! - `initialize` / `initialized`
//! - `ping`
//! - `tools/list`, `tools/call`
//! - `resources/list`, `resources/read`
//!
//! ### Tools exposed
//!
//! | Tool                       | Purpose                                          |
//! |----------------------------|--------------------------------------------------|
//! | `thoth_recall`             | Mode::Zero hybrid recall over the code memory    |
//! | `thoth_index`              | Walk a source path and populate indexes          |
//! | `thoth_remember_fact`      | Append a fact to `MEMORY.md`                     |
//! | `thoth_remember_lesson`    | Append a lesson to `LESSONS.md`                  |
//! | `thoth_skills_list`        | Enumerate installed skills under `.thoth/skills/`|
//! | `thoth_memory_show`        | Return current `MEMORY.md` + `LESSONS.md`        |
//!
//! Two markdown files are also published as MCP resources so clients can
//! surface them directly: `thoth://memory/MEMORY.md` and
//! `thoth://memory/LESSONS.md`.
//!
//! The on-disk layout is the same as everywhere else in Thoth — see
//! `thoth_store::StoreRoot`.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod proto;
pub mod sanitize;
pub mod server;

pub use server::{Server, run_socket, run_stdio, socket_path};
