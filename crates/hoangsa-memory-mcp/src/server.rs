//! MCP server core: request dispatch and tool implementations.
//!
//! The transport layer (stdio) lives at the bottom of this file in
//! [`run_stdio`]; the rest is pure logic driven by a [`Server`] handle.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{Value, json};
use hoangsa_memory_core::{
    Enforcement, Event, Fact, FactScope, Lesson, LessonTrigger, MemoryKind, MemoryMeta, Query,
};
use hoangsa_memory_policy::{
    CapExceededError, CurationConfig, GuardedAppendError, MarkdownStoreMemoryExt, MemoryConfig,
    MemoryKind as MdKind,
};
use hoangsa_memory_parse::LanguageRegistry;
use hoangsa_memory_retrieve::{ChromaConfig, Indexer, RetrieveConfig, Retriever};
use hoangsa_memory_store::{ChromaCol, ChromaStore, StoreRoot};
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::{debug, warn};
use uuid::Uuid;

use hoangsa_memory_retrieve::WatchConfig;

use crate::proto::{
    CallToolResult, Capabilities, ContentBlock, GetPromptResult, InitializeResult,
    MCP_PROTOCOL_VERSION, Prompt, PromptArgument, PromptMessage, Resource, ResourceContents,
    RpcError, RpcIncoming, RpcResponse, ServerInfo, Tool, ToolOutput, error_codes,
};

/// URI of the `MEMORY.md` resource.
const MEMORY_URI: &str = "hoangsa-memory://memory/MEMORY.md";
/// URI of the `LESSONS.md` resource.
const LESSONS_URI: &str = "hoangsa-memory://memory/LESSONS.md";

// ===========================================================================
// Server
// ===========================================================================

/// MCP server handle. Cheap to clone — all backing state is behind `Arc`.
#[derive(Clone)]
pub struct Server {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub(crate) root: PathBuf,
    store: StoreRoot,
    indexer: Indexer,
    retriever: Retriever,
    graph: hoangsa_memory_graph::Graph,
    chroma: tokio::sync::OnceCell<Option<ChromaStore>>,
    chroma_enabled: bool,
}

impl Server {
    /// Open a server rooted at `path` (the `.hoangsa-memory/` directory).
    ///
    /// ChromaDB (and its ONNX embedder) is **not** loaded here — it is
    /// lazily initialized on first use to avoid the ~2 GB RSS hit when
    /// no vector operation is needed.
    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let root = path.as_ref().to_path_buf();
        let store = StoreRoot::open(&root).await?;
        let retrieve_cfg = RetrieveConfig::load_or_default(&root).await;
        let chroma_enabled = Self::is_chroma_enabled(&root).await;

        let indexer = Indexer::new(store.clone(), LanguageRegistry::new());
        let retriever =
            Retriever::new(store.clone()).with_markdown_boost(retrieve_cfg.rerank_markdown_boost);

