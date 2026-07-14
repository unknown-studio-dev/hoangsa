//! `memory_recall` and `memory_index` tool implementations.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::{Value, json};
use hoangsa_memory_core::{Event, Query};
use time::OffsetDateTime;
use tracing::warn;
use uuid::Uuid;

use crate::proto::ToolOutput;

use super::Server;

impl Server {
    // ---- tool impls -------------------------------------------------------

    pub(super) async fn tool_recall(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            query: String,
            #[serde(default)]
            top_k: Option<usize>,
            /// Recall scope: `"curated"` (default) = code + memory,
            /// `"archive"` = archive only, `"all"` = code + memory + archive.
            #[serde(default)]
            scope: Option<String>,
            /// Filter facts to those with any of these tags.
            #[serde(default)]
            tags: Option<Vec<String>>,
            /// Whether to persist this recall as a `QueryIssued` event.
            #[serde(default)]
            log_event: Option<bool>,
            /// Absolute fused-score floor. Chunks below this are dropped.
            /// Defaults to `0.0` — i.e. only the internal noise floor
            /// (see `retriever::NOISE_FLOOR`) applies.
            #[serde(default)]
            min_score: Option<f32>,
            /// Return full chunk bodies. Default `false` — recall gives
            /// coordinates (path, line span, preview, callers/callees) so
            /// the caller can `Read path:L-L` for full content. Set to
            /// `true` when you genuinely need the body in one round trip
            /// (agent self-prompt, batch analysis, tests).
            #[serde(default)]
            detail: Option<bool>,
        }
        let Args {
            query,
            top_k,
            scope,
            tags,
            log_event,
            min_score,
            detail,
        } = serde_json::from_value(args)?;
        let want_body = detail.unwrap_or(false);
        let sanitized = crate::sanitize::sanitize_query(&query);
        let clean_query = sanitized.clean_query;
        let scope_str = scope.as_deref().unwrap_or("curated");
        let mut q = Query {
            text: clean_query.clone(),
            top_k: top_k.unwrap_or(8).max(1),
            min_score: min_score.unwrap_or(0.0).max(0.0),
            ..Query::text("")
        };
        if let Some(t) = tags {
            q.scope.tags = t;
        }
        let include_curated = scope_str == "curated" || scope_str == "all";
        let include_archive = scope_str == "archive" || scope_str == "all";

        let mut out = if include_curated {
            self.resources().await?.retriever.recall(&q).await?
        } else {
            hoangsa_memory_core::Retrieval {
                chunks: Vec::new(),
                synthesized: None,
                correlation_id: Uuid::new_v4(),
            }
        };

        // Semantic memory search via the in-process vector store —
        // best-effort, failures are silent so recall degrades gracefully
        // when the store is disabled or the embedder failed to load.
        if include_curated
            && let Ok(col) = self.open_memory_vector().await
            && let Ok(hits) = col.query_text(&query, 5, None).await
        {
            for h in hits {
                if let Some(doc) = &h.document {
                    let kind = h
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("kind"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("memory");
                    out.chunks.push(hoangsa_memory_core::Chunk {
                        id: h.id,
                        path: PathBuf::from(format!(".hoangsa/memory/{kind}")),
                        line: 0,
                        span: (0, 0),
                        symbol: None,
                        preview: doc.chars().take(200).collect(),
                        body: doc.clone(),
                        source: hoangsa_memory_core::RetrievalSource::Markdown,
                        score: 1.0 / (1.0 + h.distance),
                        context: None,
                    });
                }
            }
        }

        // Archive search — exchange-pair conversation chunks from the
        // in-process vector store.
        if include_archive
            && let Ok(col) = self.open_archive_vector().await
            && let Ok(hits) = col.query_text(&query, 5, None).await
        {
            for h in hits {
                if let Some(doc) = &h.document {
                    let topic = h
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("topic"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("conversation");
                    out.chunks.push(hoangsa_memory_core::Chunk {
                        id: h.id,
                        path: PathBuf::from(".hoangsa/memory/archive"),
                        line: 0,
                        span: (0, 0),
                        symbol: Some(format!("[{topic}]")),
                        preview: doc.chars().take(200).collect(),
                        body: doc.clone(),
                        source: hoangsa_memory_core::RetrievalSource::Markdown,
                        score: 1.0 / (1.0 + h.distance),
                        context: None,
                    });
                }
            }
        }

        // Log a `QueryIssued` event so the strict-mode gate can prove the
        // agent actually consulted memory before mutating files. Failure
        // here is non-fatal — recall still returns the chunks — but we warn
        // because a missing log entry will defeat the gate.
        if log_event.unwrap_or(true) {
            let ev = Event::QueryIssued {
                id: Uuid::new_v4(),
                text: query,
                at: OffsetDateTime::now_utc(),
            };
            if let Err(e) = self.resources().await?.store.episodes.append(&ev).await {
                warn!(error = %e, "failed to log QueryIssued event");
            }
        }

        // Strip bodies by default — recall's job is coordinates, not
        // content. Agents looking at a hit can `Read path:start-end` if
        // they need the full body; keeping it here would flood context
        // on every query. When a chunk has no preview yet (symbol-lookup
        // path), derive one from the first lines of the body so the
        // stripped response is still useful.
        if !want_body {
            for c in out.chunks.iter_mut() {
                if c.preview.is_empty() && !c.body.is_empty() {
                    c.preview = c
                        .body
                        .lines()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join("\n");
                }
                c.body.clear();
            }
        }

        let text = render_retrieval(&out, &self.inner.root).await;
        // Serialize the full `Retrieval` so CLI `--json` sees the same
        // shape as the direct-store path. Fall back to an empty object on
        // serde failure (shouldn't happen — `Retrieval: Serialize`).
        let data = serde_json::to_value(&out).unwrap_or_else(|_| json!({}));
        Ok(ToolOutput::new(data, text))
    }

    pub(super) async fn tool_index(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize, Default)]
        struct Args {
            #[serde(default)]
            path: Option<String>,
        }
        let Args { path } = serde_json::from_value(args).unwrap_or_default();
        let src = PathBuf::from(path.unwrap_or_else(|| ".".to_string()));
        // Serialise concurrent index calls — see `Inner::index_mutex`.
        // Released at the end of this function when `_index_guard` drops.
        let _index_guard = self.inner.index_mutex.lock().await;
        let res = self.resources().await?;
        // Wire the code-chunk vector collection if available, so this
        // run actually embeds chunks. The cached `res.indexer` is kept
        // vector-less so server startup doesn't pay the embedder init
        // cost up front; we upgrade per-index here on demand.
        let stats = if let Some(col) = self.open_code_vector().await {
            let retrieve_cfg =
                hoangsa_memory_retrieve::IndexConfig::load_or_default(&self.inner.root).await;
            let mut idx = hoangsa_memory_retrieve::Indexer::new(
                res.store.clone(),
                hoangsa_memory_parse::LanguageRegistry::new(),
            )
            .with_config(&retrieve_cfg);
            idx = idx.with_vector_store(col);
            idx.index_path(&src).await?
        } else {
            res.indexer.index_path(&src).await?
        };
        let reparsed = stats.files.saturating_sub(stats.files_skipped);
        // Counts are deltas for this run. `files_skipped` = content-hash
        // cache hit (no reparse needed). Callers that want lifetime totals
        // should query `memory_show` or read the KV directly.
        let text = format!(
            "indexed {}: {} file(s) — {} reparsed, {} cached. Δ: {} chunks, {} symbols, {} calls, {} imports",
            src.display(),
            stats.files,
            reparsed,
            stats.files_skipped,
            stats.chunks,
            stats.symbols,
            stats.calls,
            stats.imports,
        );
        let data = json!({
            "path": src.display().to_string(),
            "files": stats.files,
            "files_reparsed": reparsed,
            "files_skipped": stats.files_skipped,
            "chunks": stats.chunks,
            "symbols": stats.symbols,
            "calls": stats.calls,
            "imports": stats.imports,
            "embedded": stats.embedded,
        });
        Ok(ToolOutput::new(data, text))
    }
}

