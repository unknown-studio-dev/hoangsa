//! Code-graph tools: impact, symbol context, event trace, and diff-driven
//! change detection.

use serde::Deserialize;
use serde_json::{Value, json};

use crate::proto::ToolOutput;

use super::Server;

impl Server {
    // ---- graph tools -----------------------------------------------------

    /// Resolve `fqn` against the graph with the standard suffix-fallback
    /// + ambiguity UX used by `tool_impact` and `tool_symbol_context`.
    ///
    /// Returns `Ok((node, canonical_fqn))` on a unique match; on miss or
    /// ambiguity returns `Err(ToolOutput)` pre-rendered as an error.
    /// Keeps the fuzzy-FQN behaviour in one place so the agent-facing
    /// error text stays consistent across graph tools.
    async fn resolve_fqn_for_tool(
        &self,
        fqn: &str,
    ) -> anyhow::Result<Result<(hoangsa_memory_graph::Node, String), ToolOutput>> {
        let res = self.resources().await?;
        let g = &res.graph;
        if let Some(n) = g.get(fqn).await? {
            let canonical = n.fqn.clone();
            return Ok(Ok((n, canonical)));
        }
        let candidates = g.find_suffix_candidates(fqn).await?;
        match candidates.len() {
            1 => {
                let n = candidates.into_iter().next().expect("len==1");
                let canonical = n.fqn.clone();
                Ok(Ok((n, canonical)))
            }
            0 => Ok(Err(ToolOutput::error(format!(
                "symbol not found: {fqn}. \
                 Graph keys are `module::name` (e.g. `rule::cmd_rule_add`); \
                 call `memory_recall` first if you don't know the exact FQN."
            )))),
            _ => {
                let shown = candidates.len().min(10);
                let mut text = format!(
                    "symbol {fqn:?} is ambiguous — {} candidates share that suffix:\n",
                    candidates.len(),
                );
                for c in candidates.iter().take(shown) {
                    text.push_str(&format!(
                        "  {}  {}:{}\n",
                        c.fqn,
                        c.path.display(),
                        c.line
                    ));
                }
                if candidates.len() > shown {
                    text.push_str(&format!("  … +{} more\n", candidates.len() - shown));
                }
                text.push_str("(rerun with the exact FQN from the list above)");
                Ok(Err(ToolOutput::error(text)))
            }
        }
    }

    /// Blast-radius analysis: BFS from an FQN, grouped by distance.
    ///
    /// With `direction = "up"` this answers "what breaks if I change X?";
    /// `"down"` answers "what does X depend on?"; `"both"` is the union.
    /// The edge kinds followed depend on direction — see
    /// [`hoangsa_memory_graph::Graph::impact`].
    pub(super) async fn tool_impact(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            fqn: String,
            #[serde(default)]
            direction: Option<String>,
            #[serde(default)]
            depth: Option<usize>,
        }
        let Args {
            fqn,
            direction,
            depth,
        } = serde_json::from_value(args)?;
        let depth = depth.unwrap_or(3).clamp(1, 8);
        let dir = match direction.as_deref().unwrap_or("up") {
            "up" | "callers" | "incoming" => hoangsa_memory_graph::BlastDir::Up,
            "down" | "callees" | "outgoing" => hoangsa_memory_graph::BlastDir::Down,
            "both" => hoangsa_memory_graph::BlastDir::Both,
            other => {
                anyhow::bail!("invalid direction {other:?}; expected one of: up | down | both")
            }
        };

        let fqn = match self.resolve_fqn_for_tool(&fqn).await? {
            Ok((_, canonical)) => canonical,
            Err(err) => return Ok(err),
        };

        let hits = self.resources().await?.graph.impact(&fqn, dir, depth).await?;

        // Group by depth for a stable, readable rendering. `BTreeMap`
        // keeps the keys in ascending order without an extra sort.
        let mut by_depth: std::collections::BTreeMap<usize, Vec<&hoangsa_memory_graph::Node>> =
            std::collections::BTreeMap::new();
        for (node, d) in &hits {
            by_depth.entry(*d).or_default().push(node);
        }

