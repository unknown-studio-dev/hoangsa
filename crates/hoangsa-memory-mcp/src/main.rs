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

// jemalloc as the global allocator. fastembed's ORT inference allocates
// and frees large transient tensors on every embed; libmalloc (macOS) /
// glibc ptmalloc (Linux) keep those freed pages hoarded in per-thread
// arenas for the process lifetime. jemalloc's dirty/muzzy decay (~10 s
// default) returns the pages to the OS, which is what makes the
// `SharedEmbedder::evict_if_idle` reclamation visible in `ps`/`top`.
// Skipped on msvc — `tikv-jemallocator` doesn't build there.
#[cfg(not(target_env = "msvc"))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

use std::path::PathBuf;
use std::sync::Arc;

use hoangsa_memory_mcp::{
    DEFAULT_EMBEDDER_EVICTION_SCAN, DEFAULT_EMBEDDER_IDLE_EVICTION, DEFAULT_EMBEDDER_MAX_AGE,
    Server, ServiceState, populate_from_registry, run_embedder_eviction_loop, run_multi_listener,
    run_socket, run_stdio, socket_path,
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

    // Eager embedder reclamation. Without this loop the ORT session +
    // its CPU memory arena (~150-300 MB once ratcheted) stay loaded for
    // the lifetime of the MCP process, even though most tool calls only
    // need it for a few seconds at a time. The loop drops the model
    // after `DEFAULT_EMBEDDER_IDLE_EVICTION` of no embed activity; the
    // next call pays a ~1-3 s re-init from the on-disk fastembed cache.
    let embedder = server.shared_embedder().clone();
    tokio::spawn(async move {
        run_embedder_eviction_loop(
            embedder,
            DEFAULT_EMBEDDER_IDLE_EVICTION,
            DEFAULT_EMBEDDER_MAX_AGE,
            DEFAULT_EMBEDDER_EVICTION_SCAN,
        )
        .await;
    });

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

/// Resolve root for stdio mode using cwd as the project dir.
/// See `hoangsa_memory_core::resolve_root` for the precedence chain.
fn resolve_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    hoangsa_memory_core::resolve_root(&cwd, None)
}