        let graph = hoangsa_memory_graph::Graph::new(store.kv.clone());
        Ok(Self {
            inner: Arc::new(Inner {
                root,
                store,
                indexer,
                retriever,
                graph,
                chroma: tokio::sync::OnceCell::new(),
                chroma_enabled,
            }),
        })
    }

    async fn is_chroma_enabled(root: &Path) -> bool {
        ChromaConfig::load_or_default(root).await.enabled
    }

    async fn get_chroma(&self) -> Option<&ChromaStore> {
        if !self.inner.chroma_enabled {
            return None;
        }
        let root = self.inner.root.clone();
        let store = self
            .inner
            .chroma
            .get_or_init(|| async {
                let cfg = ChromaConfig::load_or_default(&root).await;
                let path = cfg.data_path.unwrap_or_else(|| {
                    StoreRoot::chroma_path(&root)
                        .to_string_lossy()
                        .to_string()
                });
                match ChromaStore::open(&path).await {
                    Ok(s) => {
                        tracing::info!(path = %path, "ChromaDB sidecar started (lazy init)");
                        Some(s)
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "ChromaDB sidecar init failed");
                        None
                    }
                }
            })
            .await;
        store.as_ref()
    }

    /// Spawn a background file watcher if `[watch] enabled = true` in
    /// `config.toml`. The watcher reuses the server's `Indexer` so there
    /// is no lock contention with the MCP daemon. Returns `true` if a
    /// watcher was spawned.
    ///
    /// `src` is the source tree to watch (typically the project root,
    /// i.e. the parent of `.hoangsa-memory/`).
    pub async fn spawn_watcher(&self, src: PathBuf) -> bool {
        let cfg = WatchConfig::load_or_default(&self.inner.root).await;
        if !cfg.enabled {
            return false;
        }
        let debounce = std::time::Duration::from_millis(cfg.debounce_ms);
        let inner = Arc::clone(&self.inner);
        tokio::spawn(async move {
            if let Err(e) = run_watcher(inner, src, debounce).await {
                warn!(error = %e, "background watcher exited");
            }
        });
        true
    }

    /// Dispatch a single request. Returns `Ok(None)` for notifications.
    pub async fn handle(&self, msg: RpcIncoming) -> Option<RpcResponse> {
        let is_note = msg.is_notification();
        let id = msg.id.clone().unwrap_or(Value::Null);

        let outcome = match msg.method.as_str() {
            "initialize" => Ok(self.initialize()),
            "initialized" | "notifications/initialized" => {
                // Notification — silently accept.
                return None;
            }
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.tools_list()),
            "tools/call" => self.tools_call(msg.params).await,
            // hoangsa-memory-private extension: same dispatch as
            // `tools/call` but returns the raw `ToolOutput` (with structured
            // `data`) instead of the text-only `CallToolResult`. Consumed
            // by the CLI thin-client so it can honour `--json` and
            // pretty-print.
            "hoangsa-memory.call" => self.memory_call(msg.params).await,
            "resources/list" => Ok(self.resources_list()),
            "resources/read" => self.resources_read(msg.params).await,
            "prompts/list" => Ok(self.prompts_list()),
            "prompts/get" => self.prompts_get(msg.params).await,
            other => Err(RpcError::new(
                error_codes::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        };

        if is_note {
            if let Err(e) = &outcome {
                warn!(code = e.code, msg = %e.message, "notification error (dropped)");
            }
            return None;
        }

        Some(match outcome {
            Ok(result) => RpcResponse::ok(id, result),
            Err(err) => RpcResponse::err(id, err),
        })
    }

    // ---- method handlers --------------------------------------------------

    fn initialize(&self) -> Value {
        let result = InitializeResult {
            protocol_version: MCP_PROTOCOL_VERSION,
            capabilities: Capabilities {
                tools: Some(json!({})),
                resources: Some(json!({})),
                prompts: Some(json!({})),
            },
            server_info: ServerInfo {
                name: "hoangsa-memory-mcp".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        };
        serde_json::to_value(result).unwrap_or_else(|_| json!({}))
    }

    fn tools_list(&self) -> Value {
        json!({ "tools": tools_catalog() })
    }

    /// MCP `tools/call` — returns a text-only [`CallToolResult`] (which is
    /// what every MCP client understands). The structured `data` half of
    /// [`ToolOutput`] is dropped; clients wanting the machine-readable
    /// form should call [`Self::memory_call`] via `hoangsa-memory.call`
    /// instead.
    async fn tools_call(&self, params: Value) -> Result<Value, RpcError> {
        let out = self.dispatch_tool(params).await?;
        let wrapped = CallToolResult {
            content: vec![ContentBlock::text(out.text)],
            is_error: out.is_error,
        };
        serde_json::to_value(wrapped)
            .map_err(|e| RpcError::new(error_codes::INTERNAL_ERROR, e.to_string()))
    }

    /// hoangsa-memory-private `hoangsa-memory.call` — returns the raw
    /// [`ToolOutput`] so the CLI thin-client can honour `--json` and
    /// pretty-print structured data. Dispatch logic is shared with
    /// [`Self::tools_call`].
    async fn memory_call(&self, params: Value) -> Result<Value, RpcError> {
        let out = self.dispatch_tool(params).await?;
        serde_json::to_value(out)
            .map_err(|e| RpcError::new(error_codes::INTERNAL_ERROR, e.to_string()))
    }

    /// Shared dispatch used by both `tools/call` and `hoangsa-memory.call`. Tool
    /// errors are folded into `ToolOutput { is_error: true, .. }` so the
    /// RPC layer can still emit a successful envelope (callers inspect
    /// `is_error` on the payload).
    async fn dispatch_tool(&self, params: Value) -> Result<ToolOutput, RpcError> {
        #[derive(Deserialize)]
        struct CallParams {
            name: String,
            #[serde(default)]
            arguments: Value,
        }
        let CallParams { name, arguments } = serde_json::from_value(params)
            .map_err(|e| RpcError::new(error_codes::INVALID_PARAMS, e.to_string()))?;

        let result = match name.as_str() {
            "memory_recall" => self.tool_recall(arguments).await,
            "memory_index" => self.tool_index(arguments).await,
            "memory_remember_fact" => self.tool_remember_fact(arguments).await,
            "memory_remember_lesson" => self.tool_remember_lesson(arguments).await,
            "memory_remember_preference" => self.tool_remember_preference(arguments).await,
            "memory_replace" => self.tool_memory_replace(arguments).await,
            "memory_remove" => self.tool_memory_remove(arguments).await,
            "memory_skills_list" => self.tool_skills_list().await,
            "memory_show" => self.tool_memory_show().await,
            "memory_wakeup" => self.tool_wakeup(arguments).await,
            "memory_detail" => self.tool_memory_detail(arguments).await,
            "memory_skill_propose" => self.tool_skill_propose(arguments).await,
            "memory_impact" => self.tool_impact(arguments).await,
            "memory_symbol_context" => self.tool_symbol_context(arguments).await,
            "memory_detect_changes" => self.tool_detect_changes(arguments).await,
            "memory_turn_save" => self.tool_turn_save(arguments).await,
            "memory_turns_search" => self.tool_turns_search(arguments).await,
            "memory_archive_status" => self.tool_archive_status().await,
            "memory_archive_topics" => self.tool_archive_topics(arguments).await,
            "memory_archive_search" => self.tool_archive_search(arguments).await,
            "memory_archive_ingest" => self.tool_archive_ingest(arguments).await,
            other => {
                return Err(RpcError::new(
                    error_codes::METHOD_NOT_FOUND,
                    format!("unknown tool: {other}"),
                ));
            }
        };

        Ok(match result {
            Ok(out) => out,
            Err(e) => ToolOutput::error(format!("{e:#}")),
        })
    }

    fn resources_list(&self) -> Value {
        let resources = vec![
            Resource {
                uri: MEMORY_URI.to_string(),
                name: "MEMORY.md".to_string(),
                description:
                    "Declarative facts (full text). For a compact index, use memory_wakeup."
                        .to_string(),
                mime_type: "text/markdown".to_string(),
            },
            Resource {
                uri: LESSONS_URI.to_string(),
                name: "LESSONS.md".to_string(),
                description: "Lessons learned (full text). For a compact index, use memory_wakeup."
                    .to_string(),
                mime_type: "text/markdown".to_string(),
            },
        ];
        json!({ "resources": resources })
    }

    async fn resources_read(&self, params: Value) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct ReadParams {
            uri: String,
        }
        let ReadParams { uri } = serde_json::from_value(params)
            .map_err(|e| RpcError::new(error_codes::INVALID_PARAMS, e.to_string()))?;

        let file = match uri.as_str() {
            MEMORY_URI => "MEMORY.md",
            LESSONS_URI => "LESSONS.md",
            other => {
                return Err(RpcError::new(
                    error_codes::INVALID_PARAMS,
                    format!("unknown resource uri: {other}"),
                ));
            }
        };

        let path = self.inner.root.join(file);
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(RpcError::new(error_codes::INTERNAL_ERROR, e.to_string())),
        };

        let contents = ResourceContents {
            uri,
            mime_type: "text/markdown".to_string(),
            text,
        };
        Ok(json!({ "contents": [contents] }))
    }

    // ---- tool impls -------------------------------------------------------

    async fn tool_recall(&self, args: Value) -> anyhow::Result<ToolOutput> {
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
            self.inner.retriever.recall(&q).await?
        } else {
            hoangsa_memory_core::Retrieval {
                chunks: Vec::new(),
                synthesized: None,
                correlation_id: Uuid::new_v4(),
            }
        };

        // Semantic memory search via ChromaDB — best-effort, failures are
        // silent so recall degrades gracefully when ChromaDB is down.
        if include_curated
            && let Ok(col) = self.open_memory_chroma().await
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
                        path: PathBuf::from(format!(".hoangsa-memory/{kind}")),
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

        // Archive search — exchange-pair conversation chunks from ChromaDB.
        if include_archive
            && let Ok(col) = self.open_archive_chroma().await
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
                        path: PathBuf::from(".hoangsa-memory/archive"),
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
            if let Err(e) = self.inner.store.episodes.append(&ev).await {
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

    async fn tool_index(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize, Default)]
        struct Args {
            #[serde(default)]
            path: Option<String>,
        }
        let Args { path } = serde_json::from_value(args).unwrap_or_default();
        let src = PathBuf::from(path.unwrap_or_else(|| ".".to_string()));
        // Wire the code-chunk ChromaDB collection if available, so this
        // run actually embeds chunks. The stored `self.inner.indexer` is
        // kept chroma-less so server startup doesn't pay the sidecar
        // init cost up front; we upgrade per-index here on demand.
        let stats = if let Some(col) = self.open_code_chroma().await {
            let retrieve_cfg =
                hoangsa_memory_retrieve::IndexConfig::load_or_default(&self.inner.root).await;
            let mut idx = hoangsa_memory_retrieve::Indexer::new(
                self.inner.store.clone(),
                hoangsa_memory_parse::LanguageRegistry::new(),
            )
            .with_config(&retrieve_cfg);
            idx = idx.with_chroma(std::sync::Arc::new(col));
            idx.index_path(&src).await?
        } else {
            self.inner.indexer.index_path(&src).await?
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

    async fn tool_remember_fact(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            text: String,
            #[serde(default)]
            tags: Vec<String>,
            #[serde(default)]
            stage: bool,
            #[serde(default)]
            scope: Option<String>,
        }
        let Args {
            text,
            tags,
            stage,
            scope,
        } = serde_json::from_value(args)?;
        let fact = Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: text.trim().to_string(),
            tags,
            scope: match scope.as_deref() {
                Some("on-demand" | "on_demand") => FactScope::OnDemand,
                _ => FactScope::Always,
            },
        };
        let cfg = CurationConfig::load_or_default(&self.inner.root).await;
        let mem_cfg = MemoryConfig::load_or_default(&self.inner.root).await;
        let staged = stage || cfg.requires_review();
        if staged {
            self.inner.store.markdown.append_pending_fact(&fact).await?;
            let path = self.inner.root.join("MEMORY.pending.md");
            let text = format!(
                "staged (review mode) — run `memory_promote` to accept: {}",
                first_line(&fact.text)
            );
            let data = json!({
                "text": fact.text,
                "tags": fact.tags,
                "path": path.display().to_string(),
                "staged": true,
            });
            return Ok(ToolOutput::new(data, text));
        }
        match self
            .inner
            .store
            .markdown
            .append_fact_guarded(
                &fact,
                mem_cfg.cap_memory_bytes,
                mem_cfg.strict_content_policy,
            )
            .await
        {
            Ok(()) => {
                self.upsert_memory_chroma("fact", &fact.text, &fact.tags)
                    .await;
                let path = self.inner.root.join("MEMORY.md");
                let text = format!("committed to MEMORY.md: {}", first_line(&fact.text));
                let data = json!({
                    "text": fact.text,
                    "tags": fact.tags,
                    "path": path.display().to_string(),
                    "staged": false,
                });
                Ok(ToolOutput::new(data, text))
            }
            Err(e) => Ok(guarded_error_output(e)),
        }
    }

    async fn tool_remember_lesson(&self, args: Value) -> anyhow::Result<ToolOutput> {
        // `trigger` may arrive as either a legacy bare string (back-compat) or
        // a structured `LessonTrigger` object with optional
        // tool/path_glob/cmd_regex/content_regex + required `natural` text.
        // Per REQ-03, `suggested_enforcement` is recorded as audit-only; the
        // actual enforcement tier is always `Advise` at creation time and is
        // promoted later by evidence-driven auto-promotion in the outcome
        // harvester.
        #[derive(Deserialize)]
        struct Args {
            trigger: Value,
            advice: String,
            #[serde(default)]
            suggested_enforcement: Option<Enforcement>,
            #[serde(default)]
            block_message: Option<String>,
            #[serde(default)]
            stage: bool,
        }
        let Args {
            trigger,
            advice,
            suggested_enforcement,
            block_message,
            stage,
        } = serde_json::from_value(args)?;

        let parsed_trigger: LessonTrigger = match trigger {
            Value::String(s) => LessonTrigger::natural_only(s.trim()),
            Value::Object(_) => serde_json::from_value(trigger)
                .map_err(|e| anyhow::anyhow!("invalid trigger object: {e}"))?,
            Value::Null => LessonTrigger::default(),
            other => {
                anyhow::bail!(
                    "`trigger` must be a string or structured object, got: {}",
                    other
                );
            }
        };
        // The `Lesson.trigger` string field is what the markdown store and the
        // existing conflict check key off; render the natural-text slot into
        // it. Structured matchers are surfaced via `data` in the response so
        // callers (and tests) can confirm they round-tripped.
        let trigger_natural = parsed_trigger.natural.trim().to_string();
        let lesson = Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger_natural.clone(),
            advice: advice.trim().to_string(),
            success_count: 0,
            failure_count: 0,
            // REQ-03: creation-time enforcement is always `Advise` regardless
            // of what the agent suggested.
            enforcement: Enforcement::default(),
            suggested_enforcement: suggested_enforcement.clone(),
            block_message: block_message.clone(),
        };
        let cfg = CurationConfig::load_or_default(&self.inner.root).await;
        let mem_cfg = MemoryConfig::load_or_default(&self.inner.root).await;
        let staged = stage || cfg.requires_review();

        // Conflict check: a lesson with the same trigger already exists.
        // In review mode we always stage; in auto mode we still refuse to
        // silently overwrite — force the agent to stage + escalate.
        let conflict = self
            .inner
            .store
            .markdown
            .read_lessons()
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|l| l.trigger.trim().eq_ignore_ascii_case(lesson.trigger.trim()));

        if staged || conflict.is_some() {
            self.inner
                .store
                .markdown
                .append_pending_lesson(&lesson)
                .await?;
            let note = if conflict.is_some() {
                "staged (conflict with existing lesson — user must review)"
            } else {
                "staged (review mode) — run `memory_promote` to accept"
            };
            let path = self.inner.root.join("LESSONS.pending.md");
            let text = format!("{note}: {}", lesson.trigger);
            let data = json!({
                "trigger": lesson.trigger,
                "structured_trigger": parsed_trigger,
                "advice": lesson.advice,
                "enforcement": lesson.enforcement,
                "suggested_enforcement": lesson.suggested_enforcement,
                "block_message": lesson.block_message,
                "path": path.display().to_string(),
                "staged": true,
                "conflict": conflict.map(|l| json!({
                    "trigger": l.trigger,
                    "existing_advice": l.advice,
                })),
            });
            return Ok(ToolOutput::new(data, text));
        }
        match self
            .inner
            .store
            .markdown
            .append_lesson_guarded(
                &lesson,
                mem_cfg.cap_lessons_bytes,
                mem_cfg.strict_content_policy,
            )
            .await
        {
            Ok(()) => {
                let combined = format!("WHEN: {}\nDO: {}", lesson.trigger, lesson.advice);
                self.upsert_memory_chroma("lesson", &combined, &[]).await;
                let path = self.inner.root.join("LESSONS.md");
                let text = format!("committed to LESSONS.md: {}", lesson.trigger);
                let data = json!({
                    "trigger": lesson.trigger,
                    "structured_trigger": parsed_trigger,
                    "advice": lesson.advice,
                    "enforcement": lesson.enforcement,
                    "suggested_enforcement": lesson.suggested_enforcement,
                    "block_message": lesson.block_message,
                    "path": path.display().to_string(),
                    "staged": false,
                    "conflict": Value::Null,
                });
                Ok(ToolOutput::new(data, text))
            }
            Err(e) => Ok(guarded_error_output(e)),
        }
    }

    // -- Enforcement: override request flow --------------------------------

    async fn tool_remember_preference(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            text: String,
            #[serde(default)]
            tags: Vec<String>,
        }
        let Args { text, tags } = serde_json::from_value(args)?;
        let trimmed = text.trim().to_string();
        let mem_cfg = MemoryConfig::load_or_default(&self.inner.root).await;
        match self
            .inner
            .store
            .markdown
            .append_preference_guarded(
                &trimmed,
                &tags,
                mem_cfg.cap_user_bytes,
                mem_cfg.strict_content_policy,
            )
            .await
        {
            Ok(()) => {
                let path = self.inner.root.join("USER.md");
                let rendered = format!("committed to USER.md: {}", first_line(&trimmed));
                let data = json!({
                    "text": trimmed,
                    "tags": tags,
                    "path": path.display().to_string(),
                });
                Ok(ToolOutput::new(data, rendered))
            }
            Err(e) => Ok(guarded_error_output(e)),
        }
    }

    async fn tool_memory_replace(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            kind: String,
            query: String,
            new_text: String,
        }
        let Args {
            kind,
            query,
            new_text,
        } = serde_json::from_value(args)?;
        let md_kind = parse_md_kind(&kind)?;
        let idx = self
            .inner
            .store
            .markdown
            .replace(md_kind, &query, &new_text)
            .await?;
        let path = md_kind_path(&self.inner.root, md_kind);
        let text = format!(
            "replaced entry [{idx}] in {}: {}",
            path.display(),
            first_line(&new_text)
        );
        let data = json!({
            "kind": kind,
            "index": idx,
            "new_text": new_text,
            "path": path.display().to_string(),
        });
        Ok(ToolOutput::new(data, text))
    }

    async fn tool_memory_remove(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            kind: String,
            query: String,
        }
        let Args { kind, query } = serde_json::from_value(args)?;
        let md_kind = parse_md_kind(&kind)?;
        let idx = self.inner.store.markdown.remove(md_kind, &query).await?;
        let path = md_kind_path(&self.inner.root, md_kind);
        let text = format!("removed entry [{idx}] from {}", path.display());
        let data = json!({
            "kind": kind,
            "index": idx,
            "path": path.display().to_string(),
        });
        Ok(ToolOutput::new(data, text))
    }

    // -- review-mode plumbing ----------------------------------------------

    async fn tool_skill_propose(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            /// Slug for the proposed skill directory under
            /// `.hoangsa-memory/skills/<slug>.draft/`.
            slug: String,
            /// The SKILL.md body the agent drafted. Must start with the
            /// `---\nname: ...` frontmatter.
            body: String,
            /// Triggers of the lessons that motivated this proposal — used
            /// only for the history log.
            #[serde(default)]
            source_triggers: Vec<String>,
        }
        let Args {
            slug,
            body,
            source_triggers,
        } = serde_json::from_value(args)?;
        let clean_slug = slug
            .trim()
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        if clean_slug.is_empty() {
            anyhow::bail!("skill slug must contain alphanumeric characters");
        }
        let draft_dir = self
            .inner
            .root
            .join("skills")
            .join(format!("{clean_slug}.draft"));
        tokio::fs::create_dir_all(&draft_dir).await?;
        tokio::fs::write(draft_dir.join("SKILL.md"), body.as_bytes()).await?;
        self.inner
            .store
            .markdown
            .append_history(&hoangsa_memory_store::markdown::HistoryEntry {
                op: "propose",
                kind: "skill",
                title: clean_slug.clone(),
                actor: Some("agent".to_string()),
                reason: if source_triggers.is_empty() {
                    None
                } else {
                    Some(format!("from lessons: {}", source_triggers.join(", ")))
                },
            })
            .await?;
        let text = format!(
            "skill proposal drafted at {} — review and run `hoangsa-memory skills install` to accept",
            draft_dir.display()
        );
        let data = json!({
            "slug": clean_slug,
            "path": draft_dir.display().to_string(),
            "source_triggers": source_triggers,
        });
        Ok(ToolOutput::new(data, text))
    }

    async fn tool_skills_list(&self) -> anyhow::Result<ToolOutput> {
        let skills = self.inner.store.markdown.list_skills().await?;
        let text = if skills.is_empty() {
            format!(
                "(no skills installed — drop a folder into {}/skills/)",
                self.inner.root.display()
            )
        } else {
            let mut buf = String::new();
            for s in &skills {
                buf.push_str(&format!("{:<28}  {}\n", s.slug, s.description));
            }
            buf
        };
        let data = serde_json::to_value(&skills).unwrap_or_else(|_| json!([]));
        Ok(ToolOutput::new(data, text))
    }

    async fn tool_memory_show(&self) -> anyhow::Result<ToolOutput> {
        let mut text = String::new();
        let mut memory_md: Option<String> = None;
        let mut lessons_md: Option<String> = None;

        for name in ["MEMORY.md", "LESSONS.md"] {
            text.push_str(&format!("─── {name} ───\n"));
            let p = self.inner.root.join(name);
            let body = match tokio::fs::read_to_string(&p).await {
                Ok(s) => Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.into()),
            };
            match &body {
                Some(s) => text.push_str(s),
                None => text.push_str("(not found)\n"),
            }
            text.push('\n');
            match name {
                "MEMORY.md" => memory_md = body,
                "LESSONS.md" => lessons_md = body,
                _ => {}
            }
        }
        let data = json!({
            "memory_md": memory_md,
            "lessons_md": lessons_md,
        });
        Ok(ToolOutput::new(data, text))
    }

    /// Compact one-line-per-entry index of MEMORY.md + LESSONS.md.
    ///
    /// Returns a scannable summary (~1 line per entry) so the LLM can
    /// quickly see what's stored and then call `memory_detail` for
    /// the full content of specific entries. This is the "L1 wake-up"
    /// layer inspired by MemPalace's layered memory stack.
    async fn tool_wakeup(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            #[serde(default)]
            scope: Option<String>,
            #[serde(default)]
            include_on_demand: Option<bool>,
        }
        let parsed = serde_json::from_value::<Args>(args).ok();
        let scope = parsed
            .as_ref()
            .and_then(|a| a.scope.clone())
            .unwrap_or_else(|| "all".to_string());
        let include_on_demand = parsed
            .as_ref()
            .and_then(|a| a.include_on_demand)
            .unwrap_or(false);

        let md = &self.inner.store.markdown;
        let mut text = String::new();
        let mut fact_count = 0usize;
        let mut on_demand_count = 0usize;
        let mut lesson_count = 0usize;

        if scope == "all" || scope == "facts" {
            let facts = md.read_facts().await?;
            let total = facts.len();
            let mut shown = Vec::new();
            for (i, f) in facts.iter().enumerate() {
                if f.scope == FactScope::OnDemand && !include_on_demand {
                    on_demand_count += 1;
                    continue;
                }
                shown.push((i, f));
            }
            fact_count = shown.len();
            if on_demand_count > 0 {
                text.push_str(&format!(
                    "=== MEMORY ({fact_count} always + {on_demand_count} on-demand, {total} total) ===\n"
                ));
            } else {
                text.push_str(&format!("=== MEMORY ({fact_count} facts) ===\n"));
            }
            for (i, f) in &shown {
                let heading = first_nonempty_line(&f.text);
                let tags = if f.tags.is_empty() {
                    String::new()
                } else {
                    format!(" | tags: {}", f.tags.join(", "))
                };
                let scope_marker = if f.scope == FactScope::OnDemand {
                    " [on-demand]"
                } else {
                    ""
                };
                text.push_str(&format!("F{:02} | {heading}{tags}{scope_marker}\n", i + 1));
            }
            text.push('\n');
        }

        if scope == "all" || scope == "lessons" {
            let lessons = md.read_lessons().await?;
            lesson_count = lessons.len();
            text.push_str(&format!("=== LESSONS ({lesson_count} lessons) ===\n"));
            for (i, l) in lessons.iter().enumerate() {
                let tier = format!("{:?}", l.enforcement);
                text.push_str(&format!(
                    "L{:02} | {} | {tier} | {}✓ {}✗\n",
                    i + 1,
                    l.trigger.trim(),
                    l.success_count,
                    l.failure_count,
                ));
            }
        }

        let data = json!({
            "facts": fact_count,
            "facts_on_demand": on_demand_count,
            "lessons": lesson_count,
        });
        Ok(ToolOutput::new(data, text))
    }

    /// Return the full content of a specific fact or lesson by index
    /// (e.g. "F03", "L01") or heading substring match.
    async fn tool_memory_detail(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            id: String,
        }
        let Args { id } = serde_json::from_value(args)?;
        let id = id.trim();

        let md = &self.inner.store.markdown;

        if let Some(Ok(idx)) = id
            .strip_prefix('F')
            .or_else(|| id.strip_prefix('f'))
            .map(|rest| rest.parse::<usize>())
        {
            let facts = md.read_facts().await?;
            if idx == 0 || idx > facts.len() {
                return Ok(ToolOutput::error(format!(
                    "F{idx} out of range (1..{})",
                    facts.len()
                )));
            }
            let f = &facts[idx - 1];
            let tags = if f.tags.is_empty() {
                String::new()
            } else {
                format!("\ntags: {}", f.tags.join(", "))
            };
            let text = format!("### F{idx:02}\n{}{tags}", f.text);
            return Ok(ToolOutput::new(json!({"kind": "fact", "index": idx}), text));
        }

        if let Some(Ok(idx)) = id
            .strip_prefix('L')
            .or_else(|| id.strip_prefix('l'))
            .map(|rest| rest.parse::<usize>())
        {
            let lessons = md.read_lessons().await?;
            if idx == 0 || idx > lessons.len() {
                return Ok(ToolOutput::error(format!(
                    "L{idx} out of range (1..{})",
                    lessons.len()
                )));
            }
            let l = &lessons[idx - 1];
            let text = format!(
                "### L{idx:02} — {}\n{}\nenforcement: {:?} | {}✓ {}✗",
                l.trigger.trim(),
                l.advice,
                l.enforcement,
                l.success_count,
                l.failure_count,
            );
            return Ok(ToolOutput::new(
                json!({"kind": "lesson", "index": idx}),
                text,
            ));
        }

        // Fallback: substring match across both facts and lessons
        let needle = id.to_lowercase();
        let facts = md.read_facts().await?;
        for (i, f) in facts.iter().enumerate() {
            if f.text.to_lowercase().contains(&needle)
                || f.tags.iter().any(|t| t.to_lowercase().contains(&needle))
            {
                let tags = if f.tags.is_empty() {
                    String::new()
                } else {
                    format!("\ntags: {}", f.tags.join(", "))
                };
                let idx = i + 1;
                let text = format!("### F{idx:02}\n{}{tags}", f.text);
                return Ok(ToolOutput::new(json!({"kind": "fact", "index": idx}), text));
            }
        }
        let lessons = md.read_lessons().await?;
        for (i, l) in lessons.iter().enumerate() {
            if l.trigger.to_lowercase().contains(&needle)
                || l.advice.to_lowercase().contains(&needle)
            {
                let idx = i + 1;
                let text = format!(
                    "### L{idx:02} — {}\n{}\nenforcement: {:?} | {}✓ {}✗",
                    l.trigger.trim(),
                    l.advice,
                    l.enforcement,
                    l.success_count,
                    l.failure_count,
                );
                return Ok(ToolOutput::new(
                    json!({"kind": "lesson", "index": idx}),
                    text,
                ));
            }
        }

        Ok(ToolOutput::error(format!("no match for \"{id}\"")))
    }

    // ---- prompts ----------------------------------------------------------

    fn prompts_list(&self) -> Value {
        let disc = CurationConfig::load_or_default_sync(&self.inner.root);
        json!({ "prompts": prompts_catalog(disc.grounding_check) })
    }

    async fn prompts_get(&self, params: Value) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        struct GetParams {
            name: String,
            #[serde(default)]
            arguments: serde_json::Map<String, Value>,
        }
        let GetParams { name, arguments } = serde_json::from_value(params)
            .map_err(|e| RpcError::new(error_codes::INVALID_PARAMS, e.to_string()))?;

        let (description, body) = match name.as_str() {
            "memory_reflect" => (
                "Reflect on the session so far and decide what to remember.",
                render_reflect_prompt(&arguments),
            ),
            "memory_nudge" => {
                // Record that the agent actually expanded the nudge prompt —
                // strict-mode gates use this to distinguish "ran a recall"
                // from "actually reflected on lessons".
                let intent = arg_str(&arguments, "intent").to_string();
                let ev = Event::NudgeInvoked {
                    id: Uuid::new_v4(),
                    intent: intent.clone(),
                    at: OffsetDateTime::now_utc(),
                };
                if let Err(e) = self.inner.store.episodes.append(&ev).await {
                    warn!(error = %e, "failed to log NudgeInvoked event");
                }
                (
                    "Nudge before a risky step: recall relevant lessons and plan.",
                    render_nudge_prompt(&arguments),
                )
            }
            "memory_grounding_check" => (
                "Verify a claim against the indexed codebase before asserting it.",
                render_grounding_prompt(&arguments),
            ),
            other => {
                return Err(RpcError::new(
                    error_codes::INVALID_PARAMS,
                    format!("unknown prompt: {other}"),
                ));
            }
        };

        let result = GetPromptResult {
            description: description.to_string(),
            messages: vec![PromptMessage {
                role: "user".to_string(),
                content: ContentBlock::text(body),
            }],
        };
        serde_json::to_value(result)
            .map_err(|e| RpcError::new(error_codes::INTERNAL_ERROR, e.to_string()))
    }

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
        let g = &self.inner.graph;
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
    async fn tool_impact(&self, args: Value) -> anyhow::Result<ToolOutput> {
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

        let hits = self.inner.graph.impact(&fqn, dir, depth).await?;

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
    async fn tool_symbol_context(&self, args: Value) -> anyhow::Result<ToolOutput> {
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
        let g = &self.inner.graph;

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

        for v in [
            &mut callers,
            &mut callees,
            &mut extends,
            &mut extended_by,
            &mut references,
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
        section("siblings", &siblings, &mut text);
        if !unresolved_imports.is_empty() {
            text.push_str("  imports (external):\n");
            for i in &unresolved_imports {
                text.push_str(&format!("    {i}\n"));
            }
        }

        Ok(ToolOutput::new(data, text))
    }

    /// Given a unified diff, return the symbols the edit touches plus
    /// their upstream blast radius (who calls / references / inherits
    /// from them). Handy as a PR pre-check: "these 7 functions need
    /// re-testing because you modified X".
    ///
    /// Input is a diff text blob (what `git diff` produces). Hunks
    /// that touch files not in the graph are silently ignored.
    async fn tool_detect_changes(&self, args: Value) -> anyhow::Result<ToolOutput> {
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
        let g = &self.inner.graph;
        let store = &self.inner.store;
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

    // ---- knowledge graph tools ---------------------------------------------
    // ---- conversation turn tools ------------------------------------------

    async fn tool_turn_save(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            session_id: String,
            role: String,
            content: String,
            /// Optional git HEAD sha captured at the moment of this turn.
            /// Lets future `memory_archive_search` answer "what was the
            /// code like when we decided X" without grepping `git log`.
            #[serde(default)]
            commit_sha: Option<String>,
            /// Optional list of file paths the caller associates with
            /// this turn (usually the files touched since the last turn).
            #[serde(default)]
            file_paths: Vec<String>,
        }
        let Args {
            session_id,
            role,
            content,
            commit_sha,
            file_paths,
        } = serde_json::from_value(args)?;

        let id = self
            .inner
            .store
            .episodes
            .append_turn(
                session_id.clone(),
                role.clone(),
                content,
                commit_sha.clone(),
                file_paths.clone(),
            )
            .await?;
        let commit_fragment = commit_sha
            .as_ref()
            .map(|s| format!(" @ {}", &s[..s.len().min(7)]))
            .unwrap_or_default();
        let files_fragment = if file_paths.is_empty() {
            String::new()
        } else {
            format!(" ({} file(s))", file_paths.len())
        };
        let text = format!(
            "saved turn #{id} ({role}) for session {session_id}{commit_fragment}{files_fragment}"
        );
        Ok(ToolOutput::new(
            json!({
                "id": id,
                "role": role,
                "commit_sha": commit_sha,
                "file_paths": file_paths,
            }),
            text,
        ))
    }

    async fn tool_turns_search(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            query: String,
            #[serde(default)]
            top_k: Option<usize>,
        }
        let Args { query, top_k } = serde_json::from_value(args)?;
        let k = top_k.unwrap_or(10);

        let hits = self.inner.store.episodes.search_turns(&query, k).await?;
        if hits.is_empty() {
            return Ok(ToolOutput::new(json!({"count": 0}), "no matching turns"));
        }

        let mut text = String::new();
        for t in &hits {
            let ts =
                t.at.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
            // `commit_sha` / `file_paths` stay in `data` unconditionally;
            // in text we only surface them when present so unenriched
            // legacy turns don't get a trailing "@ _ (0 file(s))" tag.
            let commit_tag = t
                .commit_sha
                .as_ref()
                .map(|s| format!(" @ {}", &s[..s.len().min(7)]))
                .unwrap_or_default();
            let paths_tag = if t.file_paths.is_empty() {
                String::new()
            } else {
                format!(" files={}", t.file_paths.join(","))
            };
            text.push_str(&format!(
                "[{}] {} (turn {}, session {}){}{}\n{}\n---\n",
                ts,
                t.role,
                t.turn_number,
                &t.session_id[..t.session_id.len().min(8)],
                commit_tag,
                paths_tag,
                &t.content[..t.content.len().min(500)],
            ));
        }
        let data: Vec<Value> = hits
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "session_id": t.session_id,
                    "turn_number": t.turn_number,
                    "role": t.role,
                    "commit_sha": t.commit_sha,
                    "file_paths": t.file_paths,
                })
            })
            .collect();
        Ok(ToolOutput::new(
            json!({"count": hits.len(), "turns": data}),
            text,
        ))
    }

    // ---- archive tools ---------------------------------------------------

    async fn tool_archive_status(&self) -> anyhow::Result<ToolOutput> {
        let db_path = StoreRoot::archive_path(&self.inner.root);
        let tracker = hoangsa_memory_store::ArchiveTracker::open(&db_path).await?;
        let (sessions, turns, curated) = tracker.status()?;
        let data = json!({
            "sessions": sessions,
            "turns": turns,
            "curated": curated,
        });
        let text = format!("Archive: {sessions} sessions, {turns} turns ({curated} curated)");
        Ok(ToolOutput::new(data, text))
    }

    async fn tool_archive_topics(&self, args: Value) -> anyhow::Result<ToolOutput> {
        let project = args.get("project").and_then(|v| v.as_str());
        let db_path = StoreRoot::archive_path(&self.inner.root);
        let tracker = hoangsa_memory_store::ArchiveTracker::open(&db_path).await?;
        let topics = tracker.topics(project)?;
        let arr: Vec<Value> = topics
            .iter()
            .map(|t| {
                json!({
                    "topic": t.topic,
                    "sessions": t.session_count,
                    "turns": t.total_turns,
                })
            })
            .collect();
        let text = if topics.is_empty() {
            "No topics found.".to_string()
        } else {
            topics
                .iter()
                .map(|t| {
                    format!(
                        "{}: {} sessions, {} turns",
                        t.topic, t.session_count, t.total_turns
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolOutput::new(json!(arr), text))
    }

    async fn tool_archive_search(&self, args: Value) -> anyhow::Result<ToolOutput> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let project = args.get("project").and_then(|v| v.as_str());
        let topic = args.get("topic").and_then(|v| v.as_str());

        let col = self.open_archive_chroma().await?;

        let mut filter = None;
        if project.is_some() || topic.is_some() {
            let mut conditions = Vec::new();
            if let Some(p) = project {
                conditions.push(json!({"project": {"$eq": p}}));
            }
            if let Some(t) = topic {
                conditions.push(json!({"topic": {"$eq": t}}));
            }
            filter = Some(if conditions.len() == 1 {
                conditions.into_iter().next().unwrap()
            } else {
                json!({"$and": conditions})
            });
        }

        let hits = col.query_text(query, top_k, filter).await?;
        let arr: Vec<Value> = hits
            .iter()
            .map(|h| {
                json!({
                    "id": h.id,
                    "distance": h.distance,
                    "text": h.document,
                    "metadata": h.metadata,
                })
            })
            .collect();
        let text = if hits.is_empty() {
            "No archive results.".to_string()
        } else {
            hits.iter()
                .map(|h| {
                    let topic = h
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("topic"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let preview = h
                        .document
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(200)
                        .collect::<String>();
                    format!("[{topic}] (d={:.3}) {preview}", h.distance)
                })
                .collect::<Vec<_>>()
                .join("\n---\n")
        };
        Ok(ToolOutput::new(json!(arr), text))
    }

    /// Run an archive ingest inside the daemon process so the existing
    /// ChromaDB sidecar is reused. This is the memory-pressure fix:
    /// PreCompact / SessionEnd hooks used to spawn a detached CLI which
    /// booted a fresh ~500 MB Python sidecar per invocation. Concurrent
    /// Claude Code sessions would pile those up and OOM the machine.
    /// Forwarding to this tool via the daemon socket keeps the sidecar
    /// count at one.
    async fn tool_archive_ingest(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            #[serde(default)]
            project: Option<String>,
            #[serde(default)]
            topic: Option<String>,
            #[serde(default)]
            refresh: bool,
            #[serde(default)]
            limit: Option<usize>,
        }
        let Args {
            project,
            topic,
            refresh,
            limit,
        } = serde_json::from_value(args)?;

        let tracker_path = StoreRoot::archive_path(&self.inner.root);
        let tracker = hoangsa_memory_store::ArchiveTracker::open(&tracker_path).await?;

        // Daemon-side ingest requires the already-running ChromaDB
        // sidecar. If it's not enabled we bail via ToolOutput::error so
        // the caller can fall back to spawning the CLI.
        let col = match self.get_chroma().await {
            Some(_) => self.open_archive_chroma().await?,
            None => return Ok(ToolOutput::error("ChromaDB not enabled")),
        };

        let opts = hoangsa_memory_retrieve::archive::IngestOpts {
            project_filter: project,
            topic_override: topic,
            refresh,
            limit,
        };
        let stats = hoangsa_memory_retrieve::archive::run_ingest(&tracker, &col, opts).await?;

        let text = format!(
            "Ingested {} sessions ({} chunks), skipped {} already-ingested. Retention trimmed {} session(s), cleaned {} from ChromaDB.",
            stats.total_sessions,
            stats.total_chunks,
            stats.skipped,
            stats.retention_trimmed,
            stats.retention_chroma_cleaned,
        );
        let data = json!({
            "total_sessions": stats.total_sessions,
            "total_chunks": stats.total_chunks,
            "skipped": stats.skipped,
            "retention_trimmed": stats.retention_trimmed,
            "retention_chroma_cleaned": stats.retention_chroma_cleaned,
        });
        Ok(ToolOutput::new(data, text))
    }

    async fn upsert_memory_chroma(&self, kind: &str, text: &str, tags: &[String]) {
        let col = match self.open_memory_chroma().await {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "ChromaDB memory upsert skipped (server unavailable)");
                return;
            }
        };
        let id = format!("{kind}:{}", blake3::hash(text.as_bytes()).to_hex());
        let mut meta = std::collections::HashMap::new();
        meta.insert("kind".to_string(), json!(kind));
        if !tags.is_empty() {
            meta.insert("tags".to_string(), json!(tags.join(",")));
        }
        if let Err(e) = col
            .upsert(vec![id], Some(vec![text.to_string()]), Some(vec![meta]))
            .await
        {
            tracing::debug!(error = %e, "ChromaDB memory upsert failed");
        }
    }

    async fn open_memory_chroma(&self) -> anyhow::Result<ChromaCol> {
        let cs = self
            .get_chroma()
            .await
            .ok_or_else(|| anyhow::anyhow!("ChromaDB not configured"))?;
        let (col, _info) = cs.ensure_collection("hoangsa_memory_policy").await?;
        Ok(col)
    }

    /// Code-chunk collection used by the indexer to embed source chunks.
    /// Mirrors the CLI's `open_chroma` helper so MCP-driven indexing
    /// produces embeddings instead of silently skipping the vector stage.
    async fn open_code_chroma(&self) -> Option<ChromaCol> {
        let cs = self.get_chroma().await?;
        match cs.ensure_collection("hoangsa_memory_code").await {
            Ok((col, _info)) => Some(col),
            Err(e) => {
                // Sidecar came up but the collection handshake failed — this
                // means embeddings will be skipped for this index run. Emit
                // a warning so operators can debug instead of staring at a
                // stats line that shows `embedded: 0` with no explanation.
                tracing::warn!(error = %e, "chroma: ensure_collection(hoangsa_memory_code) failed — embeddings disabled for this run");
                None
            }
        }
    }

    async fn open_archive_chroma(&self) -> anyhow::Result<ChromaCol> {
        let cs = self
            .get_chroma()
            .await
            .ok_or_else(|| anyhow::anyhow!("ChromaDB not configured"))?;
        let (col, _info) = cs.ensure_collection("hoangsa_memory_archive").await?;
        Ok(col)
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

// ===========================================================================
// Tool catalog
// ===========================================================================

fn tools_catalog() -> Vec<Tool> {
    vec![
        Tool {
            name: "memory_recall".to_string(),
            description: "Hybrid recall (symbol + BM25 + graph + markdown + semantic) over the \
                          code memory. Returns ranked chunks with path, line span, preview, and \
                          graph context (callers/callees/imports). Bodies are stripped by default \
                          — agents should `Read path:L-L` on a hit if they need the full body. \
                          Pass `detail: true` to get bodies inline. Use `scope` to include archived \
                          conversations."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language or keyword query." },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 64, "default": 8 },
                    "scope": {
                        "type": "string",
                        "enum": ["curated", "archive", "all"],
                        "default": "curated",
                        "description": "What to search: 'curated' (default) = code + facts/lessons, \
                                        'archive' = verbatim conversations only, \
                                        'all' = code + facts/lessons + archive."
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Filter facts to those with any of these tags (wing/scope filter)."
                    },
                    "min_score": {
                        "type": "number",
                        "minimum": 0.0,
                        "default": 0.0,
                        "description": "Absolute fused-score floor. Chunks below this are dropped. \
                                        An internal noise floor (~0.05) also always applies — set this \
                                        higher only to demand stronger hits."
                    },
                    "detail": {
                        "type": "boolean",
                        "default": false,
                        "description": "Return full chunk bodies inline. Default `false` — recall \
                                        returns coordinates (path + line span + preview); caller \
                                        should `Read path:L-L` for full content. Set `true` when \
                                        you need bodies in one round trip."
                    },
                    "log_event": {
                        "type": "boolean",
                        "default": true,
                        "description": "Whether to persist this call as a `query_issued` event in \
                                        episodes.db. Agent-initiated recalls (default true) MUST log \
                                        — that's how `hoangsa-cli enforce` proves the agent consulted memory \
                                        before mutating. Automated hooks that auto-recall for context \
                                        injection (e.g. UserPromptSubmit) pass `false` so their \
                                        ceremonial recall doesn't satisfy the gate on the agent's behalf."
                    }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "memory_index".to_string(),
            description: "Walk a source tree, parse every supported file, and populate the \
                          indexes (symbols, call graph, BM25, chunks)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Source path. Defaults to '.'." }
                }
            }),
        },
        Tool {
            name: "memory_remember_fact".to_string(),
            description: "Append a semantic fact to MEMORY.md. Use this when you learn \
                          something about the codebase that should survive across sessions."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The fact itself. First line becomes the heading." },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for later filtering."
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["always", "on-demand"],
                        "default": "always",
                        "description": "always = injected every session start; on-demand = only surfaced via memory_recall."
                    }
                },
                "required": ["text"]
            }),
        },
        Tool {
            name: "memory_remember_lesson".to_string(),
            description: "Append a reflective lesson to LESSONS.md. Use this after a mistake \
                          or surprise so future sessions can avoid the trap. `trigger` may be \
                          a plain string (legacy) or a structured object with optional \
                          `tool` / `path_glob` / `cmd_regex` / `content_regex` matchers plus \
                          a required `natural` description. `suggested_enforcement` is audit- \
                          only — the lesson is always saved at `Advise` tier; promotion is \
                          evidence-driven by the outcome harvester (REQ-03)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "trigger": {
                        "oneOf": [
                            {
                                "type": "string",
                                "description": "Legacy natural-language trigger."
                            },
                            {
                                "type": "object",
                                "properties": {
                                    "tool":           { "type": "string", "description": "Tool name filter: Edit/Write/Bash/etc." },
                                    "path_glob":      { "type": "string", "description": "Glob for Edit/Write/Read path." },
                                    "cmd_regex":      { "type": "string", "description": "Regex for Bash command strings." },
                                    "content_regex":  { "type": "string", "description": "Regex for Edit old_string/new_string." },
                                    "natural":        { "type": "string", "description": "Human-readable trigger description." }
                                },
                                "required": ["natural"]
                            }
                        ]
                    },
                    "advice":  { "type": "string", "description": "The lesson / rule itself." },
                    "suggested_enforcement": {
                        "type": "string",
                        "enum": ["Advise", "Require", "Block", "WorkflowGate"],
                        "description": "Tier the proposer suggests. Audit only — stored lesson enforcement starts at Advise."
                    },
                    "block_message": {
                        "type": "string",
                        "description": "Message shown via stderr when this lesson blocks a tool call (used once promoted to Block)."
                    },
                    "stage": {
                        "type": "boolean",
                        "default": false,
                        "description": "Force staging to LESSONS.pending.md even in auto-commit mode."
                    }
                },
                "required": ["trigger", "advice"]
            }),
        },
        Tool {
            name: "memory_remember_preference".to_string(),
            description: "Append a user preference to USER.md. Returns a structured \
                          `cap_exceeded` / `content_policy` error (isError=true) when the \
                          write would exceed `[memory].cap_user_bytes` or the content policy \
                          rejects the payload."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The preference itself. First line becomes the heading." },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags for later filtering."
                    }
                },
                "required": ["text"]
            }),
        },
        Tool {
            name: "memory_replace".to_string(),
            description: "Replace one entry in MEMORY.md / LESSONS.md / USER.md identified by \
                          a substring match. Use this to update an existing fact / lesson / \
                          preference instead of appending a near-duplicate (REQ-04)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind":     { "type": "string", "enum": ["fact", "lesson", "preference"] },
                    "query":    { "type": "string", "description": "Substring identifying the entry to replace." },
                    "new_text": { "type": "string", "description": "Replacement entry body." }
                },
                "required": ["kind", "query", "new_text"]
            }),
        },
        Tool {
            name: "memory_remove".to_string(),
            description: "Remove one entry from MEMORY.md / LESSONS.md / USER.md identified by \
                          a substring match. Use this to prune obsolete entries after a cap \
                          hit (REQ-05)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "kind":  { "type": "string", "enum": ["fact", "lesson", "preference"] },
                    "query": { "type": "string", "description": "Substring identifying the entry to remove." }
                },
                "required": ["kind", "query"]
            }),
        },
        Tool {
            name: "memory_skills_list".to_string(),
            description: "List every installed skill under .hoangsa-memory/skills/.".to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "memory_show".to_string(),
            description: "Return the current MEMORY.md and LESSONS.md as plain text. \
                          For large memory sets, prefer memory_wakeup (compact index) + \
                          memory_detail (drill into specific entries)."
                .to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "memory_wakeup".to_string(),
            description: "Compact one-line-per-entry index of facts and lessons. \
                          By default only shows `always`-scope facts (core context). \
                          Pass `include_on_demand: true` to also show on-demand facts. \
                          Use at session start for a cheap overview, then call \
                          memory_detail for specific entries."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "scope": {
                        "type": "string",
                        "enum": ["all", "facts", "lessons"],
                        "default": "all",
                        "description": "Which memory surface to index."
                    },
                    "include_on_demand": {
                        "type": "boolean",
                        "default": false,
                        "description": "When true, also include on-demand facts (normally only surfaced via memory_recall)."
                    }
                }
            }),
        },
        Tool {
            name: "memory_detail".to_string(),
            description: "Return the full content of a specific fact or lesson. \
                          Pass an index from memory_wakeup (e.g. 'F03', 'L01') or \
                          a heading substring to match."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "Entry index (e.g. 'F03', 'L01') or heading substring."
                    }
                },
                "required": ["id"]
            }),
        },
        Tool {
            name: "memory_impact".to_string(),
            description: "Blast-radius analysis over the code graph. Given a symbol FQN, \
                          returns every reachable symbol grouped by distance. Use \
                          `direction=\"up\"` (default) to answer \"what breaks if I change \
                          this?\" (callers / references / subtypes); `\"down\"` for \
                          \"what does this depend on?\" (callees / parent types)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "fqn": { "type": "string", "description": "Fully qualified name (module::symbol)." },
                    "direction": {
                        "type": "string",
                        "enum": ["up", "down", "both"],
                        "default": "up"
                    },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 8,
                        "default": 3
                    }
                },
                "required": ["fqn"]
            }),
        },
        Tool {
            name: "memory_symbol_context".to_string(),
            description: "360-degree view of a single symbol: callers, callees, parent types, \
                          subtypes, references, siblings, and unresolved imports. Use this \
                          when you already know the FQN of a symbol and want structured context \
                          around it (post-`memory_recall` drill-down)."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "fqn": { "type": "string", "description": "Fully qualified name (module::symbol)." },
                    "limit": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 128,
                        "default": 32,
                        "description": "Per-section cap on the returned neighbours."
                    }
                },
                "required": ["fqn"]
            }),
        },
        Tool {
            name: "memory_detect_changes".to_string(),
            description: "Parse a unified diff (e.g. `git diff`), find every indexed symbol \
                          whose declaration span overlaps a changed hunk, and return their \
                          upstream blast radius. Ideal as a PR pre-check — answers \"which \
                          code is downstream of my edit and should be re-tested?\"."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "diff":  { "type": "string", "description": "Unified diff text (`git diff` output)." },
                    "depth": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": 6,
                        "default": 2,
                        "description": "Blast-radius depth (BFS levels of callers / references / subtypes)."
                    }
                },
                "required": ["diff"]
            }),
        },
        Tool {
            name: "memory_skill_propose".to_string(),
            description: "Draft a new SKILL.md under .hoangsa-memory/skills/<slug>.draft/ — used when \
                          you've noticed ≥5 related lessons and want to consolidate them into \
                          a reusable skill. The user promotes via `hoangsa-memory skills install`."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "slug":            { "type": "string", "description": "kebab-case slug for the draft directory." },
                    "body":            { "type": "string", "description": "Full SKILL.md body starting with `---` frontmatter." },
                    "source_triggers": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Triggers of the lessons this skill consolidates."
                    }
                },
                "required": ["slug", "body"]
            }),
        },
        // ---- conversation turn tools ----
        Tool {
            name: "memory_turn_save".to_string(),
            description: "Save a verbatim conversation turn (user or assistant) to the \
                          episodic log. Called automatically by hooks or manually by the \
                          agent to preserve important exchanges. Optional `commit_sha` + \
                          `file_paths` let `memory_archive_search` link a turn back to the \
                          code state / files changed at that moment."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "session_id":  { "type": "string", "description": "Session identifier." },
                    "role":        { "type": "string", "enum": ["user", "assistant"] },
                    "content":     { "type": "string", "description": "Verbatim turn content." },
                    "commit_sha":  {
                        "type": "string",
                        "description": "Optional git HEAD sha at the time of the turn (full or short)."
                    },
                    "file_paths":  {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional file paths touched around this turn."
                    }
                },
                "required": ["session_id", "role", "content"]
            }),
        },
        Tool {
            name: "memory_turns_search".to_string(),
            description: "Full-text search over saved conversation turns. Returns matching \
                          turns with session context."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Search query (FTS5 MATCH)." },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 }
                },
                "required": ["query"]
            }),
        },
        // ---- archive tools ----
        Tool {
            name: "memory_archive_status".to_string(),
            description: "Archive summary: total sessions, turns, and curated count. \
                          ~100 tokens. Good for L0 orientation."
                .to_string(),
            input_schema: json!({ "type": "object", "properties": {} }),
        },
        Tool {
            name: "memory_archive_topics".to_string(),
            description: "List topics in the conversation archive with session and turn counts. \
                          Optionally filter by project."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Filter by project name." }
                }
            }),
        },
        Tool {
            name: "memory_archive_search".to_string(),
            description: "Semantic search across archived verbatim conversations stored in \
                          ChromaDB. Returns the most relevant conversation turns. Use this to \
                          find past discussions, decisions, and context."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural language search query." },
                    "top_k": { "type": "integer", "minimum": 1, "maximum": 50, "default": 10 },
                    "project": { "type": "string", "description": "Filter by project name." },
                    "topic": { "type": "string", "description": "Filter by topic." }
                },
                "required": ["query"]
            }),
        },
        Tool {
            name: "memory_archive_ingest".to_string(),
            description: "Ingest Claude Code conversation sessions into the archive via the \
                          daemon, reusing the running ChromaDB sidecar. Invoked by hook \
                          forwarding (PreCompact / SessionEnd) so concurrent Claude Code \
                          sessions don't each spawn their own ~500 MB Python sidecar."
                .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "project": { "type": "string", "description": "Only ingest sessions from this project." },
                    "topic":   { "type": "string", "description": "Override auto-detected topic for all ingested sessions." },
                    "refresh": { "type": "boolean", "description": "Re-ingest already-seen sessions (pick up new turns).", "default": false },
                    "limit":   { "type": "integer", "minimum": 0, "description": "Cap ingest at N most recent session files. 0 disables the implicit first-run cap." }
                }
            }),
        },
    ]
}