// ===========================================================================
// Rendering helpers
// ===========================================================================

async fn render_retrieval(r: &hoangsa_memory_core::Retrieval, root: &Path) -> String {
    // The rendering lives on `Retrieval::render_with()` so the CLI and
    // the MCP-text surface stay byte-for-byte identical. Budgets come
    // from `<root>/config.toml [output]` (max_body_lines, max_total_bytes),
    // so operators can tune the context cost of recall without rebuilding.
    let cfg = hoangsa_memory_retrieve::OutputConfig::load_or_default(root).await;
    r.render_with(&cfg.render_options())
}

// ===========================================================================
// Index-mutex serialization test
// ===========================================================================

#[cfg(test)]
mod index_mutex {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn tool_index_serialises_concurrent_calls() {
        // Two concurrent `memory_index` calls on the same source tree
        // must not interleave: whichever runs first writes content-
        // hash sentinels into kv, and the second call sees them and
        // short-circuits every file as a cache hit. Without the
        // `index_mutex` both calls would see "no sentinel" at start
        // and both would re-parse every file.
        let src_dir = tempdir().unwrap();
        for i in 0..10 {
            let body = format!(
                "pub fn item_{i}(x: i32) -> i32 {{ x + {i} }}\n"
            );
            tokio::fs::write(src_dir.path().join(format!("m_{i}.rs")), body)
                .await
                .unwrap();
        }

        let mem_dir = tempdir().unwrap();
        let srv = Server::open(mem_dir.path()).await.unwrap();

        let args = json!({ "path": src_dir.path().to_string_lossy() });
        let a = {
            let srv = srv.clone();
            let args = args.clone();
            tokio::spawn(async move { srv.tool_index(args).await.unwrap() })
        };
        let b = {
            let srv = srv.clone();
            let args = args.clone();
            tokio::spawn(async move { srv.tool_index(args).await.unwrap() })
        };
        let out_a = a.await.unwrap();
        let out_b = b.await.unwrap();

        let skipped_a = out_a.data["files_skipped"].as_u64().unwrap_or(0);
        let skipped_b = out_b.data["files_skipped"].as_u64().unwrap_or(0);
        let files_a = out_a.data["files"].as_u64().unwrap_or(0);
        let files_b = out_b.data["files"].as_u64().unwrap_or(0);

        assert!(files_a >= 10 && files_b >= 10, "both calls walked the tree");
        // Serialization guarantee: the second-to-run call sees every
        // file as a cache hit. Symmetric because we can't tell which
        // task the runtime picked first.
        let one_was_fully_cached = skipped_a == files_a || skipped_b == files_b;
        assert!(
            one_was_fully_cached,
            "expected one call to see full cache hits (ran after the other); \
             got skipped_a={skipped_a}/{files_a}, skipped_b={skipped_b}/{files_b}"
        );
    }
}
