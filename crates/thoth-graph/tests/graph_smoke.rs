//! Unit tests for `thoth-graph` — BFS traversal, impact analysis, and the
//! depth-1 accessor surface. Exercises the graph via a real `KvStore`
//! backed by a `tempdir` so the tests cover the full on-disk round-trip.

use std::path::{Path, PathBuf};

use tempfile::tempdir;
use thoth_graph::{BlastDir, Edge, EdgeKind, Graph, Node};
use thoth_store::KvStore;

async fn new_graph(dir: &Path) -> Graph {
    let kv = KvStore::open(dir.join("graph.redb")).await.unwrap();
    Graph::new(kv)
}

fn node(fqn: &str, path: &str, line: u32) -> Node {
    Node {
        fqn: fqn.to_string(),
        kind: "function".to_string(),
        path: PathBuf::from(path),
        line,
    }
}

fn edge(from: &str, to: &str, kind: EdgeKind) -> Edge {
    Edge {
        from: from.to_string(),
        to: to.to_string(),
        kind,
    }
}

/// Fixture: four nodes + edges covering every EdgeKind + one unresolved.
///
/// ```text
///   a::foo  ──Calls────▶  a::bar  ──Calls──▶  b::baz
///      │                                        ▲
///      └────References────────────────────────┘
///      │
///      └──Imports──▶ external::thing   (unresolved — no node)
///      │
///      └──Extends──▶ a::base
///      │
///      └──DeclaredIn──▶ a                 (module node)
/// ```
async fn build_fixture(g: &Graph) {
    g.upsert_nodes_batch(vec![
        node("a::foo", "a.rs", 10),
        node("a::bar", "a.rs", 20),
        node("b::baz", "b.rs", 5),
        node("a::base", "a.rs", 1),
    ])
    .await
    .unwrap();

    g.upsert_edges_batch(vec![
        edge("a::foo", "a::bar", EdgeKind::Calls),
        edge("a::bar", "b::baz", EdgeKind::Calls),
        edge("a::foo", "b::baz", EdgeKind::References),
        edge("a::foo", "external::thing", EdgeKind::Imports),
        edge("a::foo", "a::base", EdgeKind::Extends),
        edge("a::foo", "a", EdgeKind::DeclaredIn),
    ])
    .await
    .unwrap();
}

#[tokio::test]
async fn edge_kind_tag_round_trip() {
    for k in [
        EdgeKind::Calls,
        EdgeKind::Imports,
        EdgeKind::References,
        EdgeKind::Extends,
        EdgeKind::DeclaredIn,
    ] {
        assert_eq!(EdgeKind::from_tag(k.tag()), Some(k));
    }
    assert_eq!(EdgeKind::from_tag("nonsense"), None);
}

#[tokio::test]
async fn upsert_and_get_node() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;

    assert!(g.get("missing").await.unwrap().is_none());

    let n = node("a::foo", "a.rs", 42);
    g.upsert_node(n.clone()).await.unwrap();

    let back = g.get("a::foo").await.unwrap().unwrap();
    assert_eq!(back.fqn, "a::foo");
    assert_eq!(back.kind, "function");
    assert_eq!(back.path, PathBuf::from("a.rs"));
    assert_eq!(back.line, 42);
}

#[tokio::test]
async fn outgoing_and_incoming_edges() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    let out = g.outgoing("a::foo").await.unwrap();
    assert_eq!(out.len(), 5, "a::foo has 5 outgoing edges (one per kind)");

    let inc = g.incoming("b::baz").await.unwrap();
    let srcs: Vec<_> = inc.iter().map(|e| e.from.as_str()).collect();
    assert!(srcs.contains(&"a::bar"));
    assert!(srcs.contains(&"a::foo"));
}

#[tokio::test]
async fn callees_bfs_depth_bounded() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    // depth=0 yields nothing (start is never emitted).
    assert!(g.callees("a::foo", 0).await.unwrap().is_empty());

    // depth=1 from a::foo via Calls only → a::bar (not a::base, not b::baz).
    let d1: Vec<_> = g
        .callees("a::foo", 1)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.fqn)
        .collect();
    assert_eq!(d1, vec!["a::bar"]);

    // depth=2 → a::bar + b::baz (Calls chain).
    let mut d2: Vec<_> = g
        .callees("a::foo", 2)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.fqn)
        .collect();
    d2.sort();
    assert_eq!(d2, vec!["a::bar", "b::baz"]);
}

#[tokio::test]
async fn callers_walks_reverse_edges() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    let callers: Vec<_> = g
        .callers("b::baz", 2)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.fqn)
        .collect();
    // a::bar calls b::baz directly; a::foo calls a::bar which calls b::baz.
    // References from a::foo → b::baz does NOT count for `callers` (Calls-only).
    assert!(callers.contains(&"a::bar".to_string()));
    assert!(callers.contains(&"a::foo".to_string()));
}

