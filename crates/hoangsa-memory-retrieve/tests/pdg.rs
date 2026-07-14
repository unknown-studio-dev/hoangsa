//! Integration tests for PDG opt-in in the indexer.
//!
//! Pins two contracts:
//!   1. Default index → zero "stmt" nodes (PDG disabled).
//!   2. purge_path drops stmt nodes + edges written by the PDG pass.

use tempfile::tempdir;
use hoangsa_memory_graph::Graph;
use hoangsa_memory_parse::LanguageRegistry;
use hoangsa_memory_retrieve::Indexer;
use hoangsa_memory_store::StoreRoot;

const RUST_FN: &str = r#"
pub fn compute(a: i32, b: i32) -> i32 {
    let x = a + b;
    let y = x * 2;
    y
}
"#;

/// Indexing without `.with_pdg(true)` must produce zero "stmt" nodes.
#[tokio::test]
async fn indexer_default_writes_no_stmt_nodes() {
    let src_dir = tempdir().expect("src tempdir");
    let file = src_dir.path().join("compute.rs");
    tokio::fs::write(&file, RUST_FN).await.expect("write file");

    let mem_dir = tempdir().expect("mem tempdir");
    let store = StoreRoot::open(mem_dir.path()).await.expect("open store");
    let g = Graph::new(store.kv.clone());

    // Default indexer — PDG disabled.
    let idx = Indexer::new(store.clone(), LanguageRegistry::new());
    idx.index_file(&file).await.expect("index_file");
    idx.commit().await.expect("commit");

    let nodes = g.symbols_in_file(&file).await.expect("symbols_in_file");
    let stmt_nodes: Vec<_> = nodes.iter().filter(|n| n.kind == "stmt").collect();
    assert!(
        stmt_nodes.is_empty(),
        "expected 0 stmt nodes with default indexer, got: {stmt_nodes:?}",
    );
}

/// purge_path must remove stmt nodes written by the PDG pass.
#[tokio::test]
async fn purge_path_removes_stmt_nodes() {
    let src_dir = tempdir().expect("src tempdir");
    let file = src_dir.path().join("compute.rs");
    tokio::fs::write(&file, RUST_FN).await.expect("write file");

    let mem_dir = tempdir().expect("mem tempdir");
    let store = StoreRoot::open(mem_dir.path()).await.expect("open store");
    let g = Graph::new(store.kv.clone());

    // Index with PDG enabled.
    let idx = Indexer::new(store.clone(), LanguageRegistry::new()).with_pdg(true);
    idx.index_file(&file).await.expect("index_file");
    idx.commit().await.expect("commit");

    // Verify stmt nodes were written.
    let before = g.symbols_in_file(&file).await.expect("symbols_in_file before");
    let stmt_before: Vec<_> = before.iter().filter(|n| n.kind == "stmt").collect();
    assert!(
        !stmt_before.is_empty(),
        "expected stmt nodes after PDG-enabled index, got none; all nodes: {before:?}",
    );

    // Purge the file.
    idx.purge_path(&file).await.expect("purge_path");
    idx.commit().await.expect("commit after purge");

    // Verify stmt nodes are gone.
    let after = g.symbols_in_file(&file).await.expect("symbols_in_file after");
    let stmt_after: Vec<_> = after.iter().filter(|n| n.kind == "stmt").collect();
    assert!(
        stmt_after.is_empty(),
        "stmt nodes survived purge: {stmt_after:?}",
    );
}
