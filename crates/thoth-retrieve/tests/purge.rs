//! Integration tests covering [`Indexer::purge_path`] and the
//! purge-before-write path of [`Indexer::index_file`].
//!
//! The reindex pipeline previously leaked state in three ways:
//!   1. the BM25 writer was never committed after a per-file reindex,
//!   2. `index_file` only upserted — stale chunks whose line ranges had
//!      shifted (or whose symbols had been deleted) lingered forever,
//!   3. `Event::FileDeleted` still went through `index_file`, which tried
//!      to reparse a missing file and swallowed the error.
//!
//! These tests pin the fixes by asserting against the underlying stores
//! directly — going through the retriever would conflate FTS reader refresh
//! latency with the purge contract we're trying to lock down.
//!
//! Note: the rust parser builds FQNs as `"<module>::<name>"` where module
//! is the file stem (e.g. `auth.rs` → `auth::sign_token`). We rely on that
//! here to scope assertions per-symbol.

use tempfile::tempdir;
use thoth_parse::LanguageRegistry;
use thoth_retrieve::Indexer;
use thoth_store::StoreRoot;

const V1: &str = r#"
/// verify_token stub — early revision
pub fn verify_token(jwt: &str) -> bool {
    !jwt.is_empty()
}

/// sign_token will be removed in v2.
pub fn sign_token(claims: &str) -> String {
    format!("jwt::{claims}")
}
"#;

const V2: &str = r#"
/// verify_token stub — updated revision (same symbol, different body).
pub fn verify_token(jwt: &str) -> bool {
    !jwt.trim().is_empty()
}

// sign_token has been removed entirely — only verify_token remains.
"#;

#[tokio::test]
async fn reindex_drops_stale_symbols_and_nodes() {
    use thoth_graph::Graph;

    let src_dir = tempdir().unwrap();
    let file = src_dir.path().join("auth.rs");
    tokio::fs::write(&file, V1).await.unwrap();

    let thoth_dir = tempdir().unwrap();
    let store = StoreRoot::open(thoth_dir.path()).await.unwrap();
    let idx = Indexer::new(store.clone(), LanguageRegistry::new());
    let g = Graph::new(store.kv.clone());

    // Initial index of V1 — both symbols must land.
    idx.index_file(&file).await.unwrap();
    idx.commit().await.unwrap();

    let after_v1: Vec<String> = store
        .kv
        .symbols_with_prefix("")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.fqn)
        .collect();
    assert!(
        after_v1.iter().any(|f| f.ends_with("sign_token")),
        "sign_token missing from KV after v1: {after_v1:?}",
    );
    assert!(
        after_v1.iter().any(|f| f.ends_with("verify_token")),
        "verify_token missing from KV after v1: {after_v1:?}",
    );
    // Graph nodes exist for both.
    for fqn in &after_v1 {
        assert!(
            g.get(fqn).await.unwrap().is_some(),
            "graph node missing for {fqn} after v1",
        );
    }

    // Rewrite to V2 (drops sign_token) and re-index that path.
    tokio::fs::write(&file, V2).await.unwrap();
    idx.index_file(&file).await.unwrap();
    idx.commit().await.unwrap();

    let after_v2: Vec<String> = store
        .kv
        .symbols_with_prefix("")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.fqn)
        .collect();
    assert!(
        !after_v2.iter().any(|f| f.ends_with("sign_token")),
        "stale sign_token survived reindex: {after_v2:?}",
    );
    assert!(
        after_v2.iter().any(|f| f.ends_with("verify_token")),
        "verify_token should survive reindex: {after_v2:?}",
    );
    // And the graph node for the dropped symbol is gone.
    let stale_fqn = after_v1
        .iter()
        .find(|f| f.ends_with("sign_token"))
        .cloned()
        .expect("sanity: sign_token must have existed after v1");
    assert!(
        g.get(&stale_fqn).await.unwrap().is_none(),
        "graph node {stale_fqn} survived reindex",
    );
}