// ===========================================================================
// Prompts catalog
// ===========================================================================

/// Descriptors advertised by `prompts/list`. Each maps to a renderer in
/// [`Server::prompts_get`]; rendering is pure string substitution so the
/// server stays deterministic and dependency-free.
fn prompts_catalog(grounding_enabled: bool) -> Vec<Prompt> {
    let mut prompts = vec![
        Prompt {
            name: "memory_reflect".to_string(),
            description:
                "End-of-step self-reflection: decide whether to save a lesson or fact based \
                 on what just happened."
                    .to_string(),
            arguments: vec![
                PromptArgument {
                    name: "summary".to_string(),
                    description: "One-paragraph summary of what the agent just did.".to_string(),
                    required: true,
                },
                PromptArgument {
                    name: "outcome".to_string(),
                    description: "What went right or wrong (tests, user feedback, etc.)."
                        .to_string(),
                    required: false,
                },
            ],
        },
        Prompt {
            name: "memory_nudge".to_string(),
            description:
                "Pre-action nudge: surface the most relevant lessons and force the agent to \
                 acknowledge them before proceeding."
                    .to_string(),
            arguments: vec![PromptArgument {
                name: "intent".to_string(),
                description: "What the agent is about to do.".to_string(),
                required: true,
            }],
        },
    ];
    if grounding_enabled {
        prompts.push(Prompt {
            name: "memory_grounding_check".to_string(),
            description: "Ask the agent to verify a factual claim against the indexed code before \
                 asserting it to the user."
                .to_string(),
            arguments: vec![PromptArgument {
                name: "claim".to_string(),
                description: "The claim to verify.".to_string(),
                required: true,
            }],
        });
    }
    prompts
}

