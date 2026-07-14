//! # hoangsa-memory-graph
//!
//! Symbol, call, import, and reference graph built on top of
//! [`hoangsa_memory_store::KvStore`]. This is the spine of Mode::Zero retrieval: it
//! answers "who calls X", "what does X call", "which modules import Y"
//! without any LLM or embedding.
//!
//! Design:
//!
//! - Every parsed symbol becomes a [`Node`] keyed by its fully qualified
//!   name (FQN). Nodes carry the path + line of their declaration.
//! - Every call, import, extends, references relationship becomes an
//!   [`Edge`]. Edges are stored with the underlying KV as
//!   `"<src>|<kind>|<dst>"`, so outgoing-edge lookups are a prefix scan.
//! - Traversal is plain BFS bounded by `depth`; fine at indexing scale.
//!
//! See `DESIGN.md` §4 and §5.

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

/// Graph analytics: community detection and process tracing.
pub mod analytics;
pub use analytics::{Community, ProcessFlow};

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use hoangsa_memory_core::Result;
use hoangsa_memory_store::{BfsDir, EdgeRow, KvStore, NodeRow};
use serde::{Deserialize, Serialize};

/// A node in the code graph.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Node {
    /// Fully qualified name (primary key).
    pub fqn: String,
    /// Coarse kind (`"function"`, `"type"`, `"trait"`, `"module"`,
    /// `"binding"`).
    pub kind: String,
    /// Source path.
    pub path: PathBuf,
    /// 1-based declaration line.
    pub line: u32,
}

/// Edge kinds tracked by the graph.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    /// `A` calls `B`.
    Calls,
    /// `A` imports module `B`.
    Imports,
    /// `A` references symbol `B`.
    References,
    /// `A` extends / implements `B`.
    Extends,
    /// `A` is declared in module `B`.
    DeclaredIn,
    /// `A` emits / publishes event `B`. `B` is a synthetic event FQN
    /// of the form `event::<bus>::<topic>` (bus is `*` when receiver
    /// can't be statically named).
    Emits,
    /// `A` subscribes to / listens for event `B`. Same `event::*::*`
    /// FQN convention as `Emits`; the direction is event → handler so
    /// that `subscribers_of(event_fqn)` is a plain incoming-edge scan.
    Subscribes,
}

impl EdgeKind {
    /// Canonical on-disk tag.
    pub fn tag(self) -> &'static str {
        match self {
            EdgeKind::Calls => "calls",
            EdgeKind::Imports => "imports",
            EdgeKind::References => "references",
            EdgeKind::Extends => "extends",
            EdgeKind::DeclaredIn => "declared_in",
            EdgeKind::Emits => "emits",
            EdgeKind::Subscribes => "subscribes",
        }
    }

    /// Parse a tag back into an [`EdgeKind`].
    pub fn from_tag(tag: &str) -> Option<Self> {
        Some(match tag {
            "calls" => EdgeKind::Calls,
            "imports" => EdgeKind::Imports,
            "references" => EdgeKind::References,
            "extends" => EdgeKind::Extends,
            "declared_in" => EdgeKind::DeclaredIn,
            "emits" => EdgeKind::Emits,
            "subscribes" => EdgeKind::Subscribes,
            _ => return None,
        })
    }
}

/// An edge between two nodes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Edge {
    /// Source FQN.
    pub from: String,
    /// Destination FQN.
    pub to: String,
    /// Edge kind.
    pub kind: EdgeKind,
}

/// Graph handle — cheap to clone (wraps a shared [`KvStore`]).
#[derive(Clone)]
pub struct Graph {
    kv: KvStore,
}

impl Graph {
    /// Wrap an existing KV store.
    pub fn new(kv: KvStore) -> Self {
        Self { kv }
    }

    /// Insert or update a node.
    pub async fn upsert_node(&self, n: Node) -> Result<()> {
        let payload = serde_json::json!({
            "path": n.path,
            "line": n.line,
        });
        self.kv
            .put_node(NodeRow {
                id: n.fqn,
                kind: n.kind,
                payload,
            })
            .await
    }

    /// Insert or update an edge.
    pub async fn upsert_edge(&self, e: Edge) -> Result<()> {
        self.kv
            .put_edge(EdgeRow {
                src: e.from,
                dst: e.to,
                kind: e.kind.tag().to_string(),
                payload: serde_json::Value::Null,
            })
            .await
    }

    /// Insert or update many nodes in a single transaction.
    pub async fn upsert_nodes_batch(&self, nodes: Vec<Node>) -> Result<()> {
        let rows = nodes
            .into_iter()
            .map(|n| {
                let payload = serde_json::json!({ "path": n.path, "line": n.line });
                NodeRow {
                    id: n.fqn,
                    kind: n.kind,
                    payload,
                }
            })
            .collect();
        self.kv.put_nodes_batch(rows).await
    }

    /// Insert or update many edges in a single transaction.
    pub async fn upsert_edges_batch(&self, edges: Vec<Edge>) -> Result<()> {
        let rows = edges
            .into_iter()
            .map(|e| EdgeRow {
                src: e.from,
                dst: e.to,
                kind: e.kind.tag().to_string(),
                payload: serde_json::Value::Null,
            })
            .collect();
        self.kv.put_edges_batch(rows).await
    }

    /// Fetch a node by FQN.
    pub async fn get(&self, fqn: &str) -> Result<Option<Node>> {
        Ok(self.kv.get_node(fqn).await?.map(row_to_node))
    }

    /// Best-effort lookup by FQN with a suffix-match fallback.
    ///
    /// Tries `get(fqn)` first. On miss, scans the node table for any
    /// node whose FQN ends with `fqn` on a `::` boundary, then:
    ///
    /// - exactly one hit → returns it (canonical form in [`Node::fqn`]);
    /// - zero hits → `Ok(None)` — caller should surface the original FQN
    ///   in the error so the user can see what they asked for;
    /// - multiple hits → `Ok(None)` plus the list via
    ///   [`Self::find_suffix_candidates`] so the caller can show an
    ///   ambiguity message instead of picking arbitrarily.
    ///
    /// Used by `impact` / `symbol_context` to soften the pain of a user
    /// typing `cli::cmd::rule::foo` when the graph key is `rule::foo`.
    pub async fn resolve_fqn(&self, fqn: &str) -> Result<Option<Node>> {
        if let Some(n) = self.get(fqn).await? {
            return Ok(Some(n));
        }
        let candidates = self.find_suffix_candidates(fqn).await?;
        if candidates.len() == 1 {
            return Ok(Some(candidates.into_iter().next().unwrap()));
        }
        Ok(None)
    }

