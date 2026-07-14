use std::collections::{BTreeMap, HashMap, HashSet};

use hoangsa_memory_core::Result;

use crate::{Edge, EdgeKind, Graph};

/// A detected community of closely-related symbols.
pub struct Community {
    /// Opaque identifier (label index in sorted-FQN order).
    pub id: u32,
    /// Longest common `::` prefix of members, or smallest member FQN.
    pub name: String,
    /// Member FQNs, sorted.
    pub members: Vec<String>,
}

/// An entry-point together with the call chain it leads into.
pub struct ProcessFlow {
    /// FQN of the entry-point node.
    pub entry: String,
    /// Edges visited in DFS order (deterministic: sorted-FQN expansion).
    pub chain: Vec<Edge>,
}

impl Graph {
    /// Detect communities via label propagation (deterministic).
    ///
    /// Builds an undirected view over `Calls`, `Imports`, and `Extends`
    /// edges. Propagates labels in sorted-FQN order for up to 20 passes or
    /// until stable. Groups with fewer than `min_size` members are dropped.
    pub async fn communities(&self, min_size: usize) -> Result<Vec<Community>> {
        // Collect all nodes in sorted order.
        let mut all_fqns: Vec<String> = self.kv.all_node_ids().await?;
        all_fqns.sort();

        if all_fqns.is_empty() {
            return Ok(Vec::new());
        }

        // Index: fqn -> index in sorted order.
        let idx: HashMap<String, usize> = all_fqns
            .iter()
            .enumerate()
            .map(|(i, f)| (f.clone(), i))
            .collect();

        // Build undirected adjacency over calls+imports+extends.
        let community_kinds: &[EdgeKind] = &[EdgeKind::Calls, EdgeKind::Imports, EdgeKind::Extends];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); all_fqns.len()];

        for fqn in &all_fqns {
            for e in self.outgoing(fqn).await? {
                if community_kinds.contains(&e.kind)
                    && let (Some(&si), Some(&di)) = (idx.get(&e.from), idx.get(&e.to))
                    && si != di
                {
                    adj[si].push(di);
                    adj[di].push(si);
                }
            }
        }

        // Deduplicate adjacency lists.
        for neighbors in &mut adj {
            neighbors.sort_unstable();
            neighbors.dedup();
        }

        // Label propagation: init each node's label = its own index.
        let mut labels: Vec<usize> = (0..all_fqns.len()).collect();
        const MAX_PASSES: usize = 20;

        for _ in 0..MAX_PASSES {
            let mut changed = false;
            // Process nodes in sorted-FQN order (deterministic).
            for i in 0..all_fqns.len() {
                let neighbors = &adj[i];
                if neighbors.is_empty() {
                    continue;
                }
                // Count frequency of each neighbor label.
                let mut freq: BTreeMap<usize, usize> = BTreeMap::new();
                for &nb in neighbors {
                    *freq.entry(labels[nb]).or_insert(0) += 1;
                }
                // Find max frequency; ties broken by smallest label (BTreeMap
                // iterates in ascending key order, so we get smallest label
                // among equally frequent ones automatically).
                let best_label = freq
                    .iter()
                    .max_by_key(|&(label, &count)| (count, usize::MAX - label))
                    .map(|(&label, _)| label)
                    .unwrap_or(labels[i]);

                if best_label != labels[i] {
                    labels[i] = best_label;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }

        // Group by final label.
        let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
        for (i, label) in labels.iter().enumerate() {
            groups
                .entry(*label)
                .or_default()
                .push(all_fqns[i].clone());
        }

        // Build Community structs, dropping small groups.
        let mut communities: Vec<Community> = groups
            .into_iter()
            .filter(|(_, members)| members.len() >= min_size)
            .map(|(label, mut members)| {
                members.sort();
                let name = common_prefix_name(&members);
                Community {
                    id: label as u32,
                    name,
                    members,
                }
            })
            .collect();

        // Sort by size desc, then name.
        communities.sort_by(|a, b| {
            b.members
                .len()
                .cmp(&a.members.len())
                .then_with(|| a.name.cmp(&b.name))
        });

        Ok(communities)
    }

    /// Trace process flows from entry points.
    ///
    /// Entry points are nodes whose FQN ends with `::main` or that match any
    /// glob in `entry_globs` (supports `*` prefix/suffix matching only).
    /// From each entry, walks `Calls` out-edges depth-first up to `max_depth`,
    /// cycle-safe, expanding neighbors in sorted-FQN order for determinism.
    pub async fn processes(
        &self,
        max_depth: usize,
        entry_globs: &[String],
    ) -> Result<Vec<ProcessFlow>> {
        let mut all_fqns: Vec<String> = self.kv.all_node_ids().await?;
        all_fqns.sort();

        if all_fqns.is_empty() {
            return Ok(Vec::new());
        }

        // Find entry points.
        let entries: Vec<String> = all_fqns
            .iter()
            .filter(|fqn| is_entry_point(fqn, entry_globs))
            .cloned()
            .collect();

        let mut flows: Vec<ProcessFlow> = Vec::new();

        for entry in entries {
            let mut chain: Vec<Edge> = Vec::new();
            let mut visited: HashSet<String> = HashSet::new();
            visited.insert(entry.clone());

            dfs_calls(self, &entry, 0, max_depth, &mut visited, &mut chain).await?;

            flows.push(ProcessFlow {
                entry,
                chain,
            });
        }

        Ok(flows)
    }
}

