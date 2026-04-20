//! Criterion benchmark for `KvStore::graph_bfs`.
//!
//! The backstory is in commit 217c001 (perf(thoth-graph,thoth-store):
//! single-txn BFS + range-scan edges_from): `graph_bfs` collapses a
//! chatty "one spawn_blocking per frontier item" walk into a single
//! transaction, and the reverse-edge follow-up in this crate turned the
//! `In`/`Both` directions from full edges-table scans into O(matches)
//! range scans. This bench pins numbers to all three directions on a
//! realistic-shape synthetic graph so we can detect regressions the
//! next time the hot path changes.
//!
//! Run with:
//!
//! ```text
//! cargo bench -p thoth-store --bench graph_bfs
//! ```
//!
//! Or with `-- --test` for a smoke run that just exercises each case
//! once (useful as a cheap CI gate).
//!
//! Graph shape: ~1000 nodes, 4-way branching from a designated root,
//! depth 8. That sits comfortably above the "50 nodes, depth 8"
//! scenario the commit message called out as the motivating case.

use criterion::{Criterion, criterion_group, criterion_main};
use tempfile::TempDir;
use thoth_store::{BfsDir, EdgeRow, KvStore, NodeRow};
use tokio::runtime::Runtime;

const BRANCHING: usize = 4;
const LEVELS: usize = 5; // 1 + 4 + 16 + 64 + 256 = 341 nodes — plenty.
const BFS_DEPTH: usize = 8;

/// Build a balanced `BRANCHING`-ary tree of `NodeRow`s + `EdgeRow`s and
/// write it in two batched transactions. Returns the open store plus
/// the designated root fqn.
async fn build_graph() -> (TempDir, KvStore, String) {
    let dir = tempfile::tempdir().unwrap();
    let kv = KvStore::open(dir.path().join("kv.redb")).await.unwrap();

    let mut nodes: Vec<NodeRow> = Vec::new();
    let mut edges: Vec<EdgeRow> = Vec::new();
    let mut current_level: Vec<String> = vec!["root".to_string()];
    nodes.push(NodeRow {
        id: "root".to_string(),
        kind: "function".into(),
        payload: serde_json::json!({}),
    });

    for level in 0..LEVELS {
        let mut next_level: Vec<String> = Vec::with_capacity(current_level.len() * BRANCHING);
        for parent in &current_level {
            for b in 0..BRANCHING {
                let child = format!("n_{level}_{parent}_{b}");
                nodes.push(NodeRow {
                    id: child.clone(),
                    kind: "function".into(),
                    payload: serde_json::json!({}),
                });
                edges.push(EdgeRow {
                    src: parent.clone(),
                    dst: child.clone(),
                    kind: "calls".into(),
                    payload: serde_json::json!({}),
                });
                next_level.push(child);
            }
        }
        current_level = next_level;
    }

    kv.put_nodes_batch(nodes).await.unwrap();
    kv.put_edges_batch(edges).await.unwrap();
    (dir, kv, "root".to_string())
}

fn bench_graph_bfs(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let (_dir, kv, root) = rt.block_on(build_graph());

    let mut group = c.benchmark_group("graph_bfs");
    for dir in [BfsDir::Out, BfsDir::In, BfsDir::Both] {
        // A deep leaf is the worst case for `In` — the reverse walk has
        // to climb back to the root through every ancestor. `Out` from
        // the root is the worst case in the other direction.
        let (start, label) = match dir {
            BfsDir::Out => (root.clone(), "out_from_root"),
            BfsDir::In | BfsDir::Both => {
                // Any leaf: walk down the first branch all the way.
                let mut cur = root.clone();
                for level in 0..LEVELS {
                    cur = format!("n_{level}_{cur}_0");
                }
                (
                    cur,
                    if dir == BfsDir::In {
                        "in_from_leaf"
                    } else {
                        "both_from_leaf"
                    },
                )
            }
        };

        group.bench_function(label, |b| {
            b.to_async(&rt).iter(|| {
                let kv = kv.clone();
                let start = start.clone();
                async move {
                    let out = kv.graph_bfs(start, BFS_DEPTH, dir, None).await.unwrap();
                    // Discourage the optimiser from discarding the result.
                    criterion::black_box(out.len());
                }
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_graph_bfs);
criterion_main!(benches);
