//! `thoth index` — walk + parse + index a source tree.

use std::path::Path;
use std::sync::Mutex;

use anyhow::Result;
use indicatif::{ProgressBar, ProgressStyle};
use thoth_parse::LanguageRegistry;
use thoth_retrieve::{IndexProgress, Indexer};
use thoth_store::StoreRoot;

use crate::open_chroma;

pub async fn run_index(root: &Path, src: &Path, json: bool) -> Result<()> {
    if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d
            .call(
                "thoth_index",
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

    let store = StoreRoot::open(root).await?;
    // Honour `[index]` in `<root>/config.toml` — ignore patterns, max file
    // size, hidden-dir / symlink toggles. Missing file → defaults.
    let cfg = thoth_retrieve::IndexConfig::load_or_default(root).await;
    let mut idx = Indexer::new(store.clone(), LanguageRegistry::new()).with_config(&cfg);
    if let Some(col) = open_chroma(&store).await {
        idx = idx.with_chroma(col);
    }
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
                    if ev.done == 0 {
                        pb.set_length(ev.total as u64);
                        pb.set_position(0);
                        pb.set_message("embedding chunks");
                    } else {
                        pb.set_position(ev.done as u64);
                    }
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
