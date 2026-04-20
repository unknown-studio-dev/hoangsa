//! `thoth-mcp` — an MCP (Model Context Protocol) stdio server exposing
//! Thoth's recall/remember/index capabilities to any MCP-aware client
//! (Claude Agent SDK, Claude Code, Cowork, Cursor, Zed, ...).
//!
//! See the [crate-level docs](thoth_mcp) for the wire protocol details and
//! the tool catalog.
//!
//! # Usage
//!
//! ```text
//! thoth-mcp                   # serve on stdio; log to stderr
//! THOTH_ROOT=/path/.thoth thoth-mcp
//! ```

use std::path::PathBuf;

use thoth_mcp::{Server, run_socket, run_stdio, socket_path};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logs must go to stderr; stdout is reserved for the JSON-RPC transport.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let root = resolve_root();

    tracing::info!(root = %root.display(), "thoth-mcp starting");

    let server = Server::open(&root).await?;

    // The project root is either cwd (global mode) or the parent of .thoth/ (local mode).
    let project_root = std::env::current_dir().unwrap_or_else(|_| {
        root.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| std::path::PathBuf::from("."))
    });
    if server.spawn_watcher(project_root).await {
        tracing::info!("background file watcher enabled");
    }

    // Run stdio (for Claude Code / MCP clients) and a Unix socket (for the
    // CLI thin-client) concurrently. When stdio hits EOF the process exits
    // and the socket task is cancelled automatically.
    let sock = socket_path(&root);
    let socket_server = server.clone();
    tokio::spawn(async move {
        if let Err(e) = run_socket(socket_server).await {
            tracing::warn!(error = %e, "socket listener exited");
        }
    });

    run_stdio(server).await?;

    // Clean up the socket file on normal exit.
    let _ = std::fs::remove_file(&sock);

    tracing::info!("thoth-mcp exiting");
    Ok(())
}

/// Resolve root: `$THOTH_ROOT` > `./.thoth/` > `~/.thoth/projects/{slug}/`.
fn resolve_root() -> PathBuf {
    if let Ok(env) = std::env::var("THOTH_ROOT") {
        let p = PathBuf::from(env);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let local = PathBuf::from(".thoth");
    if local.is_dir() {
        return local;
    }
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(cwd) = std::env::current_dir()
    {
        let canonical = cwd.canonicalize().unwrap_or(cwd);
        let hash = blake3::hash(canonical.to_string_lossy().as_bytes());
        let slug = &hash.to_hex()[..12];
        return PathBuf::from(home)
            .join(".thoth")
            .join("projects")
            .join(slug);
    }
    local
}