/// DFS over Calls edges, expanding neighbors in sorted-FQN order.
async fn dfs_calls(
    g: &Graph,
    cur: &str,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<String>,
    chain: &mut Vec<Edge>,
) -> Result<()> {
    if depth >= max_depth {
        return Ok(());
    }

    // Collect outgoing Calls edges and sort by destination FQN for determinism.
    let mut next: Vec<Edge> = g
        .outgoing(cur)
        .await?
        .into_iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .collect();
    next.sort_by(|a, b| a.to.cmp(&b.to));

    for e in next {
        if visited.insert(e.to.clone()) {
            let to = e.to.clone();
            chain.push(e);
            // Recurse only if the destination node actually exists in the graph.
            if g.get(&to).await?.is_some() {
                Box::pin(dfs_calls(g, &to, depth + 1, max_depth, visited, chain)).await?;
            }
        }
    }

    Ok(())
}

fn is_entry_point(fqn: &str, globs: &[String]) -> bool {
    if fqn.ends_with("::main") || fqn == "main" {
        return true;
    }
    for glob in globs {
        if glob_matches(glob, fqn) {
            return true;
        }
    }
    false
}

/// Simple glob matching: supports `*` at the start and/or end only.
fn glob_matches(glob: &str, s: &str) -> bool {
    match (glob.starts_with('*'), glob.ends_with('*')) {
        (true, true) => {
            let inner = &glob[1..glob.len() - 1];
            s.contains(inner)
        }
        (true, false) => s.ends_with(&glob[1..]),
        (false, true) => s.starts_with(&glob[..glob.len() - 1]),
        (false, false) => s == glob,
    }
}

/// Longest common `::` prefix of a sorted list of FQNs, or smallest member.
fn common_prefix_name(members: &[String]) -> String {
    if members.is_empty() {
        return String::new();
    }
    if members.len() == 1 {
        return members[0].clone();
    }

    let first_segments: Vec<&str> = members[0].split("::").collect();
    let last_segments: Vec<&str> = members[members.len() - 1].split("::").collect();

    let common_len = first_segments
        .iter()
        .zip(last_segments.iter())
        .take_while(|(a, b)| a == b)
        .count();

    if common_len == 0 {
        // No common prefix — return smallest member FQN.
        members[0].clone()
    } else {
        first_segments[..common_len].join("::")
    }
}

#[cfg(test)]
mod tests {
    use crate::{Edge, EdgeKind, Graph, Node};
    use hoangsa_memory_store::KvStore;
    use std::path::PathBuf;

    async fn make_graph() -> (Graph, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let kv = KvStore::open(&dir.path().join("test.db"))
            .await
            .expect("KvStore::open");
        (Graph::new(kv), dir)
    }

    fn node(fqn: &str) -> Node {
        Node {
            fqn: fqn.to_string(),
            kind: "function".to_string(),
            path: PathBuf::from("src/lib.rs"),
            line: 1,
        }
    }