    /// Every node whose FQN ends with `needle` on a `::` boundary.
    /// Caller-facing so `impact` / `symbol_context` can render the
    /// ambiguity list when the lookup is not unique.
    pub async fn find_suffix_candidates(&self, needle: &str) -> Result<Vec<Node>> {
        Ok(self
            .kv
            .find_nodes_by_suffix(needle)
            .await?
            .into_iter()
            .map(row_to_node)
            .collect())
    }

    /// BFS callees: `fqn` → what `fqn` calls, transitively, up to `depth`.
    pub async fn callees(&self, fqn: &str, depth: usize) -> Result<Vec<Node>> {
        self.bfs(fqn, depth, Direction::Out, Some(&[EdgeKind::Calls]))
            .await
    }

    /// BFS callers: who calls `fqn`, transitively, up to `depth`.
    pub async fn callers(&self, fqn: &str, depth: usize) -> Result<Vec<Node>> {
        self.bfs(fqn, depth, Direction::In, Some(&[EdgeKind::Calls]))
            .await
    }

    /// BFS over every edge kind in both directions — useful for "related
    /// code" fan-outs in retrieval.
    pub async fn neighbors(&self, fqn: &str, depth: usize) -> Result<Vec<Node>> {
        self.bfs(fqn, depth, Direction::Both, None).await
    }

    /// Blast-radius / impact analysis: BFS from `fqn` grouped by distance.
    ///
    /// - [`BlastDir::Up`]: incoming `Calls`, `References`, and `Extends` —
    ///   "what breaks if I change `fqn`?" (callers, referrers, subtypes).
    /// - [`BlastDir::Down`]: outgoing `Calls` and `Extends` — "what does
    ///   `fqn` depend on?" (transitive callees and parent types).
    /// - [`BlastDir::Both`]: union of the two.
    ///
    /// Returns `(node, depth)` pairs in BFS order so callers can group by
    /// depth without re-running the traversal.
    pub async fn impact(
        &self,
        fqn: &str,
        dir: BlastDir,
        depth: usize,
    ) -> Result<Vec<(Node, usize)>> {
        let (direction, kinds) = match dir {
            BlastDir::Up => (
                Direction::In,
                [EdgeKind::Calls, EdgeKind::References, EdgeKind::Extends],
            ),
            BlastDir::Down => (
                Direction::Out,
                // Second slot doubles `Calls` to pad the fixed-size array;
                // `bfs_depth_tagged` dedupes edge-kind matches, so repeats
                // are harmless.
                [EdgeKind::Calls, EdgeKind::Calls, EdgeKind::Extends],
            ),
            BlastDir::Both => (
                Direction::Both,
                [EdgeKind::Calls, EdgeKind::References, EdgeKind::Extends],
            ),
        };
        let mut hits = self
            .bfs_depth_tagged(fqn, depth, direction, Some(&kinds))
            .await?;

        // Secondary walks from progressively-less-qualified suffixes.
        // Call sites whose edges couldn't be resolved through the file's
        // alias map at index time are stored with a shorter `to` than
        // the canonical FQN — a BFS rooted only at the full FQN misses
        // them. We add two fallback passes, each guarded against
        // polysemy so a noisy leaf can't blend callers of unrelated
        // same-named symbols.
        //
        // 1. **2-segment suffix** (e.g. `chroma::ChromaStore::open` →
        //    `ChromaStore::open`). This catches methods on external
        //    types that the indexer couldn't nest under the type's
        //    actual module because it lives in another crate — the
        //    edge target stays `ChromaStore::open` (2 segments) and a
        //    strict walk from `chroma::ChromaStore::open` would miss it.
        //
        // 2. **Bare leaf** (e.g. `cmd::hook::cmd_enforce` →
        //    `cmd_enforce`). Covers `cmd::hook::cmd_enforce(&cwd)`
        //    dispatched from a match arm in `main.rs` — the indexer
        //    couldn't resolve the head, so the edge target collapsed to
        //    the leaf.
        //
        // Polysemy guard: before following a suffix, count nodes in the
        // graph whose FQN ends with that suffix on a `::` boundary;
        // skip the walk if more than one distinct owner exists (other
        // than `fqn` itself), otherwise unrelated types with a same-
        // named method would pollute the answer.
        if matches!(dir, BlastDir::Up | BlastDir::Both) {
            let mut seen: std::collections::HashSet<String> =
                hits.iter().map(|(n, _)| n.fqn.clone()).collect();
            // Never report `fqn` as its own caller.
            seen.insert(fqn.to_string());

            let segments: Vec<&str> = fqn.split("::").filter(|s| !s.is_empty()).collect();
            let total = segments.len();
            // Try progressively shorter suffixes; max 2 extra walks
            // (the 2-segment and the 1-segment leaf).
            for take in [2usize, 1usize] {
                if take >= total {
                    // Same as or longer than the full FQN — strict BFS
                    // already covered it.
                    continue;
                }
                let suffix = segments[total - take..].join("::");
                if suffix.is_empty() {
                    continue;
                }
                let defs = self.kv.find_nodes_by_suffix(&suffix).await?;
                let distinct_owners: std::collections::HashSet<String> = defs
                    .iter()
                    .map(|row| row.id.clone())
                    .filter(|f| f != fqn)
                    .collect();
                if !distinct_owners.is_empty() {
                    // Polysemous — another type owns the same suffix.
                    // Over-reporting here drowns the real signal.
                    continue;
                }
                let extra = self
                    .bfs_depth_tagged(&suffix, depth, direction, Some(&kinds))
                    .await?;
                for (n, d) in extra {
                    if seen.insert(n.fqn.clone()) {
                        hits.push((n, d));
                    }
                }
            }
        }

        Ok(hits)
    }