fn arg_str<'a>(args: &'a serde_json::Map<String, Value>, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

fn render_reflect_prompt(args: &serde_json::Map<String, Value>) -> String {
    let summary = arg_str(args, "summary");
    let outcome = arg_str(args, "outcome");
    format!(
        "You just finished a step. Reflect on it before moving on.\n\
         \n\
         ## What you did\n\
         {summary}\n\
         \n\
         ## Outcome observed\n\
         {outcome}\n\
         \n\
         ## Decide\n\
         1. Is there a durable FACT worth saving about this codebase?\n\
            If yes, call `memory_remember_fact` with a one-line summary.\n\
         2. Is there a LESSON — a non-obvious pattern a future session would miss?\n\
            If yes, call `memory_remember_lesson` with a crisp `trigger` and `advice`.\n\
         3. If neither, reply `no memory needed` and continue.\n\
         \n\
         Be conservative: only save memory that is useful, specific, and not \
         already obvious from the code itself.",
        summary = if summary.is_empty() {
            "(not provided)"
        } else {
            summary
        },
        outcome = if outcome.is_empty() {
            "(not provided)"
        } else {
            outcome
        },
    )
}

fn render_nudge_prompt(args: &serde_json::Map<String, Value>) -> String {
    let intent = arg_str(args, "intent");
    format!(
        "Before you act, recall what past sessions learned.\n\
         \n\
         ## Intended action\n\
         {intent}\n\
         \n\
         ## Required checks\n\
         1. Call `memory_recall` with a short query derived from the intent above.\n\
         2. Read LESSONS.md via `resources/read hoangsa-memory://memory/LESSONS.md` and pick \
            every lesson whose `trigger` plausibly applies.\n\
         3. Restate the plan in one paragraph, naming each lesson you're honouring.\n\
         4. Only then execute. If a lesson advises against the plan, STOP and ask \
            the user before proceeding.",
        intent = if intent.is_empty() {
            "(not provided)"
        } else {
            intent
        },
    )
}