    fn edge_of(from: &str, to: &str, kind: EdgeKind) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            kind,
        }
    }

    fn calls(from: &str, to: &str) -> Edge {
        edge_of(from, to, EdgeKind::Calls)
    }

    /// Two dense clusters (a::f1 <-> a::f2 <-> a::f3) and (b::g1 <-> b::g2
    /// <-> b::g3) that are disconnected from the perspective of community
    /// detection (calls/imports/extends only). A References bridge between
    /// the two is excluded from the undirected view, so LPA converges to
    /// 2 communities named by their common `::` prefix.
    #[tokio::test]
    async fn test_communities_two_clusters() {
        let (g, _dir) = make_graph().await;

        // Cluster A — triangle so each node has 2 intra-cluster neighbors.
        for fqn in &["a::f1", "a::f2", "a::f3"] {
            g.upsert_node(node(fqn)).await.expect("upsert");
        }
        g.upsert_edge(calls("a::f1", "a::f2")).await.expect("edge");
        g.upsert_edge(calls("a::f2", "a::f3")).await.expect("edge");
        g.upsert_edge(calls("a::f3", "a::f1")).await.expect("edge");

        // Cluster B — triangle.
        for fqn in &["b::g1", "b::g2", "b::g3"] {
            g.upsert_node(node(fqn)).await.expect("upsert");
        }
        g.upsert_edge(calls("b::g1", "b::g2")).await.expect("edge");
        g.upsert_edge(calls("b::g2", "b::g3")).await.expect("edge");
        g.upsert_edge(calls("b::g3", "b::g1")).await.expect("edge");

        // Bridge via References — excluded from community detection edge kinds.
        g.upsert_edge(edge_of("a::f3", "b::g1", EdgeKind::References))
            .await
            .expect("bridge");

        let communities = g.communities(2).await.expect("communities");
        assert_eq!(communities.len(), 2, "expected 2 communities");

        // Communities sorted by size desc then name; both have 3 members.
        // Names should be "a" and "b" (longest common :: prefix).
        let names: Vec<&str> = communities.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"a"), "cluster a named by prefix");
        assert!(names.contains(&"b"), "cluster b named by prefix");
    }

    #[tokio::test]
    async fn test_communities_empty_graph() {
        let (g, _dir) = make_graph().await;
        let result = g.communities(1).await.expect("communities");
        assert!(result.is_empty(), "empty graph => empty vec");
    }

    #[tokio::test]
    async fn test_communities_min_size_filters() {
        let (g, _dir) = make_graph().await;
        // Two isolated nodes — each is its own community of size 1.
        g.upsert_node(node("x::a")).await.expect("upsert");
        g.upsert_node(node("x::b")).await.expect("upsert");
        // min_size=2 should drop both.
        let result = g.communities(2).await.expect("communities");
        assert!(result.is_empty(), "isolated nodes filtered by min_size=2");
    }

    #[tokio::test]
    async fn test_processes_entry_main_chain() {
        let (g, _dir) = make_graph().await;

        // crate::main -> crate::step1 -> crate::step2
        g.upsert_node(node("crate::main")).await.expect("upsert");
        g.upsert_node(node("crate::step1")).await.expect("upsert");
        g.upsert_node(node("crate::step2")).await.expect("upsert");
        g.upsert_edge(calls("crate::main", "crate::step1"))
            .await
            .expect("edge");
        g.upsert_edge(calls("crate::step1", "crate::step2"))
            .await
            .expect("edge");

        let flows = g.processes(10, &[]).await.expect("processes");
        assert_eq!(flows.len(), 1, "one entry point");
        assert_eq!(flows[0].entry, "crate::main");
        assert_eq!(flows[0].chain.len(), 2, "chain of 2 edges");
        assert_eq!(flows[0].chain[0].from, "crate::main");
        assert_eq!(flows[0].chain[0].to, "crate::step1");
        assert_eq!(flows[0].chain[1].from, "crate::step1");
        assert_eq!(flows[0].chain[1].to, "crate::step2");
    }

    #[tokio::test]
    async fn test_processes_cycle_terminates() {
        let (g, _dir) = make_graph().await;

        // crate::main -> crate::a -> crate::b -> crate::a (cycle)
        g.upsert_node(node("crate::main")).await.expect("upsert");
        g.upsert_node(node("crate::a")).await.expect("upsert");
        g.upsert_node(node("crate::b")).await.expect("upsert");
        g.upsert_edge(calls("crate::main", "crate::a"))
            .await
            .expect("edge");
        g.upsert_edge(calls("crate::a", "crate::b"))
            .await
            .expect("edge");
        g.upsert_edge(calls("crate::b", "crate::a"))
            .await
            .expect("edge");

        let flows = g.processes(20, &[]).await.expect("processes cycle");
        assert_eq!(flows.len(), 1, "one entry point");
        // Should terminate — chain has exactly 2 edges (main->a, a->b; b->a
        // is a cycle, a was already visited).
        assert_eq!(
            flows[0].chain.len(),
            2,
            "cycle terminated at 2 edges, got: {:?}",
            flows[0].chain
        );
    }

    #[tokio::test]
    async fn test_processes_empty_graph() {
        let (g, _dir) = make_graph().await;
        let result = g.processes(10, &[]).await.expect("processes");
        assert!(result.is_empty(), "empty graph => empty vec");
    }

    // ---- spec-named tests (T-04) -------------------------------------------

    /// Spec test 5: communities cluster related symbols.
    ///
    /// Two dense clusters a::* and b::* separated only by a References bridge
    /// (excluded from community detection) → 2 communities whose names start with
    /// "a" and "b" respectively. Empty graph → [].
    #[tokio::test]
    async fn communities_cluster_related_symbols() {
        // Two clusters, bridge via References (ignored by LPA).
        {
            let (g, _dir) = make_graph().await;
            for fqn in &["a::f1", "a::f2", "a::f3"] {
                g.upsert_node(node(fqn)).await.expect("upsert a node");
            }
            g.upsert_edge(calls("a::f1", "a::f2")).await.expect("edge");
            g.upsert_edge(calls("a::f2", "a::f3")).await.expect("edge");
            g.upsert_edge(calls("a::f3", "a::f1")).await.expect("edge");

            for fqn in &["b::g1", "b::g2", "b::g3"] {
                g.upsert_node(node(fqn)).await.expect("upsert b node");
            }
            g.upsert_edge(calls("b::g1", "b::g2")).await.expect("edge");
            g.upsert_edge(calls("b::g2", "b::g3")).await.expect("edge");
            g.upsert_edge(calls("b::g3", "b::g1")).await.expect("edge");

            // Bridge via References — must be excluded from LPA.
            g.upsert_edge(edge_of("a::f3", "b::g1", crate::EdgeKind::References))
                .await
                .expect("bridge");

            let communities = g.communities(2).await.expect("communities");
            assert_eq!(communities.len(), 2, "expected exactly 2 communities");

            let names: Vec<&str> = communities.iter().map(|c| c.name.as_str()).collect();
            assert!(
                names.iter().any(|n| n.starts_with('a')),
                "one community should have an 'a' prefix name; got {names:?}"
            );
            assert!(
                names.iter().any(|n| n.starts_with('b')),
                "one community should have a 'b' prefix name; got {names:?}"
            );
        }

        // Empty graph → [].
        {
            let (g, _dir) = make_graph().await;
            let result = g.communities(1).await.expect("communities on empty graph");
            assert!(result.is_empty(), "empty graph must return []");
        }
    }

    /// Spec test 6: process tracing walks from entry point.
    ///
    /// crate::main -> run -> helper → one flow, ordered 2-edge chain.
    #[tokio::test]
    async fn process_tracing_walks_from_entry() {
        let (g, _dir) = make_graph().await;

        g.upsert_node(node("crate::main")).await.expect("upsert main");
        g.upsert_node(node("crate::run")).await.expect("upsert run");
        g.upsert_node(node("crate::helper")).await.expect("upsert helper");
        g.upsert_edge(calls("crate::main", "crate::run")).await.expect("main->run");
        g.upsert_edge(calls("crate::run", "crate::helper")).await.expect("run->helper");

        let flows = g.processes(10, &[]).await.expect("processes");
        assert_eq!(flows.len(), 1, "exactly one flow from crate::main");
        assert_eq!(flows[0].entry, "crate::main");
        assert_eq!(flows[0].chain.len(), 2, "ordered 2-edge chain");
        // Edge 0: main -> run
        assert_eq!(flows[0].chain[0].from, "crate::main");
        assert_eq!(flows[0].chain[0].to, "crate::run");
        // Edge 1: run -> helper
        assert_eq!(flows[0].chain[1].from, "crate::run");
        assert_eq!(flows[0].chain[1].to, "crate::helper");
    }
}