    /// Delete every node and every edge that touches any symbol declared in
    /// `path`. Returns `(nodes_dropped, edges_dropped)`.
    ///
    /// Called by [`hoangsa_memory_retrieve::Indexer::purge_path`] when a file is
    /// deleted or about to be re-indexed; keeps the graph in lock-step with
    /// the source tree.
    pub async fn purge_path(&self, path: impl AsRef<std::path::Path>) -> Result<(usize, usize)> {
        let nodes = self.kv.delete_nodes_by_path(path).await?;
        let edges = self.kv.delete_edges_touching(&nodes).await?;
        Ok((nodes.len(), edges))
    }

    /// Every node declared inside `path`. Symmetric with
    /// [`Self::purge_path`] — together they form the read/write surface
    /// for file-level graph lookups.
    pub async fn symbols_in_file(&self, path: impl AsRef<std::path::Path>) -> Result<Vec<Node>> {
        Ok(self
            .kv
            .nodes_for_path(path)
            .await?
            .into_iter()
            .map(row_to_node)
            .collect())
    }

    /// Like [`Self::symbols_in_file`] but tolerates absolute /
    /// cwd-relative / `./`-prefixed path variants. The symbols table
    /// stores whatever path form the indexer was invoked with, and the
    /// caller (e.g. `detect_changes`) often has a different flavour in
    /// hand. Delegates to [`hoangsa_memory_store::KvStore::nodes_for_path_like`].
    pub async fn symbols_in_file_like(
        &self,
        path: impl AsRef<std::path::Path>,
    ) -> Result<Vec<Node>> {
        Ok(self
            .kv
            .nodes_for_path_like(path)
            .await?
            .into_iter()
            .map(row_to_node)
            .collect())
    }

    /// Distinct FQNs this file imports. Walks outgoing `Imports` edges
    /// for every symbol declared in `path`, plus the file's synthetic
    /// "module" node (file stem) which the indexer uses as the source of
    /// file-level `use`/`import` statements. Destinations are deduped;
    /// order is stable (insertion order of first occurrence).
    pub async fn imports_of_file(&self, path: impl AsRef<std::path::Path>) -> Result<Vec<String>> {
        let path = path.as_ref();
        let nodes = self.symbols_in_file(path).await?;
        let mut seen: HashSet<String> = HashSet::new();
        let mut out = Vec::new();

        // Per-symbol imports (rare — most languages attach imports at
        // file scope — but cheap to check).
        for n in &nodes {
            for e in self.outgoing(&n.fqn).await? {
                if matches!(e.kind, EdgeKind::Imports) && seen.insert(e.to.clone()) {
                    out.push(e.to);
                }
            }
        }

        // File-level imports: the indexer writes these with the file's
        // crate-qualified module path (`crate_name::mod_path`) as the
        // `from` of an `Imports` edge. That FQN has no corresponding Node,
        // so a node-driven scan alone would miss them. Using the bare
        // `file_stem()` here was the old scheme and caused import lists
        // from unrelated crates' `main.rs` files to merge; we now resolve
        // the same way the indexer keys its writes.
        let module = hoangsa_memory_parse::crate_qualified_module_path(path);
        if !module.is_empty() {
            for e in self.outgoing(&module).await? {
                if matches!(e.kind, EdgeKind::Imports) && seen.insert(e.to.clone()) {
                    out.push(e.to);
                }
            }
        }

        Ok(out)
    }

    /// Direct outgoing neighbours filtered to a single edge kind.
    ///
    /// Unlike [`Self::callees`] / [`Self::callers`] this is depth=1 and
    /// returns [`Node`]s (not just FQNs) so callers can render a path/line
    /// for every neighbour without a second round-trip. Missing nodes
    /// (edges pointing at unresolved names — common for third-party
    /// callees the indexer couldn't map) are silently dropped.
    pub async fn out_neighbors(&self, fqn: &str, kind: EdgeKind) -> Result<Vec<Node>> {
        let mut out = Vec::new();
        for e in self.outgoing(fqn).await? {
            if e.kind == kind
                && let Some(n) = self.get(&e.to).await?
            {
                out.push(n);
            }
        }
        Ok(out)
    }

    /// Direct incoming neighbours filtered to a single edge kind. Mirror of
    /// [`Self::out_neighbors`].
    pub async fn in_neighbors(&self, fqn: &str, kind: EdgeKind) -> Result<Vec<Node>> {
        let mut out = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for e in self.incoming(fqn).await? {
            if e.kind == kind
                && seen.insert(e.from.clone())
                && let Some(n) = self.get(&e.from).await?
            {
                out.push(n);
            }
        }
        // Also include edges whose `dst` is a shorter suffix of `fqn` —
        // 2-segment (`ChromaStore::open`) and then bare leaf
        // (`cmd_enforce`). These are cross-file callers whose call
        // text didn't resolve through the file-local alias map at
        // index time; see [`Self::impact`] for the full rationale and
        // polysemy guard.
        let segments: Vec<&str> = fqn.split("::").filter(|s| !s.is_empty()).collect();
        let total = segments.len();
        for take in [2usize, 1usize] {
            if take >= total {
                continue;
            }
            let suffix = segments[total - take..].join("::");
            if suffix.is_empty() {
                continue;
            }
            let defs = self.kv.find_nodes_by_suffix(&suffix).await?;
            let distinct_owners: std::collections::HashSet<String> = defs
                .iter()
                .map(|row| row.id.clone())
                .filter(|f| f != fqn)
                .collect();
            if !distinct_owners.is_empty() {
                continue;
            }
            for e in self.incoming(&suffix).await? {
                if e.kind == kind
                    && e.from != fqn
                    && seen.insert(e.from.clone())
                    && let Some(n) = self.get(&e.from).await?
                {
                    out.push(n);
                }
            }
        }
        Ok(out)
    }

    /// Unresolved destinations — i.e. `to` values of outgoing edges whose
    /// kind matches but that have no corresponding [`Node`] (external
    /// references, imports pointing at third-party modules, etc.). Useful
    /// for the symbol-context tool to report "imports: serde::Deserialize"
    /// even when `serde::Deserialize` isn't in the graph.
    pub async fn out_unresolved(&self, fqn: &str, kind: EdgeKind) -> Result<Vec<String>> {
        let mut out = Vec::new();
        for e in self.outgoing(fqn).await? {
            if e.kind == kind && self.get(&e.to).await?.is_none() {
                out.push(e.to);
            }
        }
        Ok(out)
    }