#[tokio::test]
async fn bfs_handles_cycles() {
    // A → B → A loop must terminate and yield {B} for depth≥1.
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;

    g.upsert_nodes_batch(vec![node("m::a", "m.rs", 1), node("m::b", "m.rs", 2)])
        .await
        .unwrap();
    g.upsert_edges_batch(vec![
        edge("m::a", "m::b", EdgeKind::Calls),
        edge("m::b", "m::a", EdgeKind::Calls),
    ])
    .await
    .unwrap();

    let hits: Vec<_> = g
        .callees("m::a", 8)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.fqn)
        .collect();
    assert_eq!(hits, vec!["m::b"], "start node never re-emitted on cycle");
}

#[tokio::test]
async fn impact_up_covers_callers_refs_extends() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    // Up from a::bar: callers via Calls → a::foo (depth 1).
    let hits = g.impact("a::bar", BlastDir::Up, 3).await.unwrap();
    let got: Vec<_> = hits.iter().map(|(n, d)| (n.fqn.as_str(), *d)).collect();
    assert_eq!(got, vec![("a::foo", 1)]);

    // Up from a::base: Extends from a::foo (incoming) → a::foo at depth 1.
    let hits = g.impact("a::base", BlastDir::Up, 3).await.unwrap();
    let got: Vec<_> = hits.iter().map(|(n, d)| (n.fqn.as_str(), *d)).collect();
    assert_eq!(got, vec![("a::foo", 1)]);
}

#[tokio::test]
async fn impact_down_covers_callees_and_parents() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    // Down from a::foo at depth 1: Calls→a::bar, Extends→a::base.
    // (Imports + References + DeclaredIn are NOT in the Down filter.)
    let hits = g.impact("a::foo", BlastDir::Down, 1).await.unwrap();
    let mut got: Vec<_> = hits
        .iter()
        .map(|(n, d)| (n.fqn.clone(), *d))
        .collect::<Vec<_>>();
    got.sort();
    assert_eq!(
        got,
        vec![("a::bar".to_string(), 1), ("a::base".to_string(), 1)]
    );
}

#[tokio::test]
async fn impact_both_unions_directions() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    let hits = g.impact("a::bar", BlastDir::Both, 1).await.unwrap();
    let mut got: Vec<_> = hits.iter().map(|(n, _)| n.fqn.clone()).collect();
    got.sort();
    // Up: a::foo (Calls). Down: b::baz (Calls). Both at depth 1.
    assert_eq!(got, vec!["a::foo".to_string(), "b::baz".to_string()]);
}

#[tokio::test]
async fn impact_depth_zero_returns_empty() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    assert!(
        g.impact("a::foo", BlastDir::Both, 0)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn neighbors_walks_every_edge_kind_both_ways() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    let hits: Vec<_> = g
        .neighbors("a::foo", 1)
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.fqn)
        .collect();
    // Everything a::foo touches directly, excluding unresolved `external::thing`
    // (no node) and `a` (module node IS present only if we add it — we didn't).
    // Resolved depth-1 neighbours: a::bar, b::baz, a::base.
    let mut sorted = hits;
    sorted.sort();
    assert_eq!(sorted, vec!["a::bar", "a::base", "b::baz"]);
}

#[tokio::test]
async fn out_neighbors_and_unresolved_filter_by_kind() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    // Imports: only external::thing — unresolved (no Node upserted).
    let resolved = g.out_neighbors("a::foo", EdgeKind::Imports).await.unwrap();
    assert!(resolved.is_empty(), "unresolved imports are dropped");

    let unresolved = g.out_unresolved("a::foo", EdgeKind::Imports).await.unwrap();
    assert_eq!(unresolved, vec!["external::thing".to_string()]);

    // Calls: a::bar is resolved.
    let calls = g.out_neighbors("a::foo", EdgeKind::Calls).await.unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].fqn, "a::bar");
}

#[tokio::test]
async fn in_neighbors_filters_by_kind() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    let callers = g.in_neighbors("b::baz", EdgeKind::Calls).await.unwrap();
    let fqns: Vec<_> = callers.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["a::bar"]);

    let referrers = g
        .in_neighbors("b::baz", EdgeKind::References)
        .await
        .unwrap();
    let fqns: Vec<_> = referrers.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["a::foo"]);
}

#[tokio::test]
async fn symbols_in_file_lists_declarations() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    let mut in_a: Vec<_> = g
        .symbols_in_file("a.rs")
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.fqn)
        .collect();
    in_a.sort();
    assert_eq!(in_a, vec!["a::bar", "a::base", "a::foo"]);

    let in_b: Vec<_> = g
        .symbols_in_file("b.rs")
        .await
        .unwrap()
        .into_iter()
        .map(|n| n.fqn)
        .collect();
    assert_eq!(in_b, vec!["b::baz"]);
}