fn render_grounding_prompt(args: &serde_json::Map<String, Value>) -> String {
    let claim = arg_str(args, "claim");
    format!(
        "Verify the following claim against the indexed codebase BEFORE asserting it.\n\
         \n\
         ## Claim\n\
         {claim}\n\
         \n\
         ## Procedure\n\
         1. Call `memory_recall` with the most load-bearing nouns from the claim.\n\
         2. Read the returned chunks and decide: supported, contradicted, or \
            insufficient evidence.\n\
         3. If supported, cite at least one chunk id when you answer the user.\n\
         4. If contradicted or insufficient, say so honestly — do not hedge.",
        claim = if claim.is_empty() {
            "(not provided)"
        } else {
            claim
        },
    )
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

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("")
        .chars()
        .take(120)
        .collect()
}

/// Parse the MCP-level `kind` string ("fact" / "lesson" / "preference") into
/// the hoangsa-memory-policy `MemoryKind` enum used by the three-surface markdown API
/// (DESIGN-SPEC REQ-04/05/06).
fn parse_md_kind(kind: &str) -> anyhow::Result<MdKind> {
    match kind {
        "fact" => Ok(MdKind::Fact),
        "lesson" => Ok(MdKind::Lesson),
        "preference" => Ok(MdKind::Preference),
        other => anyhow::bail!(
            "unknown memory kind: {other} (expected `fact`, `lesson`, or `preference`)"
        ),
    }
}