    /// Direct outgoing edges of any kind.
    pub async fn outgoing(&self, fqn: &str) -> Result<Vec<Edge>> {
        Ok(self
            .kv
            .edges_from(fqn)
            .await?
            .into_iter()
            .filter_map(row_to_edge)
            .collect())
    }

    /// Direct incoming edges of any kind.
    pub async fn incoming(&self, fqn: &str) -> Result<Vec<Edge>> {
        Ok(self
            .kv
            .edges_to(fqn)
            .await?
            .into_iter()
            .filter_map(row_to_edge)
            .collect())
    }

    /// BFS traversal over the graph starting from multiple FQNs.
    ///
    /// Each start FQN is resolved via [`Self::resolve_fqn`] (exact then suffix).
    /// Unresolved starts are recorded in [`Subgraph::unresolved`] and do not
    /// cause an error. The BFS is bounded by `spec.max_depth` and
    /// `spec.max_nodes`; when the node cap is hit `Subgraph::truncated` is set.
    /// Output order is deterministic: BFS layers are expanded in sorted-FQN
    /// order; output nodes sorted by `(depth, fqn)`, edges by `(from, kind, to)`.
    pub async fn traverse(&self, spec: &TraverseSpec) -> Result<Subgraph> {
        // --- resolve start nodes ---
        let mut resolved_starts: Vec<(String, usize)> = Vec::new(); // (fqn, depth=0)
        let mut unresolved: Vec<String> = Vec::new();
        for s in &spec.start {
            match self.resolve_fqn(s).await? {
                Some(n) => resolved_starts.push((n.fqn.clone(), 0)),
                None => unresolved.push(s.clone()),
            }
        }

        let kinds_filter: Option<Vec<EdgeKind>> = spec.edge_kinds.clone();
        let dir = spec.direction;
        let max_depth = spec.max_depth;
        let max_nodes = spec.max_nodes;

        // --- BFS ---
        // visited tracks FQNs we've already enqueued (includes start nodes).
        let mut visited: BTreeSet<String> = BTreeSet::new();
        // node_depth: fqn -> depth reached
        let mut node_depth: BTreeMap<String, usize> = BTreeMap::new();
        // collected nodes (in discovery order)
        let mut collected_nodes: Vec<(String, Node)> = Vec::new();
        let mut collected_edges: Vec<Edge> = Vec::new();
        let mut truncated = false;

        // Initialise visited / node_depth for start nodes.
        // Deduplicate starts stably.
        let mut deduped_starts: Vec<String> = Vec::new();
        for (fqn, _) in &resolved_starts {
            if visited.insert(fqn.clone()) {
                deduped_starts.push(fqn.clone());
            }
        }
        // Sort starts so expansion order is deterministic.
        deduped_starts.sort();

        // Fetch start nodes and check cap.
        for fqn in &deduped_starts {
            if collected_nodes.len() >= max_nodes {
                truncated = true;
                break;
            }
            if let Some(n) = self.get(fqn).await? {
                node_depth.insert(fqn.clone(), 0);
                collected_nodes.push((fqn.clone(), n));
            }
        }

        // BFS queue: (fqn, depth). We process layer by layer for determinism.
        let mut frontier: VecDeque<(String, usize)> = if !truncated {
            deduped_starts.iter().map(|f| (f.clone(), 0)).collect()
        } else {
            VecDeque::new()
        };

        while let Some((cur, depth)) = frontier.pop_front() {
            if depth >= max_depth {
                continue;
            }

            // Collect edges in both directions according to spec.
            let mut next_fqns: Vec<(String, Edge)> = Vec::new();

            if matches!(dir, Direction::Out | Direction::Both) {
                for e in self.outgoing(&cur).await? {
                    if kind_matches(&e.kind, kinds_filter.as_deref()) {
                        next_fqns.push((e.to.clone(), e));
                    }
                }
            }
            if matches!(dir, Direction::In | Direction::Both) {
                for e in self.incoming(&cur).await? {
                    if kind_matches(&e.kind, kinds_filter.as_deref()) {
                        next_fqns.push((e.from.clone(), e));
                    }
                }
            }

            // Sort for determinism before processing.
            next_fqns.sort_by(|a, b| a.0.cmp(&b.0));

            for (nfqn, edge) in next_fqns {
                // Collect edge if both endpoints are in the visited set or
                // the destination will be added. We collect edges when we
                // enqueue the destination node.
                let new_node = visited.insert(nfqn.clone());
                if new_node {
                    if collected_nodes.len() >= max_nodes {
                        truncated = true;
                        // Don't add node but still record the edge if src is in graph.
                        continue;
                    }
                    if let Some(n) = self.get(&nfqn).await? {
                        node_depth.insert(nfqn.clone(), depth + 1);
                        collected_nodes.push((nfqn.clone(), n));
                        // Record the edge.
                        collected_edges.push(edge);
                        frontier.push_back((nfqn, depth + 1));
                    }
                } else {
                    // Already visited — still record the edge if src node
                    // is in the subgraph.
                    if node_depth.contains_key(&edge.from) || node_depth.contains_key(&edge.to) {
                        let from_in = node_depth.contains_key(&edge.from);
                        let to_in = node_depth.contains_key(&edge.to);
                        if from_in && to_in {
                            // Deduplicate edges.
                            if !collected_edges.contains(&edge) {
                                collected_edges.push(edge);
                            }
                        }
                    }
                }
            }
        }

        // --- build output nodes sorted by (depth, fqn) ---
        let mut node_list: Vec<SubgraphNode> = collected_nodes
            .iter()
            .map(|(fqn, n)| SubgraphNode {
                fqn: fqn.clone(),
                resolved: true,
                depth: node_depth.get(fqn).copied(),
                kind: n.kind.clone(),
                path: n.path.to_string_lossy().into_owned(),
                line: n.line,
            })
            .collect();
        node_list.sort_by(|a, b| {
            a.depth
                .cmp(&b.depth)
                .then_with(|| a.fqn.cmp(&b.fqn))
        });

        // --- sort edges by (from, kind, to) ---
        collected_edges.sort_by(|a, b| {
            a.from
                .cmp(&b.from)
                .then_with(|| a.kind.tag().cmp(b.kind.tag()))
                .then_with(|| a.to.cmp(&b.to))
        });
        // Deduplicate edges after sort.
        collected_edges.dedup();

        Ok(Subgraph {
            nodes: node_list,
            edges: collected_edges,
            truncated,
            unresolved,
        })
    }