        // Above `impact_group_threshold` nodes, flip to a file-grouped
        // summary per depth ring. A flat list of 200 FQNs drowns the
        // useful signal (which files are involved?); grouping counts
        // nodes per file, ordered by hit density, so the caller sees
        // the tightly-coupled subsystems at a glance. Structured `data`
        // (JSON) is unchanged — the cap is text-surface only.
        let output_cfg = hoangsa_memory_retrieve::OutputConfig::load_or_default(&self.inner.root).await;
        let group_by_file =
            output_cfg.impact_group_threshold > 0 && hits.len() > output_cfg.impact_group_threshold;

        let mut text = format!(
            "impact({fqn}, direction={}, depth={depth}) — {} nodes{}\n",
            match dir {
                hoangsa_memory_graph::BlastDir::Up => "up",
                hoangsa_memory_graph::BlastDir::Down => "down",
                hoangsa_memory_graph::BlastDir::Both => "both",
            },
            hits.len(),
            if group_by_file {
                " (grouped by file — raise `output.impact_group_threshold` for the flat list)"
            } else {
                ""
            },
        );
        for (d, nodes) in &by_depth {
            text.push_str(&format!("  depth {d}:\n"));
            if group_by_file {
                // Bucket nodes in this ring by their source file, then
                // sort buckets by descending count so the most
                // concentrated dependents surface first.
                let mut by_file: std::collections::BTreeMap<
                    std::path::PathBuf,
                    Vec<&hoangsa_memory_graph::Node>,
                > = std::collections::BTreeMap::new();
                for n in nodes {
                    by_file.entry(n.path.clone()).or_default().push(*n);
                }
                let mut ordered: Vec<_> = by_file.into_iter().collect();
                ordered.sort_by(|(pa, a), (pb, b)| b.len().cmp(&a.len()).then_with(|| pa.cmp(pb)));
                for (path, bucket) in ordered {
                    // Show up to 3 example FQNs per file so the user
                    // can drill in; more than that is the same noise
                    // the grouping was meant to avoid.
                    let examples: Vec<&str> =
                        bucket.iter().take(3).map(|n| n.fqn.as_str()).collect();
                    let ellipsis = if bucket.len() > examples.len() {
                        format!(", … +{} more", bucket.len() - examples.len())
                    } else {
                        String::new()
                    };
                    text.push_str(&format!(
                        "    {}  ({} symbol{}): {}{}\n",
                        path.display(),
                        bucket.len(),
                        if bucket.len() == 1 { "" } else { "s" },
                        examples.join(", "),
                        ellipsis,
                    ));
                }
            } else {
                for n in nodes {
                    text.push_str(&format!("    {}  {}:{}\n", n.fqn, n.path.display(), n.line));
                }
            }
        }
        if hits.is_empty() {
            text.push_str("  (no reachable symbols at the requested depth)\n");
        }