/// Project a [`MdKind`] onto the on-disk markdown file for user-facing
/// status messages.
fn md_kind_path(root: &Path, kind: MdKind) -> PathBuf {
    match kind {
        MdKind::Fact => root.join("MEMORY.md"),
        MdKind::Lesson => root.join("LESSONS.md"),
        MdKind::Preference => root.join("USER.md"),
    }
}

/// Serialize a [`GuardedAppendError`] as a structured MCP tool error so the
/// client can key off `data.code` = `"cap_exceeded"` / `"content_policy"`
/// and use the attached `preview` entries to pick a `memory_replace`
/// or `memory_remove` target. DESIGN-SPEC REQ-03 / REQ-12.
fn guarded_error_output(err: GuardedAppendError) -> ToolOutput {
    match err {
        GuardedAppendError::CapExceeded(e) => cap_error_output(e),
        GuardedAppendError::ContentPolicy(e) => {
            let data = json!({
                "code": "content_policy",
                "kind": e.kind,
                "reason": e.reason,
                "offending_first_line": e.offending_first_line,
                "hint": e.hint,
            });
            let text = serde_json::to_string(&data).unwrap_or_else(|_| {
                format!(
                    "content policy rejected ({}): {}",
                    e.reason, e.offending_first_line
                )
            });
            ToolOutput {
                data,
                text,
                is_error: true,
            }
        }
    }
}