    /// BFS shortest path from `from` to `to`, optionally filtering by edge kinds.
    ///
    /// Returns `None` when `to` is unreachable within `max_depth` hops.
    /// The returned `Vec<Edge>` is the sequence of edges on the shortest path
    /// (length = number of hops). An empty vec means `from == to` (already
    /// resolved to the same canonical FQN).
    pub async fn shortest_path(
        &self,
        from: &str,
        to: &str,
        kinds: Option<&[EdgeKind]>,
        dir: Direction,
        max_depth: usize,
    ) -> Result<Option<Vec<Edge>>> {
        let start = match self.resolve_fqn(from).await? {
            Some(n) => n.fqn,
            None => return Ok(None),
        };
        let goal = match self.resolve_fqn(to).await? {
            Some(n) => n.fqn,
            None => return Ok(None),
        };

        if start == goal {
            return Ok(Some(Vec::new()));
        }

        // BFS with parent tracking: fqn -> (parent_fqn, edge_used)
        let mut parent: HashMap<String, (String, Edge)> = HashMap::new();
        let mut visited: HashSet<String> = HashSet::from([start.clone()]);
        let mut frontier: VecDeque<(String, usize)> = VecDeque::from([(start.clone(), 0)]);

        'bfs: while let Some((cur, depth)) = frontier.pop_front() {
            if depth >= max_depth {
                continue;
            }

            let mut next: Vec<(String, Edge)> = Vec::new();

            if matches!(dir, Direction::Out | Direction::Both) {
                for e in self.outgoing(&cur).await? {
                    if kind_matches(&e.kind, kinds) {
                        next.push((e.to.clone(), e));
                    }
                }
            }
            if matches!(dir, Direction::In | Direction::Both) {
                for e in self.incoming(&cur).await? {
                    if kind_matches(&e.kind, kinds) {
                        next.push((e.from.clone(), e));
                    }
                }
            }

            next.sort_by(|a, b| a.0.cmp(&b.0));

            for (nfqn, edge) in next {
                if visited.insert(nfqn.clone()) {
                    parent.insert(nfqn.clone(), (cur.clone(), edge));
                    if nfqn == goal {
                        break 'bfs;
                    }
                    frontier.push_back((nfqn, depth + 1));
                }
            }
        }

        if !parent.contains_key(&goal) {
            return Ok(None);
        }

        // Reconstruct path by walking parent pointers.
        let mut path: Vec<Edge> = Vec::new();
        let mut cur = goal.clone();
        while let Some((prev, edge)) = parent.remove(&cur) {
            path.push(edge);
            cur = prev;
            if cur == start {
                break;
            }
        }
        path.reverse();
        Ok(Some(path))
    }

    // ---- internal --------------------------------------------------------

    async fn bfs(
        &self,
        start: &str,
        depth: usize,
        dir: Direction,
        only: Option<&[EdgeKind]>,
    ) -> Result<Vec<Node>> {
        Ok(self
            .bfs_depth_tagged(start, depth, dir, only)
            .await?
            .into_iter()
            .map(|(n, _)| n)
            .collect())
    }

    /// Core BFS that also records the depth each node was reached at.
    /// `only = None` walks every [`EdgeKind`]; otherwise only edges whose
    /// kind is in the slice are followed. `start` is never returned.
    ///
    /// Delegates to [`KvStore::graph_bfs`] so the full walk lives in one
    /// `spawn_blocking` + one redb read transaction (see the N+1 note in
    /// `hoangsa-memory-store::kv::graph_bfs`).
    async fn bfs_depth_tagged(
        &self,
        start: &str,
        depth: usize,
        dir: Direction,
        only: Option<&[EdgeKind]>,
    ) -> Result<Vec<(Node, usize)>> {
        // Deduplicate kind tags — `Graph::impact` passes a fixed 3-slot
        // array that sometimes repeats `Calls` to pad. `graph_bfs` uses
        // the tag strings directly, so we collect them here.
        let kinds: Option<Vec<String>> = only.map(|ks| {
            let mut seen: HashSet<&'static str> = HashSet::new();
            let mut out = Vec::with_capacity(ks.len());
            for k in ks {
                if seen.insert(k.tag()) {
                    out.push(k.tag().to_string());
                }
            }
            out
        });
        let hits = self
            .kv
            .graph_bfs(start.to_string(), depth, direction_to_bfs_dir(dir), kinds)
            .await?;
        Ok(hits
            .into_iter()
            .map(|(row, d)| (row_to_node(row), d))
            .collect())
    }
}

fn direction_to_bfs_dir(d: Direction) -> BfsDir {
    match d {
        Direction::Out => BfsDir::Out,
        Direction::In => BfsDir::In,
        Direction::Both => BfsDir::Both,
    }
}

/// Direction for [`Graph::impact`]. `Up` walks reverse edges (callers,
/// referrers, subclasses); `Down` walks forward edges (callees, parent
/// types); `Both` is the union.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlastDir {
    /// Reverse edges — who depends on `fqn`.
    Up,
    /// Forward edges — what `fqn` depends on.
    Down,
    /// Union of both directions.
    Both,
}

/// Traversal direction for [`TraverseSpec`] and [`Graph::shortest_path`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Follow outgoing edges (forward).
    Out,
    /// Follow incoming edges (reverse).
    In,
    /// Follow edges in both directions.
    Both,
}

/// Parameters for [`Graph::traverse`].
pub struct TraverseSpec {
    /// Starting FQNs (exact or suffix-resolved).
    pub start: Vec<String>,
    /// Which direction to walk edges.
    pub direction: Direction,
    /// If `Some`, only edges with a matching kind are followed.
    pub edge_kinds: Option<Vec<EdgeKind>>,
    /// Maximum BFS depth (0 = start nodes only, no edges followed).
    pub max_depth: usize,
    /// Maximum number of nodes in the output (excluding unresolved starts).
    pub max_nodes: usize,
}

