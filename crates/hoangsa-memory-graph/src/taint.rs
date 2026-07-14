use std::collections::{HashMap, HashSet, VecDeque};

use hoangsa_memory_core::Result;
use serde::{Deserialize, Serialize};

use crate::{Edge, EdgeKind, Graph, Node};

/// Parameters for a taint-reachability query.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaintSpec {
    /// Substring patterns for source nodes (matched against FQN and payload text).
    pub sources: Vec<String>,
    /// Substring patterns for sink nodes (matched against FQN and payload text).
    pub sinks: Vec<String>,
    /// Maximum BFS depth from each source node.
    pub max_depth: usize,
    /// Stop recording findings after this many; sets `TaintReport::truncated`.
    pub max_findings: usize,
}

/// A single taint path from source to sink.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaintFinding {
    /// Source node where taint originates.
    pub source: Node,
    /// Sink node where taint arrives.
    pub sink: Node,
    /// Ordered sequence of edges on the path from source to sink.
    pub path: Vec<Edge>,
}

/// Result of a [`Graph::taint_paths`] call.
#[derive(Debug, Serialize, Deserialize)]
pub struct TaintReport {
    /// All found taint paths, deduped and sorted by (source.fqn, sink.fqn).
    pub findings: Vec<TaintFinding>,
    /// `true` when `max_findings` was reached and further paths were dropped.
    pub truncated: bool,
    /// Number of graph nodes that matched at least one source pattern.
    pub source_matches: usize,
    /// Number of graph nodes that matched at least one sink pattern.
    pub sink_matches: usize,
}

impl Default for TaintSpec {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            sinks: Vec::new(),
            max_depth: 12,
            max_findings: 50,
        }
    }
}

/// Returns `true` when `node` matches `pattern` — case-insensitive substring
/// match checked against FQN and the optional `text` field in the node's KV
/// payload. The payload is fetched once (from the `NodeRow`) and passed in
/// here as `text`.
fn node_matches(fqn: &str, text: Option<&str>, pattern: &str) -> bool {
    let p = pattern.to_lowercase();
    fqn.to_lowercase().contains(&p)
        || text.is_some_and(|t| t.to_lowercase().contains(&p))
}

impl Graph {
    /// Find all data-dependency taint paths from nodes matching `spec.sources`
    /// to nodes matching `spec.sinks`.
    ///
    /// The BFS follows only [`EdgeKind::DataDep`] and [`EdgeKind::Calls`] edges
    /// — never [`EdgeKind::Cfg`] — to avoid false positives through pure
    /// control flow.
    pub async fn taint_paths(&self, spec: &TaintSpec) -> Result<TaintReport> {
        // Collect all node IDs for the source/sink scan.
        let mut all_ids: Vec<String> = self.kv.all_node_ids().await?;
        all_ids.sort();

        // Fetch text payload for a node FQN.
        let text_for = |row: &hoangsa_memory_store::NodeRow| -> Option<String> {
            row.payload.get("text").and_then(|v| v.as_str()).map(|s| s.to_string())
        };

        // Classify every node as source / sink (a node can be both).
        let mut source_nodes: Vec<Node> = Vec::new();
        let mut sink_set: HashSet<String> = HashSet::new();
        let mut sink_nodes: HashMap<String, Node> = HashMap::new();

        for fqn in &all_ids {
            // Fetch the raw row so we can read the payload text field.
            let row = match self.kv.get_node(fqn).await? {
                Some(r) => r,
                None => continue,
            };
            let text = text_for(&row);
            let text_ref = text.as_deref();
            let node = crate::row_to_node(row.clone());

            let is_source = spec.sources.iter().any(|p| node_matches(fqn, text_ref, p));
            let is_sink = spec.sinks.iter().any(|p| node_matches(fqn, text_ref, p));

            if is_source {
                source_nodes.push(node.clone());
            }
            if is_sink {
                sink_set.insert(fqn.clone());
                sink_nodes.insert(fqn.clone(), node);
            }
        }

        let source_matches = source_nodes.len();
        let sink_matches = sink_set.len();

        // BFS from each source over DataDep + Calls edges only.
        // Collect findings keyed by (source_fqn, sink_fqn) for dedup.
        let taint_kinds: &[EdgeKind] = &[EdgeKind::DataDep, EdgeKind::Calls];

        let mut seen_pairs: HashSet<(String, String)> = HashSet::new();
        let mut findings: Vec<TaintFinding> = Vec::new();
        let mut truncated = false;

        // Sort source nodes for deterministic output.
        let mut sorted_sources = source_nodes.clone();
        sorted_sources.sort_by(|a, b| a.fqn.cmp(&b.fqn));

        'sources: for source in &sorted_sources {
            if findings.len() >= spec.max_findings {
                truncated = true;
                break;
            }

            // BFS: node_fqn -> (parent_fqn, edge_used)
            let mut parent: HashMap<String, (String, Edge)> = HashMap::new();
            let mut visited: HashSet<String> = HashSet::new();
            visited.insert(source.fqn.clone());

            // Queue entries: (fqn, depth)
            let mut queue: VecDeque<(String, usize)> = VecDeque::new();
            queue.push_back((source.fqn.clone(), 0));

            while let Some((cur, depth)) = queue.pop_front() {
                if depth >= spec.max_depth {
                    continue;
                }

                // Collect outgoing taint-relevant edges in sorted order.
                let mut next: Vec<(String, Edge)> = Vec::new();
                for e in self.outgoing(&cur).await? {
                    if taint_kinds.contains(&e.kind) {
                        next.push((e.to.clone(), e));
                    }
                }
                next.sort_by(|a, b| a.0.cmp(&b.0));

                for (nfqn, edge) in next {
                    if visited.insert(nfqn.clone()) {
                        parent.insert(nfqn.clone(), (cur.clone(), edge));

                        if sink_set.contains(&nfqn) {
                            let pair = (source.fqn.clone(), nfqn.clone());
                            if seen_pairs.insert(pair) {
                                // Reconstruct path.
                                let path = reconstruct_path(&parent, &source.fqn, &nfqn);
                                let sink_node = sink_nodes[&nfqn].clone();
                                findings.push(TaintFinding {
                                    source: source.clone(),
                                    sink: sink_node,
                                    path,
                                });
                                if findings.len() >= spec.max_findings {
                                    truncated = true;
                                    break 'sources;
                                }
                            }
                        }

                        queue.push_back((nfqn, depth + 1));
                    }
                }
            }
        }

