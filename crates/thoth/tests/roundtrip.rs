//! E2E integration tests: index → recall → remember → verify roundtrip.
//!
//! REQ-04: Covers the full CodeMemory façade pipeline using Mode::Zero
//! (no external services required).

use tempfile::tempdir;
use thoth::{CodeMemory, Mode, Query};

/// Full roundtrip: index a source file, recall by symbol, remember a fact,
/// recall the fact back.
#[tokio::test]
async fn roundtrip_index_recall_remember() {
    // 1. Create temp dirs: one for the .thoth store, one for the source tree.
    let thoth_dir = tempdir().unwrap();
    let src_dir = tempdir().unwrap();

    // 2. Write a sample .rs file with a known function name.
    let sample = src_dir.path().join("sample.rs");
    tokio::fs::write(
        &sample,
        r#"/// Computes the frobnication factor.
pub fn frobnicate(x: u32) -> u32 {
    x * 42
}

/// Returns the answer to everything.
pub fn answer() -> u32 {
    42
}
"#,
    )
    .await
    .unwrap();

    // 3. Open CodeMemory.
    let mem = CodeMemory::open(thoth_dir.path()).await.unwrap();

    // 4. Index the sample source directory.
    let stats = mem.index(src_dir.path()).await.unwrap();
    // At least one file should have been touched.
    assert!(
        stats.files >= 1,
        "expected at least 1 file indexed, got {}",
        stats.files
    );

    // 5. Recall the function name — verify chunks are returned.
    let recall = mem
        .recall(Query::text("frobnicate"), Mode::Zero)
        .await
        .unwrap();
    assert!(
        !recall.chunks.is_empty(),
        "expected at least one chunk for 'frobnicate', got none"
    );
    // At least one chunk should reference the sample file.
    let hit = recall
        .chunks
        .iter()
        .any(|c| c.path.to_string_lossy().contains("sample.rs"));
    assert!(hit, "expected a chunk from sample.rs in recall results");

    // 6. Remember a fact.
    mem.remember_fact("test fact about foo", vec!["test".into()])
        .await
        .unwrap();

    // 7. Recall "test fact" — verify the fact appears in results.
    let fact_recall = mem
        .recall(Query::text("test fact about foo"), Mode::Zero)
        .await
        .unwrap();
    assert!(
        !fact_recall.chunks.is_empty(),
        "expected at least one chunk after remembering a fact, got none"
    );
    let fact_hit = fact_recall
        .chunks
        .iter()
        .any(|c| c.preview.contains("test fact") || c.body.contains("test fact"));
    assert!(
        fact_hit,
        "expected 'test fact' to appear in recall chunks; got: {:?}",
        fact_recall
            .chunks
            .iter()
            .map(|c| &c.preview)
            .collect::<Vec<_>>()
    );
}

/// Smoke test: an empty directory should index cleanly and recall should not panic.
#[tokio::test]
async fn roundtrip_empty_directory() {
    // 1. Create empty temp dirs.
    let thoth_dir = tempdir().unwrap();
    let src_dir = tempdir().unwrap();

    // 2. Open CodeMemory.
    let mem = CodeMemory::open(thoth_dir.path()).await.unwrap();

    // 3. Index empty dir — should return stats with 0 files.
    let stats = mem.index(src_dir.path()).await.unwrap();
    assert_eq!(
        stats.files, 0,
        "expected 0 files indexed in empty dir, got {}",
        stats.files
    );

    // 4. Recall anything → should return empty results, no crash.
    let recall = mem
        .recall(Query::text("anything at all"), Mode::Zero)
        .await
        .unwrap();
    // An empty store should return zero chunks without panicking.
    assert_eq!(
        recall.chunks.len(),
        0,
        "expected 0 chunks from empty store, got {}",
        recall.chunks.len()
    );
}