/// A node entry in a [`Subgraph`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubgraphNode {
    /// Fully qualified name.
    pub fqn: String,
    /// Whether this FQN was successfully resolved in the graph.
    pub resolved: bool,
    /// BFS depth from the nearest start node (`None` for unresolved).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<usize>,
    /// Node kind (e.g. `"function"`, `"type"`). Empty for unresolved.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
    /// Source path. Empty for unresolved.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// Declaration line (0 for unresolved).
    #[serde(skip_serializing_if = "is_zero")]
    pub line: u32,
}

fn is_zero(v: &u32) -> bool {
    *v == 0
}

/// Subgraph returned by [`Graph::traverse`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Subgraph {
    /// Nodes in the subgraph, sorted by `(depth, fqn)`.
    pub nodes: Vec<SubgraphNode>,
    /// Edges in the subgraph, sorted by `(from, kind, to)`.
    pub edges: Vec<Edge>,
    /// `true` when `max_nodes` was hit and the graph was cut short.
    pub truncated: bool,
    /// FQNs from `TraverseSpec::start` that could not be resolved.
    pub unresolved: Vec<String>,
}

impl Subgraph {
    /// Serialize to a JSON `Value`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Render as a Graphviz `digraph` string.
    pub fn to_dot(&self) -> String {
        let mut out = String::from("digraph {\n");
        for n in &self.nodes {
            let label = n.fqn.rsplit("::").next().unwrap_or(&n.fqn);
            out.push_str(&format!(
                "  \"{}\" [label=\"{}\"];\n",
                n.fqn,
                label.replace('"', "\\\"")
            ));
        }
        for e in &self.edges {
            out.push_str(&format!(
                "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
                e.from, e.to, e.kind.tag()
            ));
        }
        out.push('}');
        out
    }
}

// ---- helpers ---------------------------------------------------------------

fn kind_matches(kind: &EdgeKind, filter: Option<&[EdgeKind]>) -> bool {
    filter.is_none_or(|ks| ks.contains(kind))
}

fn row_to_node(row: NodeRow) -> Node {
    let path = row
        .payload
        .get("path")
        .and_then(|v| v.as_str())
        .map(PathBuf::from)
        .unwrap_or_default();
    let line = row
        .payload
        .get("line")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    Node {
        fqn: row.id,
        kind: row.kind,
        path,
        line,
    }
}