fn cap_error_output(e: CapExceededError) -> ToolOutput {
    let preview = serde_json::to_value(&e.entries).unwrap_or_else(|_| json!([]));
    let data = json!({
        "code": "cap_exceeded",
        "kind": e.kind,
        "current_bytes": e.current_bytes,
        "cap_bytes": e.cap_bytes,
        "attempted_bytes": e.attempted_bytes,
        "preview": preview,
        "hint": e.hint,
    });
    // Serialize the structured payload into the text block too so plain MCP
    // clients (which only see `content[0].text`) can still parse it as JSON
    // and make the next replace/remove decision.
    let text = serde_json::to_string(&data).unwrap_or_else(|_| {
        format!(
            "cap exceeded: {:?} would reach {} / {} bytes",
            e.kind, e.attempted_bytes, e.cap_bytes
        )
    });
    ToolOutput {
        data,
        text,
        is_error: true,
    }
}

// ===========================================================================
// Background file watcher
// ===========================================================================

/// Watch `src` for file changes and reindex through `inner.indexer`.
///
/// Mirrors the debounce + batch logic in `cmd_watch` but runs in-process
/// alongside the MCP daemon, sharing the same `Indexer` (and therefore the
/// same redb write lock). This avoids the "daemon is running" conflict
/// that blocks the standalone `hoangsa-memory watch`.
async fn run_watcher(
    inner: Arc<Inner>,
    src: PathBuf,
    debounce: std::time::Duration,
) -> anyhow::Result<()> {
    use hoangsa_memory_parse::watch::Watcher;

    let mut w = Watcher::watch(&src, 1024)?;
    debug!(path = %src.display(), "background watcher started");

    loop {
        let Some(ev) = w.recv().await else {
            debug!("watcher channel closed");
            break;
        };

        // Debounce: drain events arriving within the window.
        let mut batch = vec![ev];
        let deadline = tokio::time::Instant::now() + debounce;
        while let Ok(Some(extra)) = tokio::time::timeout_at(deadline, w.recv()).await {
            batch.push(extra);
        }

        let mut changed = std::collections::HashSet::new();
        let mut deleted = std::collections::HashSet::new();
        for ev in batch {
            match ev {
                hoangsa_memory_core::Event::FileChanged { path, .. } => {
                    deleted.remove(&path);
                    changed.insert(path);
                }
                hoangsa_memory_core::Event::FileDeleted { path, .. } => {
                    changed.remove(&path);
                    deleted.insert(path);
                }
                _ => {}
            }
        }

        let changed_n = changed.len();
        let deleted_n = deleted.len();

        for path in deleted {
            if let Err(e) = inner.indexer.purge_path(&path).await {
                warn!(?path, error = %e, "watcher: purge failed");
            }
        }
        for path in changed {
            if let Err(e) = inner.indexer.index_file(&path).await {
                warn!(?path, error = %e, "watcher: re-index failed");
            }
        }

        if changed_n + deleted_n > 0 {
            if let Err(e) = inner.indexer.commit().await {
                warn!(error = %e, "watcher: fts commit failed");
            }
            debug!(
                changed = changed_n,
                deleted = deleted_n,
                "watcher: reindexed"
            );
        }
    }
    Ok(())
}

