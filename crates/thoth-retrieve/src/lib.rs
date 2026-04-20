//! # thoth-retrieve
//!
//! Retrieval orchestrator. Given a [`Query`] and a [`Mode`], it fans out to
//! the relevant stores, fuses the results with Reciprocal Rank Fusion, and
//! returns a [`Retrieval`].
//!
//! Pipeline (see `DESIGN.md` §4):
//!
//! ```text
//! Query → { symbol | graph | BM25 | markdown | vector (Mode::Full) }
//!       → RRF fuse
//!       → (Mode::Full) Synthesizer::synthesize
//!       → Retrieval
//! ```
//!
//! This crate also hosts the [`Indexer`], which walks a source tree and
//! populates every backend behind a [`StoreRoot`]. The retriever assumes an
//! indexer has already run.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod config;
pub mod enrich;
pub mod indexer;
pub mod retriever;

#[cfg(feature = "anthropic")]
pub mod synth;

#[cfg(feature = "anthropic")]
pub use synth::*;

pub use config::{ChromaConfig, IndexConfig, OutputConfig, RetrieveConfig, WatchConfig};
pub use enrich::{enrich_chunks, extract_docstring};
pub use indexer::{IndexProgress, IndexStats, Indexer, chunk_id, read_span};
pub use retriever::Retriever;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use thoth_core::{Mode, Query, Result, Retrieval, Synthesizer};
use thoth_store::{ChromaCol, ChromaStore, StoreRoot};

/// Process-lifetime cache for [`RetrieveConfig`], keyed by store root path.
///
/// Populated on first use per path; subsequent calls skip the disk read.
static RETRIEVE_CFG_CACHE: OnceLock<RwLock<HashMap<PathBuf, RetrieveConfig>>> = OnceLock::new();

/// Return the cached [`RetrieveConfig`] for `root`, loading and inserting it
/// on the first call for each distinct root path.
async fn cached_retrieve_config(root: &std::path::Path) -> RetrieveConfig {
    let cache = RETRIEVE_CFG_CACHE.get_or_init(|| RwLock::new(HashMap::new()));

    // Fast path: config already cached for this root.
    {
        let guard = cache.read().expect("RETRIEVE_CFG_CACHE poisoned");
        if let Some(cfg) = guard.get(root) {
            return cfg.clone();
        }
    }

    // Slow path: load from disk, then insert under write lock.
    let cfg = RetrieveConfig::load_or_default(root).await;
    {
        let mut guard = cache.write().expect("RETRIEVE_CFG_CACHE poisoned");
        // Another task may have raced us — only insert if still absent.
        guard
            .entry(root.to_path_buf())
            .or_insert_with(|| cfg.clone());
    }
    cfg
}

/// Convenience wrapper: opens the right extra backends for the requested
/// [`Mode`] and runs a single recall.
///
/// In Mode::Zero the synthesizer and vector stages are skipped but ChromaDB
/// semantic search is used if configured. In Mode::Full the ChromaDB stage
/// always runs and the caller-supplied synthesizer is plugged in.
pub async fn recall(store: StoreRoot, q: Query, mode: Mode) -> Result<Retrieval> {
    let retrieve_cfg = cached_retrieve_config(&store.path).await;
    let chroma = chroma_from_config(&store.path).await;
    match mode {
        Mode::Zero => {
            Retriever::new(store)
                .with_chroma(chroma)
                .with_markdown_boost(retrieve_cfg.rerank_markdown_boost)
                .recall(&q)
                .await
        }
        Mode::Full { synthesizer } => {
            let synth: Option<Arc<dyn Synthesizer>> = synthesizer.map(Arc::from);
            Retriever::with_full(store, chroma, synth)
                .with_markdown_boost(retrieve_cfg.rerank_markdown_boost)
                .recall_full(&q)
                .await
        }
    }
}

/// Try to connect to ChromaDB using config. Returns None if ChromaDB is
/// not configured or unreachable.
async fn chroma_from_config(root: &std::path::Path) -> Option<Arc<ChromaCol>> {
    let cfg = ChromaConfig::load_or_default(root).await;
    if !cfg.enabled {
        return None;
    }
    let path = cfg
        .data_path
        .unwrap_or_else(|| StoreRoot::chroma_path(root).to_string_lossy().to_string());
    let store = ChromaStore::open(&path).await.ok()?;
    let (col, _info) = store.ensure_collection("thoth_code").await.ok()?;
    Some(Arc::new(col))
}