fn row_to_edge(row: EdgeRow) -> Option<Edge> {
    Some(Edge {
        from: row.src,
        to: row.dst,
        kind: EdgeKind::from_tag(&row.kind)?,
    })
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
            kind: "function".to_string(),
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

    // Build a simple linear chain: a -> b -> c
    async fn seed_chain(g: &Graph) {
        g.upsert_node(node("a")).await.expect("upsert a");
        g.upsert_node(node("b")).await.expect("upsert b");
        g.upsert_node(node("c")).await.expect("upsert c");
        g.upsert_edge(edge("a", "b", EdgeKind::Calls)).await.expect("a->b");
        g.upsert_edge(edge("b", "c", EdgeKind::Calls)).await.expect("b->c");
    }

    #[tokio::test]
    async fn test_traverse_unknown_fqn_returns_empty_unresolved() {
        let (g, _dir) = make_graph().await;
        let spec = TraverseSpec {
            start: vec!["no::such::symbol".to_string()],
            direction: Direction::Out,
            edge_kinds: None,
            max_depth: 5,
            max_nodes: 100,
        };
        let sg = g.traverse(&spec).await.expect("traverse");
        assert!(sg.nodes.is_empty(), "no nodes for unknown fqn");
        assert!(sg.edges.is_empty(), "no edges for unknown fqn");
        assert!(!sg.truncated, "not truncated");
        assert_eq!(sg.unresolved, vec!["no::such::symbol"]);
    }

    #[tokio::test]
    async fn test_traverse_cycle_terminates() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("x")).await.expect("upsert x");
        g.upsert_node(node("y")).await.expect("upsert y");
        // Cycle: x -> y -> x
        g.upsert_edge(edge("x", "y", EdgeKind::Calls)).await.expect("x->y");
        g.upsert_edge(edge("y", "x", EdgeKind::Calls)).await.expect("y->x");

        let spec = TraverseSpec {
            start: vec!["x".to_string()],
            direction: Direction::Out,
            edge_kinds: None,
            max_depth: 20,
            max_nodes: 1000,
        };
        let sg = g.traverse(&spec).await.expect("traverse cycle");
        // Should include both x and y, no infinite loop.
        let fqns: Vec<&str> = sg.nodes.iter().map(|n| n.fqn.as_str()).collect();
        assert!(fqns.contains(&"x"), "x in subgraph");
        assert!(fqns.contains(&"y"), "y in subgraph");
        assert_eq!(fqns.len(), 2, "exactly 2 nodes");
    }

    #[tokio::test]
    async fn test_traverse_max_nodes_truncation() {
        let (g, _dir) = make_graph().await;
        // Chain of 5: a->b->c->d->e
        for name in &["a", "b", "c", "d", "e"] {
            g.upsert_node(node(name)).await.expect("upsert");
        }
        g.upsert_edge(edge("a", "b", EdgeKind::Calls)).await.expect("a->b");
        g.upsert_edge(edge("b", "c", EdgeKind::Calls)).await.expect("b->c");
        g.upsert_edge(edge("c", "d", EdgeKind::Calls)).await.expect("c->d");
        g.upsert_edge(edge("d", "e", EdgeKind::Calls)).await.expect("d->e");

        let spec = TraverseSpec {
            start: vec!["a".to_string()],
            direction: Direction::Out,
            edge_kinds: None,
            max_depth: 10,
            max_nodes: 3,
        };
        let sg = g.traverse(&spec).await.expect("traverse cap");
        assert!(sg.truncated, "truncated flag must be true");
        assert!(sg.nodes.len() <= 3, "at most 3 nodes");
    }

    #[tokio::test]
    async fn test_traverse_kind_filter() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;
        // Add a References edge that should be filtered out.
        g.upsert_node(node("d")).await.expect("upsert d");
        g.upsert_edge(edge("a", "d", EdgeKind::References)).await.expect("a->d refs");

        let spec = TraverseSpec {
            start: vec!["a".to_string()],
            direction: Direction::Out,
            edge_kinds: Some(vec![EdgeKind::Calls]),
            max_depth: 5,
            max_nodes: 100,
        };
        let sg = g.traverse(&spec).await.expect("traverse kind filter");
        let fqns: Vec<&str> = sg.nodes.iter().map(|n| n.fqn.as_str()).collect();
        assert!(fqns.contains(&"b"), "b reachable via Calls");
        assert!(fqns.contains(&"c"), "c reachable via Calls");
        assert!(!fqns.contains(&"d"), "d not reachable via Calls-only filter");
    }

    #[tokio::test]
    async fn test_traverse_output_sorted() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;

        let spec = TraverseSpec {
            start: vec!["a".to_string()],
            direction: Direction::Out,
            edge_kinds: None,
            max_depth: 5,
            max_nodes: 100,
        };
        let sg = g.traverse(&spec).await.expect("traverse sorted");
        // Nodes sorted by (depth, fqn): a(0), b(1), c(2)
        assert_eq!(sg.nodes[0].fqn, "a");
        assert_eq!(sg.nodes[1].fqn, "b");
        assert_eq!(sg.nodes[2].fqn, "c");
        // Edges sorted by (from, kind, to)
        assert_eq!(sg.edges[0].from, "a");
        assert_eq!(sg.edges[1].from, "b");
    }

    #[tokio::test]
    async fn test_shortest_path_found() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;

        let path = g
            .shortest_path("a", "c", None, Direction::Out, 5)
            .await
            .expect("shortest_path")
            .expect("path exists");
        assert_eq!(path.len(), 2, "a->b->c is 2 hops");
        assert_eq!(path[0].from, "a");
        assert_eq!(path[0].to, "b");
        assert_eq!(path[1].from, "b");
        assert_eq!(path[1].to, "c");
    }

    #[tokio::test]
    async fn test_shortest_path_not_found() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;

        // c has no outgoing edges, so c -> a is unreachable going Out.
        let path = g
            .shortest_path("c", "a", None, Direction::Out, 10)
            .await
            .expect("shortest_path call");
        assert!(path.is_none(), "c cannot reach a going forward");
    }

    #[tokio::test]
    async fn test_shortest_path_same_node() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;

        let path = g
            .shortest_path("a", "a", None, Direction::Out, 5)
            .await
            .expect("shortest_path same")
            .expect("empty path for same node");
        assert!(path.is_empty(), "same start/end = 0 hops");
    }

    #[tokio::test]
    async fn test_shortest_path_unknown_endpoints() {
        let (g, _dir) = make_graph().await;
        // Neither node exists.
        let path = g
            .shortest_path("no::from", "no::to", None, Direction::Out, 5)
            .await
            .expect("call succeeds");
        assert!(path.is_none(), "unknown fqn -> None");
    }

    #[tokio::test]
    async fn test_subgraph_to_json_shape() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;

        let spec = TraverseSpec {
            start: vec!["a".to_string(), "no::such".to_string()],
            direction: Direction::Out,
            edge_kinds: None,
            max_depth: 5,
            max_nodes: 100,
        };
        let sg = g.traverse(&spec).await.expect("traverse json");
        let json = sg.to_json();
        assert!(json.get("nodes").is_some(), "nodes key");
        assert!(json.get("edges").is_some(), "edges key");
        assert!(json.get("truncated").is_some(), "truncated key");
        assert!(json.get("unresolved").is_some(), "unresolved key");
        let unresolved = json["unresolved"].as_array().expect("array");
        assert_eq!(unresolved.len(), 1);
        assert_eq!(unresolved[0].as_str().expect("str"), "no::such");
    }

    #[tokio::test]
    async fn test_subgraph_to_dot_shape() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;

        let spec = TraverseSpec {
            start: vec!["a".to_string()],
            direction: Direction::Out,
            edge_kinds: None,
            max_depth: 5,
            max_nodes: 100,
        };
        let sg = g.traverse(&spec).await.expect("traverse dot");
        let dot = sg.to_dot();
        assert!(dot.starts_with("digraph {"), "starts with digraph");
        assert!(dot.ends_with('}'), "ends with closing brace");
        assert!(dot.contains("\"a\""), "node a in dot");
        assert!(dot.contains("\"b\""), "node b in dot");
        assert!(dot.contains("calls"), "edge label calls");
    }

    // ---- spec-named tests (T-04) -------------------------------------------

    /// Spec test 1: traverse filters edge kinds and depth.
    ///
    /// Graph: A -calls-> B -imports-> C, A -extends-> D.
    /// kinds=[calls, imports] depth=2 → {A, B, C} in output, D excluded.
    /// Output nodes are sorted by (depth, fqn).
    #[tokio::test]
    async fn traverse_filters_edge_kinds_and_depth() {
        let (g, _dir) = make_graph().await;
        // Nodes
        g.upsert_node(node("A")).await.expect("upsert A");
        g.upsert_node(node("B")).await.expect("upsert B");
        g.upsert_node(node("C")).await.expect("upsert C");
        g.upsert_node(node("D")).await.expect("upsert D");
        // Edges
        g.upsert_edge(edge("A", "B", EdgeKind::Calls)).await.expect("A->B calls");
        g.upsert_edge(edge("B", "C", EdgeKind::Imports)).await.expect("B->C imports");
        g.upsert_edge(edge("A", "D", EdgeKind::Extends)).await.expect("A->D extends");

        let spec = TraverseSpec {
            start: vec!["A".to_string()],
            direction: Direction::Out,
            edge_kinds: Some(vec![EdgeKind::Calls, EdgeKind::Imports]),
            max_depth: 2,
            max_nodes: 100,
        };
        let sg = g.traverse(&spec).await.expect("traverse");

        let fqns: Vec<&str> = sg.nodes.iter().map(|n| n.fqn.as_str()).collect();
        assert!(fqns.contains(&"A"), "A must be in subgraph");
        assert!(fqns.contains(&"B"), "B must be reachable via Calls");
        assert!(fqns.contains(&"C"), "C must be reachable via Calls+Imports at depth 2");
        assert!(!fqns.contains(&"D"), "D must be excluded (Extends filtered out)");

        // Deterministic order: sorted by (depth, fqn) — A at depth 0, B at depth 1, C at depth 2.
        assert_eq!(sg.nodes[0].fqn, "A", "A first (depth 0)");
        assert_eq!(sg.nodes[1].fqn, "B", "B second (depth 1)");
        assert_eq!(sg.nodes[2].fqn, "C", "C third (depth 2)");
    }

    /// Spec test 2: traverse terminates on cycles and caps nodes.
    ///
    /// Part A: cycle A->B->A terminates (no infinite loop).
    /// Part B: 10-node chain with max_nodes=3 → truncated=true, exactly 3 nodes.
    #[tokio::test]
    async fn traverse_terminates_on_cycles_and_caps_nodes() {
        // Part A: cycle terminates
        {
            let (g, _dir) = make_graph().await;
            g.upsert_node(node("cyc_a")).await.expect("upsert cyc_a");
            g.upsert_node(node("cyc_b")).await.expect("upsert cyc_b");
            g.upsert_edge(edge("cyc_a", "cyc_b", EdgeKind::Calls)).await.expect("cyc_a->cyc_b");
            g.upsert_edge(edge("cyc_b", "cyc_a", EdgeKind::Calls)).await.expect("cyc_b->cyc_a");

            let spec = TraverseSpec {
                start: vec!["cyc_a".to_string()],
                direction: Direction::Out,
                edge_kinds: None,
                max_depth: 50,
                max_nodes: 1000,
            };
            let sg = g.traverse(&spec).await.expect("traverse cycle");
            let fqns: Vec<&str> = sg.nodes.iter().map(|n| n.fqn.as_str()).collect();
            assert!(fqns.contains(&"cyc_a"), "cyc_a in subgraph");
            assert!(fqns.contains(&"cyc_b"), "cyc_b in subgraph");
            assert_eq!(fqns.len(), 2, "cycle terminates at 2 nodes, no infinite loop");
            assert!(!sg.truncated, "not truncated — all reachable nodes fit");
        }

        // Part B: 10-node chain truncated to 3
        {
            let (g, _dir) = make_graph().await;
            let names = ["n0", "n1", "n2", "n3", "n4", "n5", "n6", "n7", "n8", "n9"];
            for name in &names {
                g.upsert_node(node(name)).await.expect("upsert");
            }
            for i in 0..names.len() - 1 {
                g.upsert_edge(edge(names[i], names[i + 1], EdgeKind::Calls))
                    .await
                    .expect("chain edge");
            }

            let spec = TraverseSpec {
                start: vec!["n0".to_string()],
                direction: Direction::Out,
                edge_kinds: None,
                max_depth: 20,
                max_nodes: 3,
            };
            let sg = g.traverse(&spec).await.expect("traverse cap");
            assert!(sg.truncated, "truncated must be true when cap hit");
            assert_eq!(sg.nodes.len(), 3, "exactly 3 nodes (the cap)");
        }
    }

    /// Spec test 3: shortest_path finds a 2-hop path and returns None for unreachable.
    ///
    /// Graph: A->B->C. A→C = 2 edges. Unrelated X: A→X = None.
    #[tokio::test]
    async fn shortest_path_finds_and_misses() {
        let (g, _dir) = make_graph().await;
        g.upsert_node(node("A")).await.expect("upsert A");
        g.upsert_node(node("B")).await.expect("upsert B");
        g.upsert_node(node("C")).await.expect("upsert C");
        g.upsert_node(node("X")).await.expect("upsert X");
        g.upsert_edge(edge("A", "B", EdgeKind::Calls)).await.expect("A->B");
        g.upsert_edge(edge("B", "C", EdgeKind::Calls)).await.expect("B->C");
        // X has no connection to A

        let found = g
            .shortest_path("A", "C", None, Direction::Out, 10)
            .await
            .expect("shortest_path call")
            .expect("path A→C must exist");
        assert_eq!(found.len(), 2, "A→C is exactly 2 edges");
        assert_eq!(found[0].from, "A");
        assert_eq!(found[0].to, "B");
        assert_eq!(found[1].from, "B");
        assert_eq!(found[1].to, "C");

        let not_found = g
            .shortest_path("A", "X", None, Direction::Out, 10)
            .await
            .expect("shortest_path call");
        assert!(not_found.is_none(), "A→X must be None (no path)");
    }

    /// Spec test 4: subgraph exports valid JSON (nodes/edges/truncated keys) and DOT.
    ///
    /// JSON must contain nodes, edges, truncated keys. DOT must contain "digraph" and edge lines.
    #[tokio::test]
    async fn subgraph_exports_json_and_dot() {
        let (g, _dir) = make_graph().await;
        seed_chain(&g).await;

        let spec = TraverseSpec {
            start: vec!["a".to_string()],
            direction: Direction::Out,
            edge_kinds: None,
            max_depth: 5,
            max_nodes: 100,
        };
        let sg = g.traverse(&spec).await.expect("traverse");

        // JSON shape
        let json = sg.to_json();
        assert!(json.get("nodes").is_some(), "JSON must have 'nodes' key");
        assert!(json.get("edges").is_some(), "JSON must have 'edges' key");
        assert!(json.get("truncated").is_some(), "JSON must have 'truncated' key");
        let nodes_arr = json["nodes"].as_array().expect("nodes is array");
        assert!(!nodes_arr.is_empty(), "nodes array must be non-empty");
        let edges_arr = json["edges"].as_array().expect("edges is array");
        assert!(!edges_arr.is_empty(), "edges array must be non-empty");

        // DOT shape
        let dot = sg.to_dot();
        assert!(dot.contains("digraph"), "DOT output must contain 'digraph'");
        // Edge lines: "from" -> "to"
        assert!(dot.contains("->"), "DOT output must contain edge arrows");
        assert!(dot.contains("\"a\""), "DOT must include node a");
        assert!(dot.contains("\"b\""), "DOT must include node b");
    }
}