// ===========================================================================
// Stdio transport
// ===========================================================================

/// Run the server on stdin/stdout until EOF or ctrl-c.
///
/// Each JSON-RPC message is expected on its own line. Responses are emitted
/// as newline-terminated JSON on stdout; all logging goes to stderr via
/// `tracing`.
pub async fn run_stdio(server: Server) -> anyhow::Result<()> {
    let mut reader = BufReader::new(tokio::io::stdin());
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = tokio::select! {
            res = reader.read_line(&mut line) => res?,
            _ = tokio::signal::ctrl_c() => {
                debug!("ctrl-c; shutting down mcp");
                0
            }
        };
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RpcIncoming>(trimmed) {
            Ok(msg) => server.handle(msg).await,
            Err(e) => Some(RpcResponse::err(
                Value::Null,
                RpcError::new(error_codes::PARSE_ERROR, format!("parse error: {e}")),
            )),
        };

        if let Some(resp) = response {
            let text = serde_json::to_string(&resp)?;
            stdout.write_all(text.as_bytes()).await?;
            stdout.write_all(b"\n").await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

/// Canonical path for the Unix domain socket that the CLI connects to.
pub fn socket_path(root: &Path) -> std::path::PathBuf {
    root.join("mcp.sock")
}

/// Run a Unix-socket sidecar alongside the stdio transport.
///
/// Binds `.hoangsa-memory/mcp.sock` and accepts connections in a loop. Each
/// connection is a short-lived JSON-RPC session (one line in → one line
/// out, then close). The socket is removed on clean shutdown.
///
/// This is the "thin-client" entry point: when the CLI detects the socket
/// it forwards requests here instead of opening the store directly,
/// avoiding the redb exclusive-lock conflict.
pub async fn run_socket(server: Server) -> anyhow::Result<()> {
    use tokio::net::{UnixListener, UnixStream};

    let sock = socket_path(&server.inner.root);

    // Try binding first. Only if it fails with `AddrInUse` do we probe
    // the existing socket and, if nothing is listening, unlink and retry.
    // This avoids the race where two daemons start at the same time, and
    // the "remove stale and rebind" pattern of the previous version would
    // happily overwrite an actively-used socket.
    let listener = match UnixListener::bind(&sock) {
        Ok(l) => l,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            // Peer responsive? Then another daemon owns the socket — bail.
            if UnixStream::connect(&sock).await.is_ok() {
                return Err(anyhow::anyhow!(
                    "another hoangsa-memory-mcp is already listening on {}",
                    sock.display()
                ));
            }
            // Stale socket file — safe to remove and retry.
            let _ = std::fs::remove_file(&sock);
            UnixListener::bind(&sock)?
        }
        Err(e) => return Err(e.into()),
    };
    debug!(path = %sock.display(), "mcp socket listening");

    loop {
        let (stream, _) = listener.accept().await?;
        let server = server.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_socket_conn(server, stream).await {
                debug!(error = %e, "socket connection error");
            }
        });
    }
}

/// Handle one Unix-socket connection: read lines, dispatch, respond.
async fn handle_socket_conn(server: Server, stream: tokio::net::UnixStream) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<RpcIncoming>(trimmed) {
            Ok(msg) => server.handle(msg).await,
            Err(e) => Some(RpcResponse::err(
                Value::Null,
                RpcError::new(error_codes::PARSE_ERROR, format!("parse error: {e}")),
            )),
        };

        if let Some(resp) = response {
            let text = serde_json::to_string(&resp)?;
            writer.write_all(text.as_bytes()).await?;
            writer.write_all(b"\n").await?;
            writer.flush().await?;
        }
    }
    Ok(())
}

// ===========================================================================
// Enforcement tool tests (T-14)
// ===========================================================================

#[cfg(test)]
mod enforcement_tools {
    //! Covers REQ-03 (structured trigger + suggested audit-only), plus the
    //! override + workflow MCP surfaces introduced for the enforcement layer.
    use super::*;
    use serde_json::json;
    use tempfile::TempDir;

    async fn fresh_server() -> (TempDir, Server) {
        let td = TempDir::new().expect("tempdir");
        let srv = Server::open(td.path())
            .await
            .expect("Server::open on fresh tempdir");
        (td, srv)
    }

    fn call(name: &str, args: Value) -> Value {
        json!({ "name": name, "arguments": args })
    }

    async fn dispatch(srv: &Server, name: &str, args: Value) -> ToolOutput {
        srv.dispatch_tool(call(name, args))
            .await
            .expect("dispatch_tool")
    }

    // -- remember_lesson ----------------------------------------------------

    #[tokio::test]
    async fn remember_lesson_accepts_structured_trigger_roundtrip() {
        let (_td, srv) = fresh_server().await;
        let out = dispatch(
            &srv,
            "memory_remember_lesson",
            json!({
                "trigger": {
                    "tool": "Bash",
                    "cmd_regex": "^rm\\s+-rf\\s+/",
                    "natural": "don't nuke the root"
                },
                "advice": "always dry-run destructive bash commands",
                "suggested_enforcement": "Block",
                "block_message": "rm -rf / is never the answer"
            }),
        )
        .await;

        assert!(!out.is_error, "tool call must succeed, got: {}", out.text);
        let st = &out.data["structured_trigger"];
        assert_eq!(st["tool"], "Bash");
        assert_eq!(st["cmd_regex"], "^rm\\s+-rf\\s+/");
        assert_eq!(st["natural"], "don't nuke the root");
    }

    #[tokio::test]
    async fn remember_lesson_suggested_ignored_saved_as_advise() {
        // REQ-03: even when the proposer suggests `Block`, the stored lesson
        // must come out at `Advise`.
        let (_td, srv) = fresh_server().await;
        let out = dispatch(
            &srv,
            "memory_remember_lesson",
            json!({
                "trigger": { "natural": "skip tests on main" },
                "advice": "never push without running tests",
                "suggested_enforcement": "Block"
            }),
        )
        .await;
        assert!(!out.is_error, "tool call must succeed, got: {}", out.text);
        assert_eq!(out.data["enforcement"], json!("Advise"));
        assert_eq!(out.data["suggested_enforcement"], json!("Block"));
    }

    #[tokio::test]
    async fn remember_lesson_legacy_string_trigger_still_works() {
        let (_td, srv) = fresh_server().await;
        let out = dispatch(
            &srv,
            "memory_remember_lesson",
            json!({
                "trigger": "plain legacy trigger",
                "advice": "still gets stored"
            }),
        )
        .await;
        assert!(!out.is_error, "legacy path failed: {}", out.text);
        assert_eq!(out.data["trigger"], "plain legacy trigger");
        assert_eq!(
            out.data["structured_trigger"]["natural"],
            "plain legacy trigger"
        );
        assert_eq!(out.data["enforcement"], json!("Advise"));
    }

    // -- catalog wiring -----------------------------------------------------

    #[test]
    fn tools_catalog_advertises_enforcement_surface() {
        let names: Vec<String> = tools_catalog().into_iter().map(|t| t.name).collect();
        {
            let needed = "memory_remember_lesson";
            assert!(
                names.iter().any(|n| n == needed),
                "tools catalog missing `{needed}`; have {names:?}"
            );
        }
    }
}
