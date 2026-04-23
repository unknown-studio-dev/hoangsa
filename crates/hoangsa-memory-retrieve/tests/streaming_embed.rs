//! Streaming embed pipeline test.
//!
//! Exercises [`Indexer::index_path`] with a mock [`VectorCol`] to verify
//! that parsed chunks flow through the bounded channel → batched embed
//! path and that `stats.embedded` reflects every chunk the mock saw,
//! including partial final batches.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use hoangsa_memory_core::Result as MemResult;
use hoangsa_memory_parse::LanguageRegistry;
use hoangsa_memory_retrieve::Indexer;
use hoangsa_memory_store::{StoreRoot, VectorCol, VectorHit};
use parking_lot::Mutex;
use serde_json::Value;
use tempfile::tempdir;

/// Minimal `VectorCol` that records every `upsert` call so tests can
/// assert on batch count and total ids seen. All other methods fail
/// loudly — they must not be reached by [`Indexer::index_path`].
#[derive(Default)]
struct RecordingVectorCol {
    batches: Mutex<Vec<usize>>,
}

impl RecordingVectorCol {
    fn batch_sizes(&self) -> Vec<usize> {
        self.batches.lock().clone()
    }

    fn total_ids(&self) -> usize {
        self.batches.lock().iter().sum()
    }
}

#[async_trait]
impl VectorCol for RecordingVectorCol {
    async fn upsert(
        &self,
        ids: Vec<String>,
        _documents: Option<Vec<String>>,
        _metadatas: Option<Vec<HashMap<String, Value>>>,
    ) -> MemResult<()> {
        self.batches.lock().push(ids.len());
        Ok(())
    }

    async fn query_text(
        &self,
        _text: &str,
        _n_results: usize,
        _where_filter: Option<Value>,
    ) -> MemResult<Vec<VectorHit>> {
        unreachable!("indexer must not call query_text")
    }

    async fn delete(&self, _ids: Vec<String>) -> MemResult<()> {
        // Called by `purge_path` when re-indexing; benign no-op.
        Ok(())
    }

    async fn delete_by_filter(&self, _where_filter: Value) -> MemResult<()> {
        // Called by `purge_path` when re-indexing; benign no-op.
        Ok(())
    }

    async fn count(&self) -> MemResult<usize> {
        Ok(self.batches.lock().iter().sum())
    }
}

/// Write `n` distinct `.rs` files under `dir`, each with a single
/// top-level function so the tree-sitter pass extracts exactly one
/// chunk per file. Returns the files written.
async fn write_one_fn_per_file(dir: &std::path::Path, n: usize) {
    for i in 0..n {
        let body = format!(
            "pub fn item_{i}(x: i32) -> i32 {{\n    x + {i}\n}}\n"
        );
        tokio::fs::write(dir.join(format!("item_{i}.rs")), body)
            .await
            .unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streaming_embed_visits_every_chunk_across_batches() {
    // 300 files with 1 chunk each ⇒ ≥ 2 batches at the 256 batch
    // boundary, guaranteeing we exercise both the mid-loop drain and
    // the tail flush.
    let src_dir = tempdir().unwrap();
    write_one_fn_per_file(src_dir.path(), 300).await;

    let memory_dir = tempdir().unwrap();
    let store = StoreRoot::open(memory_dir.path()).await.unwrap();

    let vector: Arc<RecordingVectorCol> = Arc::new(RecordingVectorCol::default());
    let idx = Indexer::new(store, LanguageRegistry::new())
        .with_vector_store(vector.clone() as Arc<dyn VectorCol>);

    let stats = idx.index_path(src_dir.path()).await.unwrap();

    // stats.embedded is the total ids the mock saw.
    assert_eq!(
        stats.embedded,
        vector.total_ids(),
        "stats.embedded ({}) must equal mock ids ({})",
        stats.embedded,
        vector.total_ids()
    );
    // At least one chunk per file.
    assert!(
        stats.embedded >= 300,
        "expected ≥ 300 embedded chunks, got {}",
        stats.embedded
    );
    // Multi-batch: final batch may be partial, so `batches` > 1 and
    // every non-final batch is capped at EMBED_BATCH_SIZE (256).
    let sizes = vector.batch_sizes();
    assert!(
        sizes.len() >= 2,
        "streaming should produce ≥ 2 batches, got {sizes:?}"
    );
    for (i, &sz) in sizes.iter().enumerate() {
        assert!(sz > 0, "batch {i} was empty");
        if i + 1 < sizes.len() {
            assert_eq!(
                sz, 256,
                "non-final batch {i} was {sz}, expected 256"
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn streaming_embed_flushes_tail_below_batch_size() {
    // Small tree: far fewer than 256 chunks → everything rides the
    // tail-flush branch.
    let src_dir = tempdir().unwrap();
    write_one_fn_per_file(src_dir.path(), 5).await;

    let memory_dir = tempdir().unwrap();
    let store = StoreRoot::open(memory_dir.path()).await.unwrap();

    let vector: Arc<RecordingVectorCol> = Arc::new(RecordingVectorCol::default());
    let idx = Indexer::new(store, LanguageRegistry::new())
        .with_vector_store(vector.clone() as Arc<dyn VectorCol>);

    let stats = idx.index_path(src_dir.path()).await.unwrap();

    let sizes = vector.batch_sizes();
    assert_eq!(
        sizes.len(),
        1,
        "5 chunks should fit in a single tail batch, got {sizes:?}"
    );
    assert_eq!(stats.embedded, sizes[0]);
    assert!(stats.embedded >= 5);
}
