//! Criterion benchmark stubs for `KvStore` read/write operations.
//!
//! Measures p50 / p95 latency for the hot `put_node` (upsert) and
//! `get_node` paths that every recall cycle exercises. Run with:
//!
//! ```text
//! cargo bench -p thoth-store --bench kv_ops
//! ```
//!
//! Or with `-- --test` for a single-iteration smoke run suitable for CI.

use criterion::{Criterion, criterion_group, criterion_main};
use tempfile::TempDir;
use thoth_store::{KvStore, NodeRow};
use tokio::runtime::Runtime;

// ---------------------------------------------------------------------------
// Shared setup
// ---------------------------------------------------------------------------

/// Open a fresh `KvStore` in a temp directory and pre-populate it with
/// `n` nodes so that `get_node` benchmarks hit a realistic-size B-tree.
async fn build_store(n: usize) -> (TempDir, KvStore) {
    let dir = tempfile::tempdir().unwrap();
    let kv = KvStore::open(dir.path().join("kv.redb")).await.unwrap();

    let nodes: Vec<NodeRow> = (0..n)
        .map(|i| NodeRow {
            id: format!("bench::node_{i}"),
            kind: "function".into(),
            payload: serde_json::json!({ "index": i }),
        })
        .collect();

    kv.put_nodes_batch(nodes).await.unwrap();
    (dir, kv)
}

// ---------------------------------------------------------------------------
// Benchmark: put_node (single-row upsert)
// ---------------------------------------------------------------------------

fn bench_put_node(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (_dir, kv) = rt.block_on(build_store(100));

    let mut group = c.benchmark_group("kv_ops");
    group.sample_size(50);

    group.bench_function("put_node", |b| {
        let mut counter: u64 = 0;
        b.to_async(&rt).iter(|| {
            counter += 1;
            let kv = kv.clone();
            async move {
                let row = NodeRow {
                    id: format!("bench::write_{counter}"),
                    kind: "function".into(),
                    payload: serde_json::json!({}),
                };
                kv.put_node(row).await.unwrap();
            }
        });
    });

    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: get_node (point lookup)
// ---------------------------------------------------------------------------

fn bench_get_node(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (_dir, kv) = rt.block_on(build_store(100));

    let mut group = c.benchmark_group("kv_ops");
    group.sample_size(50);

    group.bench_function("get_node", |b| {
        b.to_async(&rt).iter(|| {
            let kv = kv.clone();
            async move {
                // Look up a node that exists (mid-range key).
                let result = kv.get_node("bench::node_50").await.unwrap();
                criterion::black_box(result);
            }
        });
    });

    group.finish();
}

criterion_group!(benches, bench_put_node, bench_get_node);
criterion_main!(benches);
