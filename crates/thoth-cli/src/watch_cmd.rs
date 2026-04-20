//! `thoth watch` — stay resident, reindex on source-tree changes.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use thoth_parse::{LanguageRegistry, watch::Watcher};
use thoth_retrieve::Indexer;
use thoth_store::StoreRoot;
use tracing::warn;

use crate::{index_cmd::make_progress_bar, open_chroma};

pub async fn run_watch(root: &Path, src: &Path, debounce: Duration) -> Result<()> {
    // If the MCP daemon is running it holds the redb exclusive lock.
    // Instead of failing, fall back to a log-only mode: watch the
    // filesystem and print what changed, but don't index (the daemon's
    // auto-watch handles that when `[watch] enabled = true`).
    if crate::daemon::DaemonClient::try_connect(root)
        .await
        .is_some()
    {
        let watch_cfg = thoth_retrieve::WatchConfig::load_or_default(root).await;
        if watch_cfg.enabled {
            println!(
                "thoth-mcp daemon is running with auto-watch enabled — \
                 showing live file-change log only."
            );
        } else {
            println!(
                "thoth-mcp daemon is running (auto-watch disabled). \
                 Showing live file-change log. Tip: set `[watch] enabled = true` \
                 in config.toml to auto-reindex inside the daemon."
            );
        }
        return run_watch_log_only(src, debounce).await;
    }

    let store = StoreRoot::open(root).await?;
    let cfg = thoth_retrieve::IndexConfig::load_or_default(root).await;
    let mut idx = Indexer::new(store.clone(), LanguageRegistry::new()).with_config(&cfg);
    if let Some(col) = open_chroma(&store).await {
        idx = idx.with_chroma(col);
    }
    idx = idx.with_progress(make_progress_bar());

    // Do an initial full index so subsequent deltas matter.
    let stats = idx.index_path(src).await?;
    println!(
        "✓ initial index: {} files · {} chunks · {} symbols",
        stats.files, stats.chunks, stats.symbols,
    );

    let mut w = Watcher::watch(src, 1024)?;
    println!("… watching {} (ctrl-c to stop)", src.display());

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\n✓ stopped");
                break;
            }
            ev = w.recv() => {
                let Some(ev) = ev else {
                    warn!("watcher channel closed");
                    break;
                };
                // Simple debounce: after the first event, drain anything that
                // arrives within `debounce` then batch-reindex affected files.
                let mut batch = vec![ev];
                let deadline = tokio::time::Instant::now() + debounce;
                while let Ok(Some(extra)) = tokio::time::timeout_at(deadline, w.recv()).await {
                    batch.push(extra);
                }

                // Split into change vs. delete sets so deletions only
                // purge (no reparse on a missing file).
                let mut changed = std::collections::HashSet::new();
                let mut deleted = std::collections::HashSet::new();
                for ev in batch {
                    match ev {
                        thoth_core::Event::FileChanged { path, .. } => {
                            deleted.remove(&path);
                            changed.insert(path);
                        }
                        thoth_core::Event::FileDeleted { path, .. } => {
                            changed.remove(&path);
                            deleted.insert(path);
                        }
                        _ => {}
                    }
                }

                let changed_n = changed.len();
                let deleted_n = deleted.len();

                for path in deleted {
                    if let Err(e) = idx.purge_path(&path).await {
                        warn!(?path, error = %e, "purge failed");
                    }
                }
                for path in changed {
                    if let Err(e) = idx.index_file(&path).await {
                        warn!(?path, error = %e, "re-index failed");
                    }
                }

                // Flush BM25 writes so the next `query` (or hook pull) sees
                // them — both deletes and adds need to be committed.
                if changed_n + deleted_n > 0 {
                    if let Err(e) = idx.commit().await {
                        warn!(error = %e, "fts commit failed");
                    }
                    if changed_n > 0 {
                        println!(
                            "  ↻ reindexed {changed_n} file{}",
                            if changed_n == 1 { "" } else { "s" }
                        );
                    }
                    if deleted_n > 0 {
                        println!(
                            "  🗑 purged {deleted_n} file{}",
                            if deleted_n == 1 { "" } else { "s" }
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Log-only fallback for `thoth watch` when the MCP daemon holds the
/// redb lock. Watches the filesystem and prints changes, but doesn't
/// index — the daemon handles that.
pub async fn run_watch_log_only(src: &Path, debounce: Duration) -> Result<()> {
    let mut w = Watcher::watch(src, 1024)?;
    println!("… watching {} (log only, ctrl-c to stop)", src.display());

    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                println!("\n✓ stopped");
                break;
            }
            ev = w.recv() => {
                let Some(ev) = ev else {
                    warn!("watcher channel closed");
                    break;
                };
                let mut batch = vec![ev];
                let deadline = tokio::time::Instant::now() + debounce;
                while let Ok(Some(extra)) = tokio::time::timeout_at(deadline, w.recv()).await {
                    batch.push(extra);
                }

                let mut changed = Vec::new();
                let mut deleted = Vec::new();
                for ev in batch {
                    match ev {
                        thoth_core::Event::FileChanged { path, .. } => changed.push(path),
                        thoth_core::Event::FileDeleted { path, .. } => deleted.push(path),
                        _ => {}
                    }
                }

                for p in &changed {
                    println!("  ✎ {}", p.display());
                }
                for p in &deleted {
                    println!("  ✗ {}", p.display());
                }
            }
        }
    }
    Ok(())
}
