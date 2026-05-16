//! `hoangsa-memory-mcp` — an MCP (Model Context Protocol) server
//! exposing hoangsa-memory's recall/remember/index capabilities to any
//! MCP-aware client (Claude Agent SDK, Claude Code, Cowork, Cursor, Zed,
//! ...).
//!
//! See the [crate-level docs](hoangsa_memory_mcp) for the wire protocol details and
//! the tool catalog.
//!
//! # Modes
//!
//! - **stdio (default)**: serves a single project on stdin/stdout + one Unix
//!   socket at `<root>/mcp.sock`. Used by `Command::new("hoangsa-memory-mcp")`
//!   spawn paths (Claude Code MCP config, etc.).
//! - **service**: one process, one listener per registered project. Discovers
//!   projects from `~/.hoangsa/projects.json` and the `~/.hoangsa/memory/projects/`
//!   directory and binds `<slug>/mcp.sock` for each. New projects added via the
//!   UI are picked up without restart.
//!
//! # Usage
//!
//! ```text
//! hoangsa-memory-mcp                               # stdio mode (single project)
//! HOANGSA_MEMORY_ROOT=/path/.hoangsa/memory hoangsa-memory-mcp
//! HOANGSA_MEMORY_SERVICE=1 hoangsa-memory-mcp      # multi-project service mode
//! hoangsa-memory-mcp --service                     # equivalent
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use hoangsa_memory_mcp::{
    Server, ServiceState, populate_from_registry, run_multi_listener, run_socket, run_stdio,
    socket_path,
};

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

    if is_service_mode() {
        return run_service().await;
    }
    run_single().await
}

fn is_service_mode() -> bool {
    if std::env::args().any(|a| a == "--service") {
        return true;
    }
    matches!(
        std::env::var("HOANGSA_MEMORY_SERVICE").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

async fn run_service() -> anyhow::Result<()> {
    let home = hoangsa_memory_core::projects::default_hoangsa_home()?;
    tracing::info!(home = %home.display(), "hoangsa-memory-mcp service starting");

    let state = Arc::new(ServiceState::new(home));
    populate_from_registry(&state)?;
    run_multi_listener(state).await?;
    tracing::info!("hoangsa-memory-mcp service exiting");
    Ok(())
}

async fn run_single() -> anyhow::Result<()> {
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