#[tokio::test]
async fn purge_path_drops_nodes_and_touching_edges() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    build_fixture(&g).await;

    let (nodes_dropped, edges_dropped) = g.purge_path("a.rs").await.unwrap();
    assert_eq!(nodes_dropped, 3, "a::foo + a::bar + a::base");
    assert!(edges_dropped >= 5, "every edge touching an a.rs symbol");

    assert!(g.get("a::foo").await.unwrap().is_none());
    assert!(g.get("a::bar").await.unwrap().is_none());
    assert!(g.get("a::base").await.unwrap().is_none());
    // b.rs symbol survives.
    assert!(g.get("b::baz").await.unwrap().is_some());
    // Incoming edges to b::baz from the purged file are gone.
    let inc = g.incoming("b::baz").await.unwrap();
    assert!(inc.is_empty(), "all edges from a.rs were purged");
}

/// When the indexer couldn't resolve a callee's bare name through the
/// file-local alias map (`cmd::hook::cmd_enforce(…)` invoked from a
/// different file that only says `use cmd::hook;`), the edge's `dst`
/// stays as the simple leaf `cmd_enforce`. `impact(hook::cmd_enforce,
/// Up)` must still surface those callers — otherwise agents get a false
/// "0 callers, safe to delete" reading on a real cross-file symbol.
#[tokio::test]
async fn impact_up_picks_up_unresolved_bare_name_edges() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;

    g.upsert_nodes_batch(vec![
        node("hook::cmd_enforce", "hook.rs", 10),
        node("main::main", "main.rs", 1),
    ])
    .await
    .unwrap();
    // Edge `to` is the bare `cmd_enforce`, not the canonical
    // `hook::cmd_enforce` — exactly what the indexer would write when
    // the calling file didn't have a `use` that maps `cmd_enforce`.
    g.upsert_edges_batch(vec![edge("main::main", "cmd_enforce", EdgeKind::Calls)])
        .await
        .unwrap();

    let hits = g.impact("hook::cmd_enforce", BlastDir::Up, 3).await.unwrap();
    let got: Vec<_> = hits.iter().map(|(n, d)| (n.fqn.as_str(), *d)).collect();
    assert_eq!(
        got,
        vec![("main::main", 1)],
        "leaf-fallback BFS must surface callers keyed on the bare name"
    );

    // Symmetric: `in_neighbors(Calls)` serves the same role for
    // `symbol_context`, and must not miss the bare-name edge either.
    let callers = g
        .in_neighbors("hook::cmd_enforce", EdgeKind::Calls)
        .await
        .unwrap();
    let fqns: Vec<_> = callers.iter().map(|n| n.fqn.as_str()).collect();
    assert_eq!(fqns, vec!["main::main"]);
}

/// `resolve_fqn` folds `cli::cmd::rule::foo` into the graph key
/// `rule::foo` when exactly one node shares that suffix — the FQN
/// friction the bug report called out.
#[tokio::test]
async fn resolve_fqn_suffix_matches_when_unique() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;
    g.upsert_node(node("rule::cmd_rule_add", "rule.rs", 100))
        .await
        .unwrap();

    let resolved = g
        .resolve_fqn("cli::cmd::rule::cmd_rule_add")
        .await
        .unwrap()
        .expect("unique suffix should resolve");
    assert_eq!(resolved.fqn, "rule::cmd_rule_add");

    // Ambiguous → None (caller renders the list via find_suffix_candidates).
    g.upsert_node(node("other::cmd_rule_add", "other.rs", 1))
        .await
        .unwrap();
    let resolved = g.resolve_fqn("cmd_rule_add").await.unwrap();
    assert!(
        resolved.is_none(),
        "ambiguous suffix must not silently pick one"
    );

    let cands = g.find_suffix_candidates("cmd_rule_add").await.unwrap();
    assert_eq!(cands.len(), 2);
}

#[tokio::test]
async fn imports_of_file_dedupes_and_includes_file_stem() {
    let dir = tempdir().unwrap();
    let g = new_graph(dir.path()).await;

    g.upsert_node(node("file::sym", "file.rs", 1))
        .await
        .unwrap();
    g.upsert_edges_batch(vec![
        edge("file::sym", "ext::one", EdgeKind::Imports),
        // Duplicate destination from the file-stem pseudo-source — must dedupe.
        edge("file", "ext::one", EdgeKind::Imports),
        edge("file", "ext::two", EdgeKind::Imports),
    ])
    .await
    .unwrap();

    let mut imports = g.imports_of_file("file.rs").await.unwrap();
    imports.sort();
    assert_eq!(
        imports,
        vec!["ext::one".to_string(), "ext::two".to_string()]
    );
}
