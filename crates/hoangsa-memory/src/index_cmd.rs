//! `hoangsa-memory index` — walk + parse + index a source tree.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use hoangsa_memory_parse::LanguageRegistry;
use hoangsa_memory_retrieve::{IndexProgress, Indexer};
use hoangsa_memory_store::StoreRoot;

use crate::open_vector_store;

/// Window we wait for a booting daemon before falling through to
/// direct mode. Covers the race between `Server::open` (store locks
/// taken) and `UnixListener::bind` (socket accepting) at session start.
const DAEMON_CONNECT_WAIT: Duration = Duration::from_secs(3);

/// How long a direct-mode `index` waits on the per-project store lock
/// before giving up. Long enough to sit behind a slow bootstrap on a
/// large repo; short enough that a truly stuck writer reports instead
/// of hanging forever.
const STORE_LOCK_WAIT: Duration = Duration::from_secs(120);

pub async fn run_index(root: &Path, src: &Path, json: bool) -> Result<()> {
    if let Some(mut d) =
        crate::daemon::DaemonClient::connect_or_wait(root, DAEMON_CONNECT_WAIT).await
    {
        let result = d
            .call(
                "memory_index",
                serde_json::json!({ "path": src.to_string_lossy() }),
            )
            .await?;
        if crate::daemon::tool_is_error(&result) {
            anyhow::bail!("{}", crate::daemon::tool_text(&result));
        }
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&crate::daemon::tool_data(&result))?
            );
        } else {
            println!("{}", crate::daemon::tool_text(&result));
        }
        return Ok(());
    }

    // Direct mode: no daemon to route through. Serialize with any
    // other direct-mode CLI on the same project (typical case: the
    // SessionStart bootstrap worker is still running).
    let _store_lock = match crate::acquire_store_lock(root, STORE_LOCK_WAIT).await? {
        Some(lock) => lock,
        None => {
            anyhow::bail!(
                "another hoangsa-memory process has been indexing this project for >{}s; aborting. \
                 If you believe it's stuck, remove {}/store.lock after confirming no index is running.",
                STORE_LOCK_WAIT.as_secs(),
                root.display()
            );
        }
    };

    let store = StoreRoot::open(root).await?;
    // Honour `[index]` in `<root>/config.toml` — ignore patterns, max file
    // size, hidden-dir / symlink toggles. Missing file → defaults.
    let cfg = hoangsa_memory_retrieve::IndexConfig::load_or_default(root).await;
    let mut idx = Indexer::new(store.clone(), LanguageRegistry::new()).with_config(&cfg);
    // Hold the process-wide vector lock for the duration of this run
    // so a hook-triggered `archive ingest` can't load a second copy of
    // the fastembed ONNX model on top of ours. If another vector-using
    // command is already running we skip embeddings rather than aborting
    // the whole index — BM25/graph indexing is still useful without
    // them.
    let _vector_lock = match crate::acquire_vector_lock() {
        Ok(Some(lock)) => {
            if let Some(col) = open_vector_store(&store).await {
                idx = idx.with_vector_store(col);
            }
            Some(lock)
        }
        Ok(None) => {
            eprintln!(
                "hoangsa-memory: another vector-using command is running; indexing without embeddings."
            );
            None
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to acquire vector lock; proceeding without embeddings");
            None
        }
    };
    idx = idx.with_progress(make_progress_bar());

    let stats = idx.index_path(src).await?;
    let reparsed = stats.files.saturating_sub(stats.files_skipped);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "path": src.display().to_string(),
                "files": stats.files,
                "files_reparsed": reparsed,
                "files_skipped": stats.files_skipped,
                "chunks": stats.chunks,
                "symbols": stats.symbols,
                "calls": stats.calls,
                "imports": stats.imports,
                "embedded": stats.embedded,
                "note": "counters are deltas for this run; files_skipped were cache-hits (content hash unchanged)",
            }))?
        );
    } else {
        println!("✓ indexed {}", src.display());
        println!(
            "  {} files ({} reparsed, {} up-to-date) · {} chunks · {} symbols · {} calls · {} imports",
            stats.files,
            reparsed,
            stats.files_skipped,
            stats.chunks,
            stats.symbols,
            stats.calls,
            stats.imports,
        );
        if stats.embedded > 0 {
            println!("  {} chunks embedded", stats.embedded);
        }
    }
    Ok(())
}

/// Build a closure that drives an `indicatif::ProgressBar` from
/// [`IndexProgress`] events. The bar is lazily allocated on the first `walk`
/// event so the total is known, and finished when the commit stage fires.
pub fn make_progress_bar() -> impl for<'a> Fn(IndexProgress<'a>) + Send + Sync + 'static {
    let bar: Mutex<Option<ProgressBar>> = Mutex::new(None);
    move |ev: IndexProgress<'_>| {
        let mut slot = bar.lock().unwrap();
        match ev.stage {
            "walk" => {
                let pb = ProgressBar::new(ev.total as u64);
                // Template: [00:12] [#######>---] 42/128 path/to/file.rs
                let style = ProgressStyle::with_template(
                    "{elapsed_precise} [{bar:30.cyan/blue}] {pos}/{len} {msg}",
                )
                .unwrap_or_else(|_| ProgressStyle::default_bar())
                .progress_chars("=>-");
                pb.set_style(style);
                *slot = Some(pb);
            }
            "file" => {
                if let Some(pb) = slot.as_ref() {
                    pb.set_position(ev.done as u64);
                    if let Some(p) = ev.path {
                        pb.set_message(p.display().to_string());
                    }
                }
            }
            "embed" => {
                if let Some(pb) = slot.as_ref() {
                    // First embed event resets the bar to chunk-scale.
                    // `total` grows per event in streaming mode (phase A
                    // hasn't finished when the first batch embeds), so
                    // resize every tick — indicatif tolerates a growing
                    // length without clobbering the position.
                    if ev.done == 0 {
                        pb.set_position(0);
                        pb.set_message("embedding chunks");
                    }
                    pb.set_length(ev.total as u64);
                    pb.set_position(ev.done as u64);
                }
            }
            "commit" => {
                if let Some(pb) = slot.take() {
                    pb.set_message("committing…");
                    pb.finish_and_clear();
                }
            }
            _ => {}
        }
    }
}