        let data = json!({
            "fqn": fqn,
            "direction": match dir {
                hoangsa_memory_graph::BlastDir::Up => "up",
                hoangsa_memory_graph::BlastDir::Down => "down",
                hoangsa_memory_graph::BlastDir::Both => "both",
            },
            "depth": depth,
            "total": hits.len(),
            "by_depth": by_depth.iter().map(|(d, nodes)| {
                json!({
                    "depth": d,
                    "nodes": nodes.iter().map(|n| json!({
                        "fqn": n.fqn,
                        "kind": n.kind,
                        "path": n.path.to_string_lossy(),
                        "line": n.line,
                    })).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
        });
        Ok(ToolOutput::new(data, text))
    }

    /// 360-degree view of a symbol: callers, callees, parent types,
    /// subtypes, imports-to-this-symbol, and siblings in the same file.
    ///
    /// Unlike `memory_recall` this is a pure graph lookup keyed on the
    /// exact FQN — use it when the agent already knows the symbol it
    /// wants to understand (e.g. after a recall returned a chunk).
    pub(super) async fn tool_symbol_context(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            fqn: String,
            #[serde(default)]
            limit: Option<usize>,
        }
        let Args { fqn, limit } = serde_json::from_value(args)?;
        let limit = limit.unwrap_or(32).clamp(1, 128);

        let (self_node, fqn) = match self.resolve_fqn_for_tool(&fqn).await? {
            Ok(pair) => pair,
            Err(err) => return Ok(err),
        };
        let res = self.resources().await?;
        let g = &res.graph;

        let mut callers = g.in_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Calls).await?;
        let mut callees = g.out_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Calls).await?;
        let mut extends = g
            .out_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Extends)
            .await?;
        let mut extended_by = g.in_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Extends).await?;
        let mut references = g
            .in_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::References)
            .await?;
        let unresolved_imports = g
            .out_unresolved(&fqn, hoangsa_memory_graph::EdgeKind::Imports)
            .await?;

        // Event-bus relations — usually empty for plain functions; the
        // sections render only when populated, so cheap to always
        // request.
        // - `emits`         : `fqn → event_node`        (out Emits)
        // - `subscribes_to` : `event_node → fqn`        (in Subscribes)
        // - `emitted_by`    : `function → event_node`   (in Emits)  — when fqn IS the event node
        // - `subscribers`   : `event_node → function`   (out Subscribes) — when fqn IS the event node
        let mut emits = g
            .out_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Emits)
            .await?;
        let mut subscribes_to = g
            .in_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Subscribes)
            .await?;
        let mut emitted_by = g
            .in_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Emits)
            .await?;
        let mut subscribers = g
            .out_neighbors(&fqn, hoangsa_memory_graph::EdgeKind::Subscribes)
            .await?;

        for v in [
            &mut callers,
            &mut callees,
            &mut extends,
            &mut extended_by,
            &mut references,
            &mut emits,
            &mut subscribes_to,
            &mut emitted_by,
            &mut subscribers,
        ] {
            v.truncate(limit);
        }

        // Siblings — declared in the same file, excluding self.
        let mut siblings = g.symbols_in_file(&self_node.path).await?;
        siblings.retain(|n| n.fqn != fqn);
        siblings.truncate(limit);

        let node_to_json = |n: &hoangsa_memory_graph::Node| {
            json!({
                "fqn": n.fqn,
                "kind": n.kind,
                "path": n.path.to_string_lossy(),
                "line": n.line,
            })
        };
        let data = json!({
            "fqn": fqn,
            "kind": self_node.kind,
            "path": self_node.path.to_string_lossy(),
            "line": self_node.line,
            "callers": callers.iter().map(node_to_json).collect::<Vec<_>>(),
            "callees": callees.iter().map(node_to_json).collect::<Vec<_>>(),
            "extends": extends.iter().map(node_to_json).collect::<Vec<_>>(),
            "extended_by": extended_by.iter().map(node_to_json).collect::<Vec<_>>(),
            "references": references.iter().map(node_to_json).collect::<Vec<_>>(),
            "imports_unresolved": unresolved_imports,
            "siblings": siblings.iter().map(node_to_json).collect::<Vec<_>>(),
            "emits": emits.iter().map(node_to_json).collect::<Vec<_>>(),
            "subscribes_to": subscribes_to.iter().map(node_to_json).collect::<Vec<_>>(),
            "emitted_by": emitted_by.iter().map(node_to_json).collect::<Vec<_>>(),
            "subscribers": subscribers.iter().map(node_to_json).collect::<Vec<_>>(),
        });

        let mut text = format!(
            "{} [{}]  {}:{}\n",
            self_node.fqn,
            self_node.kind,
            self_node.path.display(),
            self_node.line,
        );
        let section = |label: &str, nodes: &[hoangsa_memory_graph::Node], buf: &mut String| {
            if nodes.is_empty() {
                return;
            }
            buf.push_str(&format!("  {label}:\n"));
            for n in nodes {
                buf.push_str(&format!(
                    "    {}  ({}) {}:{}\n",
                    n.fqn,
                    n.kind,
                    n.path.display(),
                    n.line
                ));
            }
        };
        section("callers", &callers, &mut text);
        section("callees", &callees, &mut text);
        section("extends", &extends, &mut text);
        section("extended_by", &extended_by, &mut text);
        section("references", &references, &mut text);
        section("emits", &emits, &mut text);
        section("subscribes_to", &subscribes_to, &mut text);
        section("emitted_by", &emitted_by, &mut text);
        section("subscribers", &subscribers, &mut text);
        section("siblings", &siblings, &mut text);
        if !unresolved_imports.is_empty() {
            text.push_str("  imports (external):\n");
            for i in &unresolved_imports {
                text.push_str(&format!("    {i}\n"));
            }
        }

        Ok(ToolOutput::new(data, text))
    }

    /// Find every emitter and subscriber for a given event topic.
    ///
    /// Event FQNs are stored as `event::<bus>::<topic>`. With only
    /// `topic` supplied, returns hits across all buses; with `bus`
    /// supplied, restricts to that receiver. The lookup uses the
    /// existing `find_nodes_by_suffix` infrastructure plus a `kind ==
    /// "event"` filter so non-event FQNs that happen to end with the
    /// topic string don't pollute the result.
    pub(super) async fn tool_event_trace(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            topic: String,
            #[serde(default)]
            bus: Option<String>,
            #[serde(default)]
            limit: Option<usize>,
        }
        let Args { topic, bus, limit } = serde_json::from_value(args)?;
        let limit = limit.unwrap_or(32).clamp(1, 128);
        if topic.trim().is_empty() {
            return Ok(ToolOutput::error("topic must be non-empty".to_string()));
        }
        let res = self.resources().await?;
        let g = &res.graph;
        let store = &res.store;

        let bus_prefix = bus
            .as_deref()
            .map(|b| format!("event::{b}::"))
            .unwrap_or_else(|| "event::".to_string());

        // O(|NODES|) scan via the existing suffix index. Event nodes
        // share the same `nodes` table as everything else, so this is
        // the same cost as `find_suffix_candidates` already pays.
        let candidates = store.kv.find_nodes_by_suffix(&topic).await?;
        let mut event_fqns: Vec<String> = candidates
            .into_iter()
            .filter(|r| r.kind == "event" && r.id.starts_with(&bus_prefix))
            .map(|r| r.id)
            .collect();
        event_fqns.sort();
        event_fqns.dedup();

        let mut events_payload: Vec<serde_json::Value> = Vec::new();
        let node_to_json = |n: &hoangsa_memory_graph::Node| {
            json!({
                "fqn": n.fqn,
                "kind": n.kind,
                "path": n.path.to_string_lossy(),
                "line": n.line,
            })
        };
        let mut text = String::new();
        if event_fqns.is_empty() {
            text.push_str(&format!("no event matches: topic={topic}"));
            if let Some(b) = &bus {
                text.push_str(&format!(" bus={b}"));
            }
            text.push('\n');
        }
        for event_fqn in &event_fqns {
            let mut emitters = g
                .in_neighbors(event_fqn, hoangsa_memory_graph::EdgeKind::Emits)
                .await?;
            let mut subscribers = g
                .out_neighbors(event_fqn, hoangsa_memory_graph::EdgeKind::Subscribes)
                .await?;
            emitters.truncate(limit);
            subscribers.truncate(limit);

            text.push_str(&format!("{event_fqn}\n"));
            if !emitters.is_empty() {
                text.push_str("  emitted by:\n");
                for n in &emitters {
                    text.push_str(&format!(
                        "    {}  ({}) {}:{}\n",
                        n.fqn,
                        n.kind,
                        n.path.display(),
                        n.line
                    ));
                }
            }
            if !subscribers.is_empty() {
                text.push_str("  subscribers:\n");
                for n in &subscribers {
                    text.push_str(&format!(
                        "    {}  ({}) {}:{}\n",
                        n.fqn,
                        n.kind,
                        n.path.display(),
                        n.line
                    ));
                }
            }
            events_payload.push(json!({
                "fqn": event_fqn,
                "emitters": emitters.iter().map(&node_to_json).collect::<Vec<_>>(),
                "subscribers": subscribers.iter().map(&node_to_json).collect::<Vec<_>>(),
            }));
        }

        let data = json!({
            "topic": topic,
            "bus": bus,
            "events": events_payload,
        });
        Ok(ToolOutput::new(data, text))
    }

    /// Given a unified diff, return the symbols the edit touches plus
    /// their upstream blast radius (who calls / references / inherits
    /// from them). Handy as a PR pre-check: "these 7 functions need
    /// re-testing because you modified X".
    ///
    /// Input is a diff text blob (what `git diff` produces). Hunks
    /// that touch files not in the graph are silently ignored.
    pub(super) async fn tool_detect_changes(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            diff: String,
            #[serde(default)]
            depth: Option<usize>,
        }
        let Args { diff, depth } = serde_json::from_value(args)?;
        let depth = depth.unwrap_or(2).clamp(1, 6);

        let hunks = parse_unified_diff(&diff);
        if hunks.is_empty() {
            return Ok(ToolOutput::error(
                "diff contained no parseable hunks; expected `git diff` output".to_string(),
            ));
        }

        // Collect touched symbols: for every hunk, intersect its post-
        // image line range with the declaration spans of symbols in
        // the file. We use `symbols_in_file` on the post-image path
        // because that's the identity after the edit.
        let res = self.resources().await?;
        let g = &res.graph;
        let store = &res.store;
        let mut touched: std::collections::BTreeMap<String, hoangsa_memory_graph::Node> =
            std::collections::BTreeMap::new();
        let mut file_hits: Vec<serde_json::Value> = Vec::new();

        for DiffHunk { path, ranges } in &hunks {
            // Look up all symbol rows for this file (which carry the
            // `(start, end)` line span we need to test hunk overlap). Then
            // fetch the matching graph Nodes for rendering via a second
            // round trip — nodes and rows key on the same FQN but live in
            // different tables.
            // Diffs can arrive with any of three path flavours depending on
            // how `git diff` was invoked: `cli/src/cmd/rule.rs` (cwd-rel),
            // `./cli/src/cmd/rule.rs` (dot-prefixed), or absolute. The
            // symbols table could have been populated with a different
            // flavour when the repo was indexed (`hoangsa-memory index .` vs
            // `hoangsa-memory index /abs/path`). Go through the lenient lookup so a
            // PR pre-check actually finds the symbols instead of silently
            // returning "no overlap".
            let path_buf = std::path::PathBuf::from(path);
            let sym_rows = match store.kv.symbols_for_path_like(&path_buf).await {
                Ok(r) => r,
                Err(_) => continue,
            };
            if sym_rows.is_empty() {
                continue;
            }
            let nodes = g.symbols_in_file_like(&path_buf).await?;
            let by_fqn: std::collections::HashMap<&str, &hoangsa_memory_graph::Node> =
                nodes.iter().map(|n| (n.fqn.as_str(), n)).collect();

            let mut hit_in_file: Vec<String> = Vec::new();
            for row in &sym_rows {
                let (s, e) = (row.start_line, row.end_line);
                if ranges.iter().any(|(a, b)| !(s > *b || e < *a))
                    && let Some(n) = by_fqn.get(row.fqn.as_str())
                {
                    touched.insert(n.fqn.clone(), (*n).clone());
                    hit_in_file.push(n.fqn.clone());
                }
            }
            if !hit_in_file.is_empty() {
                file_hits.push(json!({
                    "path": path,
                    "hunks": ranges.len(),
                    "touched": hit_in_file,
                }));
            }
        }

        if touched.is_empty() {
            let text = format!(
                "diff touched {} file(s) but no indexed symbols overlapped any hunk",
                hunks.len()
            );
            return Ok(ToolOutput::new(
                json!({ "touched": [], "impact": [], "hunks": hunks.len() }),
                text,
            ));
        }

        // Blast radius: for every touched symbol, upstream impact. Union
        // into a single de-duped set so cross-symbol overlap (common on
        // real PRs) is naturally collapsed.
        let mut impact_seen: std::collections::HashMap<String, (hoangsa_memory_graph::Node, usize)> =
            std::collections::HashMap::new();
        for node in touched.values() {
            let radius = g
                .impact(&node.fqn, hoangsa_memory_graph::BlastDir::Up, depth)
                .await?;
            for (n, d) in radius {
                // Keep the *shortest* distance seen across all roots so
                // a symbol reached both directly and transitively is
                // rendered at its true minimum depth.
                impact_seen
                    .entry(n.fqn.clone())
                    .and_modify(|existing| {
                        if d < existing.1 {
                            existing.1 = d;
                        }
                    })
                    .or_insert((n, d));
            }
        }
        // Don't double-list the touched symbols themselves as part of
        // their own blast radius.
        for fqn in touched.keys() {
            impact_seen.remove(fqn);
        }

        let mut impact_vec: Vec<(hoangsa_memory_graph::Node, usize)> = impact_seen.into_values().collect();
        impact_vec.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.fqn.cmp(&b.0.fqn)));

        let node_json = |n: &hoangsa_memory_graph::Node| {
            json!({
                "fqn": n.fqn,
                "kind": n.kind,
                "path": n.path.to_string_lossy(),
                "line": n.line,
            })
        };
        let data = json!({
            "hunks": hunks.len(),
            "files": file_hits,
            "touched": touched.values().map(node_json).collect::<Vec<_>>(),
            "impact": impact_vec.iter().map(|(n, d)| {
                let mut v = node_json(n);
                v["depth"] = json!(d);
                v
            }).collect::<Vec<_>>(),
            "depth": depth,
        });

        // Cap the impact list in the text surface so a wide-blast PR
        // pre-check (200+ nodes) doesn't drown the agent in output —
        // structured `data.impact` still carries the full set for
        // programmatic consumers (CLI --json).
        let output_cfg = hoangsa_memory_retrieve::OutputConfig::load_or_default(&self.inner.root).await;
        let group_threshold = output_cfg.impact_group_threshold.max(1);
        let group_impact = impact_vec.len() > group_threshold;

        let mut text = format!(
            "diff touched {} symbol(s) across {} file(s); upstream blast radius (depth {depth}): {} node(s){}\n",
            touched.len(),
            file_hits.len(),
            impact_vec.len(),
            if group_impact { " (grouped by file)" } else { "" },
        );
        text.push_str("touched:\n");
        for n in touched.values() {
            text.push_str(&format!("  {}  {}:{}\n", n.fqn, n.path.display(), n.line));
        }
        if !impact_vec.is_empty() {
            text.push_str("impact:\n");
            if group_impact {
                let mut by_file: std::collections::BTreeMap<
                    std::path::PathBuf,
                    Vec<(&hoangsa_memory_graph::Node, usize)>,
                > = std::collections::BTreeMap::new();
                for (n, d) in &impact_vec {
                    by_file.entry(n.path.clone()).or_default().push((n, *d));
                }
                let mut ordered: Vec<_> = by_file.into_iter().collect();
                ordered.sort_by(|(pa, a), (pb, b)| b.len().cmp(&a.len()).then_with(|| pa.cmp(pb)));
                for (path, bucket) in ordered {
                    let examples: Vec<String> = bucket
                        .iter()
                        .take(3)
                        .map(|(n, d)| format!("{}@{d}", n.fqn))
                        .collect();
                    let ellipsis = if bucket.len() > examples.len() {
                        format!(", … +{} more", bucket.len() - examples.len())
                    } else {
                        String::new()
                    };
                    text.push_str(&format!(
                        "  {}  ({}): {}{}\n",
                        path.display(),
                        bucket.len(),
                        examples.join(", "),
                        ellipsis,
                    ));
                }
            } else {
                for (n, d) in &impact_vec {
                    text.push_str(&format!(
                        "  @{d}  {}  {}:{}\n",
                        n.fqn,
                        n.path.display(),
                        n.line
                    ));
                }
            }
        }

        Ok(ToolOutput::new(data, text))
    }
}