        // Sort findings by (source.fqn, sink.fqn) for determinism.
        findings.sort_by(|a, b| a.source.fqn.cmp(&b.source.fqn).then_with(|| a.sink.fqn.cmp(&b.sink.fqn)));

        Ok(TaintReport {
            findings,
            truncated,
            source_matches,
            sink_matches,
        })
    }
}

fn reconstruct_path(
    parent: &HashMap<String, (String, Edge)>,
    start: &str,
    goal: &str,
) -> Vec<Edge> {
    let mut path: Vec<Edge> = Vec::new();
    let mut cur = goal.to_string();
    while let Some((prev, edge)) = parent.get(&cur) {
        path.push(edge.clone());
        if prev == start {
            break;
        }
        cur = prev.clone();
    }
    path.reverse();
    path
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoangsa_memory_store::KvStore;
    use std::path::PathBuf;

    async fn make_graph() -> (Graph, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let kv = KvStore::open(&db_path).await.expect("KvStore::open");
        (Graph::new(kv), dir)
    }

    fn node(fqn: &str) -> Node {
        Node {
            fqn: fqn.to_string(),
            kind: "binding".to_string(),
            path: PathBuf::from("src/lib.rs"),
            line: 1,
        }
    }

    fn edge(from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            kind,
        }
    }

    /// Basic: source -DataDep-> sink; BFS must find the path.
    #[tokio::test]
    async fn taint_finds_simple_source_sink_path() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("source_node")).await.expect("upsert source");
        g.upsert_node(node("sink_node")).await.expect("upsert sink");
        g.upsert_edge(edge("source_node", "sink_node", EdgeKind::DataDep))
            .await
            .expect("source->sink DataDep");

        let spec = TaintSpec {
            sources: vec!["source_node".to_string()],
            sinks: vec!["sink_node".to_string()],
            max_depth: 5,
            max_findings: 100,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert_eq!(report.findings.len(), 1, "must find exactly one path");
        let f = &report.findings[0];
        assert_eq!(f.source.fqn, "source_node");
        assert_eq!(f.sink.fqn, "sink_node");
        assert_eq!(f.path.len(), 1, "direct single-hop path");
        assert_eq!(f.path[0].from, "source_node");
        assert_eq!(f.path[0].to, "sink_node");
        assert!(!report.truncated);
        assert_eq!(report.source_matches, 1);
        assert_eq!(report.sink_matches, 1);
    }

    /// Regression: source/sink patterns must match the stmt node's payload
    /// `text`, not just its FQN. The stmt FQN (`fn#s<line>`) never contains
    /// the source token (`env::var`); only the statement text does. If the
    /// indexer writes stmt nodes without carrying text into the payload
    /// (the bug this guards), source_matches drops to 0 and taint goes dark.
    #[tokio::test]
    async fn taint_matches_stmt_payload_text_not_just_fqn() {
        let (g, _dir) = make_graph().await;
        g.upsert_stmt_nodes_batch(vec![
            (
                "app::handler#s3".to_string(),
                PathBuf::from("src/app.rs"),
                3,
                "let cmd = std::env::var(\"CMD\").unwrap();".to_string(),
            ),
            (
                "app::handler#s4".to_string(),
                PathBuf::from("src/app.rs"),
                4,
                "let out = Command::new(cmd).spawn();".to_string(),
            ),
        ])
        .await
        .expect("upsert stmt nodes with text");
        g.upsert_edge(edge("app::handler#s3", "app::handler#s4", EdgeKind::DataDep))
            .await
            .expect("def->use DataDep");

        let spec = TaintSpec {
            sources: vec!["env::var".to_string()],
            sinks: vec!["Command::new".to_string()],
            max_depth: 5,
            max_findings: 100,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert_eq!(report.source_matches, 1, "source must match on payload text");
        assert_eq!(report.sink_matches, 1, "sink must match on payload text");
        assert_eq!(report.findings.len(), 1, "one text-matched source→sink flow");
        assert_eq!(report.findings[0].source.fqn, "app::handler#s3");
        assert_eq!(report.findings[0].sink.fqn, "app::handler#s4");
    }

    /// Control-flow-only reachability must NOT produce a finding.
    #[tokio::test]
    async fn taint_ignores_cfg_only_reachability() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("cfg_source")).await.expect("upsert cfg_source");
        g.upsert_node(node("cfg_sink")).await.expect("upsert cfg_sink");
        // Only a Cfg edge — the taint engine must NOT follow it.
        g.upsert_edge(edge("cfg_source", "cfg_sink", EdgeKind::Cfg))
            .await
            .expect("cfg_source->cfg_sink Cfg");

        let spec = TaintSpec {
            sources: vec!["cfg_source".to_string()],
            sinks: vec!["cfg_sink".to_string()],
            max_depth: 5,
            max_findings: 100,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert!(
            report.findings.is_empty(),
            "Cfg edge must not create a taint finding"
        );
        assert!(!report.truncated);
        assert_eq!(report.source_matches, 1);
        assert_eq!(report.sink_matches, 1);
    }

    /// Taint crosses a Calls bridge: stmt -DataDep-> middle -Calls-> sink.
    #[tokio::test]
    async fn taint_bridges_call_args_across_functions() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("stmt")).await.expect("upsert stmt");
        g.upsert_node(node("callee")).await.expect("upsert callee");
        g.upsert_node(node("param_sink")).await.expect("upsert param_sink");
        g.upsert_edge(edge("stmt", "callee", EdgeKind::DataDep))
            .await
            .expect("stmt->callee DataDep");
        g.upsert_edge(edge("callee", "param_sink", EdgeKind::Calls))
            .await
            .expect("callee->param_sink Calls");

        let spec = TaintSpec {
            sources: vec!["stmt".to_string()],
            sinks: vec!["param_sink".to_string()],
            max_depth: 5,
            max_findings: 100,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert_eq!(report.findings.len(), 1, "must find the two-hop path");
        let f = &report.findings[0];
        assert_eq!(f.source.fqn, "stmt");
        assert_eq!(f.sink.fqn, "param_sink");
        assert_eq!(f.path.len(), 2, "stmt->callee->param_sink = 2 edges");
    }

    /// max_findings cap sets truncated=true.
    #[tokio::test]
    async fn taint_caps_set_truncated() {
        let (g, _dir) = make_graph().await;
        // One source, three sinks — cap at 2 findings.
        let src = "tainted_src";
        g.upsert_node(node(src)).await.expect("upsert src");
        for i in 0..3 {
            let s = format!("sink_{i}");
            g.upsert_node(node(&s)).await.expect("upsert sink");
            g.upsert_edge(edge(src, &s, EdgeKind::DataDep))
                .await
                .expect("src->sink");
        }

        let spec = TaintSpec {
            sources: vec![src.to_string()],
            sinks: vec!["sink_".to_string()], // substring match all three
            max_depth: 5,
            max_findings: 2,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert!(report.truncated, "truncated must be true when cap hit");
        assert_eq!(report.findings.len(), 2, "exactly max_findings findings recorded");
    }

    /// Unknown patterns → empty report, source_matches=0, exit Ok.
    #[tokio::test]
    async fn taint_unknown_patterns_empty_report() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("real_node")).await.expect("upsert real_node");

        let spec = TaintSpec {
            sources: vec!["zzz_nothing".to_string()],
            sinks: vec!["zzz_other".to_string()],
            max_depth: 5,
            max_findings: 100,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert!(report.findings.is_empty(), "no findings for unknown patterns");
        assert_eq!(report.source_matches, 0);
        assert_eq!(report.sink_matches, 0);
        assert!(!report.truncated);
    }

    /// Cyclic DataDep edges must not cause infinite BFS.
    #[tokio::test]
    async fn taint_cyclic_data_dep_terminates() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("cyc_a")).await.expect("upsert cyc_a");
        g.upsert_node(node("cyc_b")).await.expect("upsert cyc_b");
        // A -> B -> A cycle
        g.upsert_edge(edge("cyc_a", "cyc_b", EdgeKind::DataDep))
            .await
            .expect("cyc_a->cyc_b");
        g.upsert_edge(edge("cyc_b", "cyc_a", EdgeKind::DataDep))
            .await
            .expect("cyc_b->cyc_a");

        let spec = TaintSpec {
            sources: vec!["cyc_a".to_string()],
            sinks: vec!["cyc_b".to_string()],
            max_depth: 20,
            max_findings: 100,
        };
        // Must complete without hanging.
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        // cyc_b is directly reachable via DataDep — one finding.
        assert_eq!(report.findings.len(), 1, "cycle terminates; one direct finding");
    }

    /// A clean variable that only reaches an unrelated node via DataDep
    /// produces no finding for an unrelated sink.
    #[tokio::test]
    async fn taint_clean_variable_no_finding() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("clean_src")).await.expect("upsert clean_src");
        g.upsert_node(node("unrelated")).await.expect("upsert unrelated");
        g.upsert_node(node("real_sink")).await.expect("upsert real_sink");
        // clean_src -> unrelated (DataDep) but NOT to real_sink
        g.upsert_edge(edge("clean_src", "unrelated", EdgeKind::DataDep))
            .await
            .expect("clean_src->unrelated");

        let spec = TaintSpec {
            sources: vec!["clean_src".to_string()],
            sinks: vec!["real_sink".to_string()],
            max_depth: 5,
            max_findings: 100,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert!(
            report.findings.is_empty(),
            "clean_src never reaches real_sink — no finding"
        );
    }

    /// A chain longer than max_depth yields truncated=true.
    #[tokio::test]
    async fn taint_depth_cap_truncated() {
        let (g, _dir) = make_graph().await;
        // Chain: s -> a -> b -> c -> sink (4 hops); cap at depth 2.
        for name in &["ts", "ta", "tb", "tc", "tsink"] {
            g.upsert_node(node(name)).await.expect("upsert");
        }
        g.upsert_edge(edge("ts", "ta", EdgeKind::DataDep)).await.expect("ts->ta");
        g.upsert_edge(edge("ta", "tb", EdgeKind::DataDep)).await.expect("ta->tb");
        g.upsert_edge(edge("tb", "tc", EdgeKind::DataDep)).await.expect("tb->tc");
        g.upsert_edge(edge("tc", "tsink", EdgeKind::DataDep)).await.expect("tc->tsink");

        // max_depth=2 means we only follow 2 hops from ts (reaches tb at most).
        // tsink is 4 hops away — should NOT appear.
        let spec = TaintSpec {
            sources: vec!["ts".to_string()],
            sinks: vec!["tsink".to_string()],
            max_depth: 2,
            max_findings: 100,
        };
        let report = g.taint_paths(&spec).await.expect("taint_paths");
        assert!(
            report.findings.is_empty(),
            "sink not reachable within max_depth=2"
        );
        // Not truncated — we just couldn't reach the sink within the depth bound.
        assert!(!report.truncated);
    }
}
