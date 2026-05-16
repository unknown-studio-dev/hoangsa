//! Local web UI server for hoangsa.
//!
//! Boots an Axum server bound to 127.0.0.1, picks a free port, gates `/api/*`
//! on a CSRF token passed via the launch URL, and serves the embedded React
//! SPA at `/`. Designed to be invoked on-demand from `hoangsa-cli ui` — there
//! is no background daemon mode.

mod assets;
mod auth;
mod browser;
mod config;
mod mcp_client;
mod memory;
mod memory_files;
mod patch;
mod port;
mod routes;
mod rules;
mod server;
mod state;

pub use server::run;
pub use server::RunOptions;