/// One parsed hunk: a file path + every post-image line range the diff
/// touches inside that file. Pure value, Display-free — the caller joins
/// with the graph to get symbol-level resolution.
#[derive(Debug)]
struct DiffHunk {
    path: String,
    /// `(start, end)` inclusive line ranges, 1-based. A pure-deletion
    /// hunk at post-image line N is represented as `(N, N)` so it still
    /// overlaps any symbol whose declaration spans N.
    ranges: Vec<(u32, u32)>,
}

/// Parse a git unified diff into per-file line-range hunks.
///
/// Accepts the output of `git diff` / `git diff --staged` as well as
/// rustfmt-style patches. Binary / rename-only entries are skipped.
/// Paths are taken from the `+++ b/...` header (falling back to `--- a/...`
/// for pure deletions where the `+++` is `/dev/null`).
fn parse_unified_diff(diff: &str) -> Vec<DiffHunk> {
    let mut out: Vec<DiffHunk> = Vec::new();
    let mut current_path: Option<String> = None;
    let mut current_ranges: Vec<(u32, u32)> = Vec::new();

    fn flush(out: &mut Vec<DiffHunk>, path: &mut Option<String>, ranges: &mut Vec<(u32, u32)>) {
        if let Some(p) = path.take() {
            if !ranges.is_empty() {
                out.push(DiffHunk {
                    path: p,
                    ranges: std::mem::take(ranges),
                });
            } else {
                // Pure rename / binary — drop silently.
                ranges.clear();
            }
        }
    }

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("+++ ") {
            flush(&mut out, &mut current_path, &mut current_ranges);
            // `+++ b/path` or `+++ /dev/null` — tolerate both.
            let raw = rest.trim();
            let path = raw.strip_prefix("b/").unwrap_or(raw);
            if path != "/dev/null" {
                current_path = Some(path.to_string());
            }
        } else if line.starts_with("--- ") {
            // Handle the fallback where the post-image is /dev/null
            // (pure deletion) — we still want to emit a "file touched"
            // record so the caller sees it, but we have no post-image
            // lines. Record the pre-image path against an empty range
            // list; `flush` will drop it cleanly because `ranges` stays
            // empty.
            if current_path.is_none()
                && let Some(rest) = line.strip_prefix("--- ")
            {
                let raw = rest.trim();
                let path = raw.strip_prefix("a/").unwrap_or(raw);
                if path != "/dev/null" {
                    current_path = Some(path.to_string());
                }
            }
        } else if let Some(rest) = line.strip_prefix("@@ ") {
            // `@@ -a,b +c,d @@ ...` — we only care about the `+c,d` half.
            // `d` defaults to `1` if omitted (per unified-diff spec).
            if let Some(end) = rest.find(" @@")
                && let Some((start, count)) = parse_post_image_range(&rest[..end])
                && count > 0
                && current_path.is_some()
            {
                current_ranges.push((start, start + count - 1));
            }
        }
    }
    flush(&mut out, &mut current_path, &mut current_ranges);
    out
}

/// Parse the `+c,d` half of a `@@ -a,b +c,d @@` hunk header. `d` is
/// optional and defaults to `1` per the unified-diff spec.
fn parse_post_image_range(header: &str) -> Option<(u32, u32)> {
    let plus = header.split_whitespace().find(|p| p.starts_with('+'))?;
    let body = plus.trim_start_matches('+');
    let (start_str, count_str) = match body.split_once(',') {
        Some((s, c)) => (s, c),
        None => (body, "1"),
    };
    Some((start_str.parse().ok()?, count_str.parse().ok()?))
}
