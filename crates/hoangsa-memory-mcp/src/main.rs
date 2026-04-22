//! `hoangsa-memory-mcp` — an MCP (Model Context Protocol) stdio server
//! exposing hoangsa-memory's recall/remember/index capabilities to any
//! MCP-aware client (Claude Agent SDK, Claude Code, Cowork, Cursor, Zed,
//! ...).
//!
//! See the [crate-level docs](hoangsa_memory_mcp) for the wire protocol details and
//! the tool catalog.
//!
//! # Usage
//!
//! ```text
//! hoangsa-memory-mcp                               # serve on stdio; log to stderr
//! HOANGSA_MEMORY_ROOT=/path/.hoangsa/memory hoangsa-memory-mcp
//! ```

use std::path::{Path, PathBuf};

use hoangsa_memory_mcp::{Server, run_socket, run_stdio, socket_path};

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

    tracing::info!(root = %root.display(), "hoangsa-memory-mcp starting");

    let server = Server::open(&root).await?;

    // The project root is either cwd (global mode) or the parent of
    // .hoangsa/memory/ (local mode).
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

    tracing::info!("hoangsa-memory-mcp exiting");
    Ok(())
}

/// Resolve root: `$HOANGSA_MEMORY_ROOT` > populated `./.hoangsa/memory/` >
/// `~/.hoangsa/memory/projects/{readable-slug}/`.
///
/// Mirrors `hoangsa_memory::resolve::resolve_root`: an empty/unpopulated local
/// `.hoangsa/memory/` must not shadow the global root.
fn resolve_root() -> PathBuf {
    if let Ok(env) = std::env::var("HOANGSA_MEMORY_ROOT") {
        let p = PathBuf::from(env);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    let local = PathBuf::from(".hoangsa").join("memory");
    let local_populated = local.is_dir() && is_populated_root(&local);
    if local_populated {
        return local;
    }
    if let Some(home) = std::env::var_os("HOME")
        && let Ok(cwd) = std::env::current_dir()
    {
        let projects = PathBuf::from(home)
            .join(".hoangsa")
            .join("memory")
            .join("projects");
        return projects.join(project_slug(&cwd));
    }
    local
}

/// Human-readable slug: last two path components, lowercased, non-alnum → `-`.
fn project_slug(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let components: Vec<&str> = canonical
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let n = components.len();
    let parts = if n >= 2 { &components[n - 2..] } else { &components[..] };
    let raw = parts.join("-");
    let mut result = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for c in raw.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            result.push(c);
            prev_dash = false;
        } else if !prev_dash {
            result.push('-');
            prev_dash = true;
        }
    }
    result.trim_matches('-').to_string()
}

fn is_populated_root(root: &Path) -> bool {
    let graph = root.join("graph.redb");
    match std::fs::metadata(&graph) {
        Ok(m) => m.is_file() && m.len() > 4096,
        Err(_) => false,
    }
}
