//! Background file watcher that reindexes changed files in-process.

use std::path::PathBuf;

use tracing::{debug, warn};

use super::Server;

// ===========================================================================
// Background file watcher
// ===========================================================================

/// Watch `src` for file changes and reindex through the project's Indexer.
///
/// Mirrors the debounce + batch logic in `cmd_watch` but runs in-process
/// alongside the MCP daemon, sharing the same `Indexer` (and therefore the
/// same redb write lock). This avoids the "daemon is running" conflict
/// that blocks the standalone `hoangsa-memory watch`.
///
/// Resolves the `Indexer` per batch via [`Server::resources_if_warm`] —
/// **not** [`Server::resources`] — so the watcher cooperates with
/// Phase-5 idle eviction. If the bundle has been dropped while no
/// user-driven traffic arrived, the watcher drops its batch and waits
/// for the next user tool call to rehydrate the project; chunk IDs are
/// content-hash-derived, so a subsequent `memory_index` re-picks up
/// every changed file. Without this gate, any project with background
/// fs activity (git checkout, npm install) would never stay evicted.
pub(super) async fn run_watcher(
    server: Server,
    src: PathBuf,
    debounce: std::time::Duration,
) -> anyhow::Result<()> {
    use hoangsa_memory_parse::watch::Watcher;

    let mut w = Watcher::watch(&src, 1024)?;
    debug!(path = %src.display(), "background watcher started");

    loop {
        let Some(ev) = w.recv().await else {
            debug!("watcher channel closed");
            break;
        };

        // Debounce: drain events arriving within the window.
        let mut batch = vec![ev];
        let deadline = tokio::time::Instant::now() + debounce;
        while let Ok(Some(extra)) = tokio::time::timeout_at(deadline, w.recv()).await {
            batch.push(extra);
        }

        let mut changed = std::collections::HashSet::new();
        let mut deleted = std::collections::HashSet::new();
        for ev in batch {
            match ev {
                hoangsa_memory_core::Event::FileChanged { path, .. } => {
                    deleted.remove(&path);
                    changed.insert(path);
                }
                hoangsa_memory_core::Event::FileDeleted { path, .. } => {
                    changed.remove(&path);
                    deleted.insert(path);
                }
                _ => {}
            }
        }

        let changed_n = changed.len();
        let deleted_n = deleted.len();
        if changed_n + deleted_n == 0 {
            continue;
        }

        let Some(res) = server.resources_if_warm().await else {
            debug!(
                changed = changed_n,
                deleted = deleted_n,
                "watcher: bundle evicted; dropping batch (will be re-scanned on next user reindex)"
            );
            continue;
        };
        for path in deleted {
            if let Err(e) = res.indexer.purge_path(&path).await {
                warn!(?path, error = %e, "watcher: purge failed");
            }
        }
        for path in changed {
            if let Err(e) = res.indexer.index_file(&path).await {
                warn!(?path, error = %e, "watcher: re-index failed");
            }
        }

        if let Err(e) = res.indexer.commit().await {
            warn!(error = %e, "watcher: fts commit failed");
        }
        debug!(
            changed = changed_n,
            deleted = deleted_n,
            "watcher: reindexed"
        );
    }
    Ok(())
}