#[tokio::test]
async fn purge_path_clears_kv_and_graph() {
    use thoth_graph::Graph;

    let src_dir = tempdir().unwrap();
    let file = src_dir.path().join("auth.rs");
    tokio::fs::write(&file, V1).await.unwrap();

    let thoth_dir = tempdir().unwrap();
    let store = StoreRoot::open(thoth_dir.path()).await.unwrap();

    let idx = Indexer::new(store.clone(), LanguageRegistry::new());
    idx.index_file(&file).await.unwrap();
    idx.commit().await.unwrap();

    let before_fqns: Vec<String> = store
        .kv
        .symbols_with_prefix("")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.fqn)
        .collect();
    assert!(!before_fqns.is_empty(), "indexer wrote no symbols for v1");

    // Delete on disk, then purge.
    tokio::fs::remove_file(&file).await.unwrap();
    idx.purge_path(&file).await.unwrap();
    idx.commit().await.unwrap();

    // KV: no symbol row references this path.
    let after = store.kv.symbols_with_prefix("").await.unwrap();
    let still_here: Vec<_> = after.iter().filter(|r| r.path == file).collect();
    assert!(
        still_here.is_empty(),
        "symbol rows for purged path survived: {still_here:#?}",
    );

    // Graph: every FQN we saw before is gone.
    let g = Graph::new(store.kv.clone());
    for fqn in &before_fqns {
        assert!(
            g.get(fqn).await.unwrap().is_none(),
            "graph node {fqn} survived purge",
        );
    }
}

/// Content-hash gating — re-indexing an unchanged file must not re-parse,
/// re-write FTS, or clobber symbol rows. We pin the contract by seeding a
/// sentinel symbol row *after* the first index, then running `index_file`
/// again and asserting the sentinel survives. If the second pass had
/// purged + rewritten, the sentinel would be gone.
#[tokio::test]
async fn reindex_skips_when_content_hash_unchanged() {
    use thoth_store::SymbolRow;

    let src_dir = tempdir().unwrap();
    let file = src_dir.path().join("auth.rs");
    tokio::fs::write(&file, V1).await.unwrap();

    let thoth_dir = tempdir().unwrap();
    let store = StoreRoot::open(thoth_dir.path()).await.unwrap();
    let idx = Indexer::new(store.clone(), LanguageRegistry::new());

    // First pass — should populate.
    idx.index_file(&file).await.unwrap();
    idx.commit().await.unwrap();

    let after_v1: Vec<String> = store
        .kv
        .symbols_with_prefix("")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.fqn)
        .collect();
    assert!(!after_v1.is_empty(), "first pass indexed nothing");

    // Plant a sentinel row keyed on the file's path. If the second index
    // pass short-circuits (hash unchanged) this must survive; if it
    // re-runs the pipeline, `purge_path` will nuke it alongside the real
    // symbols for this file.
    store
        .kv
        .put_symbol(SymbolRow {
            fqn: "auth::__hash_sentinel".to_string(),
            path: file.clone(),
            start_line: 999,
            end_line: 999,
            kind: "function".to_string(),
        })
        .await
        .unwrap();

    // Re-index with identical bytes. This must short-circuit on the hash
    // check without purging.
    idx.index_file(&file).await.unwrap();
    idx.commit().await.unwrap();

    let after_v1_again: Vec<String> = store
        .kv
        .symbols_with_prefix("")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.fqn)
        .collect();
    assert!(
        after_v1_again.iter().any(|f| f == "auth::__hash_sentinel"),
        "sentinel was wiped — hash gating failed: {after_v1_again:?}",
    );

    // Sanity: a real edit *must* bust the cache and re-run the pipeline,
    // which will purge the sentinel along with the stale symbols.
    tokio::fs::write(&file, V2).await.unwrap();
    idx.index_file(&file).await.unwrap();
    idx.commit().await.unwrap();
    let after_v2: Vec<String> = store
        .kv
        .symbols_with_prefix("")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.fqn)
        .collect();
    assert!(
        !after_v2.iter().any(|f| f == "auth::__hash_sentinel"),
        "sentinel survived a real content change — hash gate over-matched: {after_v2:?}",
    );
}
