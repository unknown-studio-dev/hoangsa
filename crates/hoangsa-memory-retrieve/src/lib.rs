//! # hoangsa-memory-retrieve
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
//! Vector recall runs in-process via fastembed (see
//! `hoangsa_memory_store::vector`); the old ChromaDB Python sidecar has
//! been removed.
//!
//! This crate also hosts the [`Indexer`], which walks a source tree and
//! populates every backend behind a [`StoreRoot`]. The retriever assumes an
//! indexer has already run.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

pub mod archive;
pub mod config;
pub mod enrich;
pub mod indexer;
pub mod retriever;

pub use archive::{IngestOpts, IngestStats, run_ingest};
pub use config::{IndexConfig, OutputConfig, RetrieveConfig, VectorStoreConfig, WatchConfig};
pub use enrich::{enrich_chunks, extract_docstring};
pub use indexer::{IndexProgress, IndexStats, Indexer, chunk_id, read_span};
pub use retriever::Retriever;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use hoangsa_memory_core::{Mode, Query, Result, Retrieval, Synthesizer};
use hoangsa_memory_store::{EmbeddedVectorStore, StoreRoot, VectorCol, VectorStore};

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
/// In Mode::Zero the synthesizer and vector stages are skipped but the
/// vector store is used if configured. In Mode::Full the vector stage
/// always runs and the caller-supplied synthesizer is plugged in.
pub async fn recall(store: StoreRoot, q: Query, mode: Mode) -> Result<Retrieval> {
    let retrieve_cfg = cached_retrieve_config(&store.path).await;
    let vectors = vector_col_from_config(&store.path).await;
    match mode {
        Mode::Zero => {
            Retriever::new(store)
                .with_vector_store(vectors)
                .with_markdown_boost(retrieve_cfg.rerank_markdown_boost)
                .recall(&q)
                .await
        }
        Mode::Full { synthesizer } => {
            let synth: Option<Arc<dyn Synthesizer>> = synthesizer.map(Arc::from);
            Retriever::with_full(store, vectors, synth)
                .with_markdown_boost(retrieve_cfg.rerank_markdown_boost)
                .recall_full(&q)
                .await
        }
    }
}

/// Try to open the project's vector collection. Returns `None` when the
/// store is disabled in config or when opening it fails (e.g. embedder
/// model download hasn't happened yet) — callers fall back to BM25.
async fn vector_col_from_config(root: &std::path::Path) -> Option<Arc<dyn VectorCol>> {
    let cfg = VectorStoreConfig::load_or_default(root).await;
    if !cfg.enabled {
        return None;
    }
    let path = cfg
        .data_path
        .map(PathBuf::from)
        .unwrap_or_else(|| StoreRoot::vectors_path(root));
    let store = EmbeddedVectorStore::open(&path).await.ok()?;
    let (col, _info) = store.ensure_collection("hoangsa_memory_code").await.ok()?;
    Some(col)
}
