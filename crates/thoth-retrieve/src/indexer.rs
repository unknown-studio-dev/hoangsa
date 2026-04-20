//! Full-project indexer.
//!
//! Walks a source tree, parses every recognised file with `thoth-parse`, and
//! writes the results into every backend of a [`StoreRoot`]:
//!
//! - `fts`      — one BM25 document per [`SourceChunk`].
//! - `kv`       — symbol rows keyed by FQN (for exact lookup).
//! - `graph`    — nodes per symbol, edges for calls + imports.
//!
//! Per-file work (parse + FTS/KV/graph writes) fans out across a bounded
//! pool of concurrent tasks; the underlying stores are already behind their
//! own mutexes, so writes serialize there naturally. Embedding is deferred
//! until the whole tree is walked so we can ship chunks to the provider in
//! large batches instead of one HTTP round-trip per file.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures::stream::{self, StreamExt};
use parking_lot::Mutex;
use thoth_core::Result;
use thoth_graph::{Edge, EdgeKind, Graph, Node};
use thoth_parse::{
    LanguageRegistry, SourceChunk, SymbolKind,
    walk::{WalkOptions, walk_sources},
};
use thoth_store::{ChromaCol, ChunkDoc, StoreRoot, SymbolRow};
use tracing::debug;

/// How many chunks to embed in one `embed_batch` call. Each provider adapter
/// already chunks this down to its own HTTP cap (Voyage 128, OpenAI 2048,
/// Cohere 96), so 256 is a comfortable upper bound that keeps the progress
/// bar moving without blowing out memory.
const EMBED_BATCH_SIZE: usize = 256;

/// Stats returned from one full [`Indexer::index_path`] run.
///
/// Every counter is a **delta** for this run — not an index-wide total.
/// `files` is the count of files the walker emitted; `files_skipped` is
/// the subset of those whose content hash matched the last-indexed
/// blake3 sentinel so the file was short-circuited with zero work.
/// `chunks`, `symbols`, `calls`, `imports`, and `embedded` therefore
/// reflect only the newly-parsed or reparsed files, which is why a
/// steady-state reindex can report e.g. `files=23 files_skipped=23
/// chunks=0 symbols=0` — expected, not a bug.
#[derive(Debug, Default, Clone, Copy)]
pub struct IndexStats {
    /// Files the walker yielded (parsed + cache-hit).
    pub files: usize,
    /// Subset of `files` where content hash matched the prior index
    /// sentinel so no FTS/KV/graph writes happened.
    pub files_skipped: usize,
    /// Chunks written to the BM25 index during this run.
    pub chunks: usize,
    /// Symbols written to the KV + graph during this run.
    pub symbols: usize,
    /// Call edges inserted during this run.
    pub calls: usize,
    /// Import edges inserted during this run.
    pub imports: usize,
    /// Chunks embedded into ChromaDB during this run. `0` unless
    /// ChromaDB is configured.
    pub embedded: usize,
}

/// Progress event fired during [`Indexer::index_path`].
///
/// The indexer walks a tree in four stages and emits one event per stage
/// transition (and per unit of progress within each stage):
///
/// | Stage      | `done` / `total` counted in | Emitted              |
/// |------------|-----------------------------|----------------------|
/// | `"walk"`   | files                       | once, at start       |
/// | `"file"`   | files                       | after each file      |
/// | `"embed"`  | chunks                      | once at 0, then per batch |
/// | `"commit"` | files                       | once, before flushing FTS |
///
/// `path` is populated for `"file"` events only.
#[derive(Debug, Clone, Copy)]
pub struct IndexProgress<'a> {
    /// Current pipeline stage (see table above).
    pub stage: &'static str,
    /// Units processed so far.
    pub done: usize,
    /// Total units in this stage.
    pub total: usize,
    /// File path for the `"file"` stage.
    pub path: Option<&'a Path>,
}

/// Dynamic progress callback. Stored inside [`Indexer`] when
/// [`Indexer::with_progress`] is called.
type ProgressFn = Arc<dyn for<'a> Fn(IndexProgress<'a>) + Send + Sync>;

/// Project indexer.
#[derive(Clone)]
pub struct Indexer {
    store: StoreRoot,
    graph: Graph,
    registry: LanguageRegistry,
    /// Optional ChromaDB collection for semantic search.
    chroma: Option<Arc<ChromaCol>>,
    /// Optional per-file progress callback.
    on_progress: Option<ProgressFn>,
    /// Max concurrent per-file pipelines during [`Indexer::index_path`].
    concurrency: usize,
    /// Walker options: ignore patterns, max file size, hidden-dir toggle,
    /// symlink handling. Typically sourced from `config.toml`'s
    /// `[index]` table via [`Indexer::with_config`].
    walk_opts: WalkOptions,
}

impl Indexer {
    /// Build a new indexer over the given store + language registry.
    pub fn new(store: StoreRoot, registry: LanguageRegistry) -> Self {
        let graph = Graph::new(store.kv.clone());
        Self {
            store,
            graph,
            registry,
            chroma: None,
            on_progress: None,
            concurrency: default_concurrency(),
            walk_opts: WalkOptions::default(),
        }
    }

    /// Attach extra ignore patterns (gitignore syntax) that will be applied
    /// during [`Indexer::index_path`] on top of `.gitignore`, `.ignore`, and
    /// `.thothignore`. Malformed patterns are logged and skipped.
    ///
    /// Typical source: `config.toml`'s `[index] ignore = [...]`.
    pub fn with_ignore_patterns<I, S>(mut self, patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.walk_opts.extra_ignore_patterns = patterns.into_iter().map(Into::into).collect();
        self
    }

    /// Replace the [`WalkOptions`] wholesale. Useful for callers that want
    /// to tweak `max_file_size` / `include_hidden` / `follow_symlinks`
    /// programmatically without round-tripping through `config.toml`.
    pub fn with_walk_options(mut self, opts: WalkOptions) -> Self {
        self.walk_opts = opts;
        self
    }

    /// Apply a user-facing [`IndexConfig`] (typically loaded from
    /// `config.toml`) to this indexer. Sets the ignore list, max file size,
    /// hidden-dir toggle, and symlink handling in one call.
    ///
    /// This is the "one-stop wire" for apps: load once, pass here, done.
    pub fn with_config(mut self, cfg: &crate::IndexConfig) -> Self {
        self.walk_opts = WalkOptions {
            max_file_size: cfg.max_file_size,
            follow_symlinks: cfg.follow_symlinks,
            include_hidden: cfg.include_hidden,
            extra_ignore_patterns: cfg.ignore.clone(),
        };
        self
    }

    /// Override the per-file concurrency cap used by [`Indexer::index_path`].
    /// Passing `0` falls back to the default (≈ CPU count, capped at 16).
    pub fn with_concurrency(mut self, n: usize) -> Self {
        self.concurrency = if n == 0 { default_concurrency() } else { n };
        self
    }

    /// Attach a ChromaDB collection for semantic indexing.
    pub fn with_chroma(mut self, col: Arc<ChromaCol>) -> Self {
        self.chroma = Some(col);
        self
    }
    /// Register a progress callback fired once per file during
    /// [`Indexer::index_path`] (plus one `stage = "walk"` at the start and
    /// one `stage = "commit"` at the end).
    pub fn with_progress<F>(mut self, cb: F) -> Self
    where
        F: for<'a> Fn(IndexProgress<'a>) + Send + Sync + 'static,
    {
        self.on_progress = Some(Arc::new(cb));
        self
    }

    fn emit(&self, ev: IndexProgress<'_>) {
        if let Some(cb) = &self.on_progress {
            cb(ev);
        }
    }

    /// Index every eligible file under `root`.
    ///
    /// Pipeline:
    /// 1. Walk the source tree (synchronous; fast).
    /// 2. Fan out per-file parse + FTS/KV/graph writes over a bounded pool
    ///    of concurrent tasks. Chunks are buffered for later embedding.
    /// 3. If Mode::Full is configured, batch-embed every collected chunk in
    ///    rounds of [`EMBED_BATCH_SIZE`] — one `embed_batch` call per round
    ///    instead of one per file.
    /// 4. Commit the BM25 writer so fresh docs become searchable.
    pub async fn index_path(&self, root: impl AsRef<Path>) -> Result<IndexStats> {
        let root = root.as_ref().to_path_buf();
        let files = walk_sources(&root, &self.registry, &self.walk_opts);
        let total = files.len();
        debug!(
            count = total,
            ?root,
            concurrency = self.concurrency,
            "indexing"
        );
        self.emit(IndexProgress {
            stage: "walk",
            done: 0,
            total,
            path: None,
        });

        // Phase A: fan-out parse + writes.
        // Each task returns its own Vec<SourceChunk> to avoid Mutex contention
        // on a shared accumulator. Results are collected via join_all and
        // flattened after all tasks complete.
        let stats = Arc::new(Mutex::new(IndexStats::default()));
        let done = Arc::new(AtomicUsize::new(0));
        let want_embed = self.chroma.is_some();

        let tasks: Vec<_> = stream::iter(files)
            .map(|path| {
                let this = self.clone();
                let stats = stats.clone();
                let done = done.clone();
                async move {
                    let mut local_chunks: Vec<SourceChunk> = Vec::new();
                    match this.index_file_no_embed(&path).await {
                        Ok((s, chunks)) => {
                            {
                                let mut st = stats.lock();
                                st.files += 1;
                                st.files_skipped += s.files_skipped;
                                st.chunks += s.chunks;
                                st.symbols += s.symbols;
                                st.calls += s.calls;
                                st.imports += s.imports;
                            }
                            if want_embed {
                                local_chunks = chunks;
                            }
                        }
                        Err(e) => {
                            debug!(?path, error = %e, "skip: index error");
                        }
                    }
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    this.emit(IndexProgress {
                        stage: "file",
                        done: d,
                        total,
                        path: Some(&path),
                    });
                    local_chunks
                }
            })
            .buffer_unordered(self.concurrency)
            .collect()
            .await;

        // Phase B: batch embedding (Mode::Full only).
        if want_embed {
            let chunks: Vec<SourceChunk> = tasks.into_iter().flatten().collect::<Vec<_>>();
            let total_embed = chunks.len();
            self.emit(IndexProgress {
                stage: "embed",
                done: 0,
                total: total_embed,
                path: None,
            });
            let mut embedded = 0usize;
            for (i, batch) in chunks.chunks(EMBED_BATCH_SIZE).enumerate() {
                match self.embed_chunks(batch).await {
                    Ok(Some(n)) => embedded += n,
                    Ok(None) => {}
                    Err(e) => {
                        // Don't abort the whole run — log, skip this batch.
                        debug!(error = %e, batch = i, "skip: embed error");
                    }
                }
                let done_count = ((i + 1) * EMBED_BATCH_SIZE).min(total_embed);
                self.emit(IndexProgress {
                    stage: "embed",
                    done: done_count,
                    total: total_embed,
                    path: None,
                });
            }
            stats.lock().embedded += embedded;
        }

        // Phase C: commit FTS.
        self.emit(IndexProgress {
            stage: "commit",
            done: total,
            total,
            path: None,
        });
        self.store.fts.commit().await?;

        let final_stats = *stats.lock();
        debug!(?final_stats, "index complete");
        Ok(final_stats)
    }

    /// Index a single file. Public so callers (e.g. the watcher) can
    /// re-index on change. Embeds the file's chunks inline if a provider is
    /// configured.
    ///
    /// Any pre-existing index state for `path` (FTS chunks, KV symbol rows,
    /// graph nodes/edges, and — in Mode::Full — vectors) is purged before
    /// the new parse is written, so line shifts, renames, and deleted
    /// symbols don't leave stale rows behind. The caller is still
    /// responsible for calling [`Indexer::commit`] before the next query.
    pub async fn index_file(&self, path: &Path) -> Result<IndexStats> {
        let (mut s, chunks) = self.index_file_no_embed(path).await?;
        if let Some(n) = self.embed_chunks(&chunks).await? {
            s.embedded += n;
        }
        Ok(s)
    }

    /// Remove every indexed artefact that references `path` — FTS chunks,
    /// KV symbol rows, graph nodes/edges, and (if Mode::Full) vectors.
    ///
    /// Used by both [`Indexer::index_file`] (purge-before-write) and the
    /// watcher's `FileDeleted` branch (purge-only, no reparse). Commit is
    /// the caller's responsibility so batched watch events can coalesce
    /// into a single flush.
    pub async fn purge_path(&self, path: &Path) -> Result<()> {
        let path_str = path.to_string_lossy().into_owned();

        // 1. FTS — delete every doc whose `path` field matches.
        self.store.fts.delete_path(&path_str).await?;

        // 2. KV symbols — collect the FQNs we're dropping so we can also
        //    prune graph nodes and any edges that touch them.
        let symbol_fqns = self.store.kv.delete_symbols_by_path(path).await?;

        // 3. Graph nodes + edges.
        let (_node_count, _edge_count) = self.graph.purge_path(path).await?;
        if !symbol_fqns.is_empty() {
            // `delete_nodes_by_path` will usually be a superset, but some
            // symbol rows live without matching graph nodes (e.g. when the
            // parser produced a symbol but not a node — rare, belt-and-
            // braces). Drop any edges keyed on those FQNs explicitly.
            let _ = self.store.kv.delete_edges_touching(&symbol_fqns).await?;
        }

        // 4. ChromaDB vectors — delete chunks for this path.
        if let Some(col) = &self.chroma {
            let filter = serde_json::json!({"path": {"$eq": path_str}});
            let _ = col.delete_by_filter(filter).await;
        }

        // 5. Drop the content-hash sentinel so the next writer sees a miss
        //    and rebuilds from scratch. Without this, deleting + recreating
        //    a file would short-circuit on the old hash.
        self.store.kv.delete_meta(hash_meta_key(path)).await?;

        Ok(())
    }

    /// Flush the BM25 writer so previously indexed chunks become
    /// searchable. Safe to call repeatedly.
    pub async fn commit(&self) -> Result<()> {
        self.store.fts.commit().await
    }

    /// Internal: parse + write chunks/symbols/edges for one file, returning
    /// the parsed chunks so a caller (e.g. [`Indexer::index_path`]) can defer
    /// embedding and batch it across files.
    ///
    /// The file's previous index state is purged before the new parse is
    /// written so stale chunks (e.g. from a function that moved lines or
    /// was deleted) can never linger.
    ///
    /// # Content-hash gating
    ///
    /// Before doing any work, we blake3 the file bytes and compare against
    /// the hash we stored under `hash:<path>` the last time this file was
    /// indexed. If they match, the on-disk state is authoritative and we
    /// short-circuit — no purge, no reparse, no writes. This is DESIGN §9's
    /// "content-hash gated" writer clause. On a hash miss (new file, real
    /// edit, or first-ever index) we fall through to the full pipeline and
    /// record the new hash at the end.
    async fn index_file_no_embed(&self, path: &Path) -> Result<(IndexStats, Vec<SourceChunk>)> {
        let bytes = tokio::fs::read(path).await?;
        let new_hash = blake3::hash(&bytes);
        let hash_key = hash_meta_key(path);

        let new_hash_bytes: &[u8] = new_hash.as_bytes();
        if let Some(prev) = self.store.kv.get_meta(hash_key.clone()).await?
            && prev.as_slice() == new_hash_bytes
        {
            debug!(?path, "skip: content hash unchanged");
            let skipped = IndexStats {
                files_skipped: 1,
                ..IndexStats::default()
            };
            return Ok((skipped, Vec::new()));
        }

        self.purge_path(path).await?;
        let (chunks, table) = thoth_parse::parse_file(&self.registry, path).await?;

        // --- batch FTS writes (single spawn_blocking) ---
        let chunk_docs: Vec<ChunkDoc> = chunks
            .iter()
            .map(|c| ChunkDoc {
                id: chunk_id(&c.path, c.start_line, c.end_line),
                path: c.path.to_string_lossy().into_owned(),
                symbol: c.symbol.clone(),
                body: c.body.clone(),
                start_line: c.start_line,
                end_line: c.end_line,
                language: c.language.to_string(),
            })
            .collect();
        self.store.fts.index_chunks_batch(chunk_docs).await?;

        // --- batch KV symbol writes (single redb transaction) ---
        let symbol_rows: Vec<SymbolRow> = table
            .symbols
            .iter()
            .map(|sym| SymbolRow {
                fqn: sym.fqn.clone(),
                path: sym.path.clone(),
                start_line: sym.span.0,
                end_line: sym.span.1,
                kind: symbol_kind_tag(sym.kind).to_string(),
            })
            .collect();
        self.store.kv.put_symbols_batch(symbol_rows).await?;

        // --- batch graph node writes (single redb transaction) ---
        let nodes: Vec<Node> = table
            .symbols
            .iter()
            .map(|sym| Node {
                fqn: sym.fqn.clone(),
                kind: symbol_kind_tag(sym.kind).to_string(),
                path: sym.path.clone(),
                line: sym.span.0,
            })
            .collect();
        self.graph.upsert_nodes_batch(nodes).await?;

        // Build the file-local resolution map.
        let mut resolution: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        for sym in &table.symbols {
            if let Some(leaf) = sym.fqn.rsplit("::").next() {
                resolution
                    .entry(leaf.to_string())
                    .or_insert_with(|| sym.fqn.clone());
            }
        }
        for (local, target) in &table.aliases {
            resolution.insert(local.clone(), target.clone());
        }

        // --- batch all edges (calls + imports + extends) in one transaction ---
        let mut all_edges: Vec<Edge> = Vec::new();

        // Call edges
        for (caller, callee) in &table.calls {
            let resolved = resolution
                .get(callee)
                .cloned()
                .unwrap_or_else(|| callee.clone());
            all_edges.push(Edge {
                from: caller.clone(),
                to: resolved,
                kind: EdgeKind::Calls,
            });
        }

        // Import edges
        let alias_only: std::collections::HashMap<String, String> =
            table.aliases.iter().cloned().collect();
        let module = module_fqn(path);
        if !alias_only.is_empty() {
            let mut seen = std::collections::HashSet::new();
            for target in alias_only.values() {
                if seen.insert(target.clone()) {
                    all_edges.push(Edge {
                        from: module.clone(),
                        to: target.clone(),
                        kind: EdgeKind::Imports,
                    });
                }
            }
        } else {
            for imp in &table.imports {
                let target = imp.trim().to_string();
                if !target.is_empty() {
                    all_edges.push(Edge {
                        from: module.clone(),
                        to: target,
                        kind: EdgeKind::Imports,
                    });
                }
            }
        }

        // Extends edges
        for (child, parent) in &table.extends {
            let resolved = resolution
                .get(parent)
                .cloned()
                .unwrap_or_else(|| parent.clone());
            if !resolved.is_empty() {
                all_edges.push(Edge {
                    from: child.clone(),
                    to: resolved,
                    kind: EdgeKind::Extends,
                });
            }
        }

        // References edges — a type mentioned inside another symbol's
        // body / signature. Dedup on `(from, to)` so repeated mentions
        // inside the same owner collapse to one edge, matching the
        // semantics of the key-based dedup redb does anyway but saving
        // the round-trip.
        let mut ref_seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        for (referrer, ty) in &table.references {
            let resolved = resolution
                .get(ty)
                .cloned()
                .unwrap_or_else(|| ty.clone());
            if resolved.is_empty() || resolved == *referrer {
                continue;
            }
            if !ref_seen.insert((referrer.clone(), resolved.clone())) {
                continue;
            }
            all_edges.push(Edge {
                from: referrer.clone(),
                to: resolved,
                kind: EdgeKind::References,
            });
        }

        self.graph.upsert_edges_batch(all_edges).await?;

        let s = IndexStats {
            files: 0, // caller increments
            files_skipped: 0,
            chunks: chunks.len(),
            symbols: table.symbols.len(),
            calls: table.calls.len(),
            imports: table.imports.len(),
            embedded: 0,
        };

        // Record the new content hash *after* all the writes succeeded.
        self.store.kv.put_meta(hash_key, new_hash_bytes).await?;

        Ok((s, chunks))
    }

    /// Upsert chunks into ChromaDB (server-side embedding).
    async fn embed_chunks(&self, chunks: &[SourceChunk]) -> Result<Option<usize>> {
        let Some(col) = self.chroma.as_ref() else {
            return Ok(None);
        };
        if chunks.is_empty() {
            return Ok(Some(0));
        }

        let ids: Vec<String> = chunks
            .iter()
            .map(|c| chunk_id(&c.path, c.start_line, c.end_line))
            .collect();
        let documents: Vec<String> = chunks.iter().map(|c| c.body.clone()).collect();
        let metadatas: Vec<std::collections::HashMap<String, serde_json::Value>> = chunks
            .iter()
            .map(|c| {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "path".to_string(),
                    serde_json::json!(c.path.to_string_lossy()),
                );
                m.insert("start_line".to_string(), serde_json::json!(c.start_line));
                m.insert("end_line".to_string(), serde_json::json!(c.end_line));
                if let Some(sym) = &c.symbol {
                    m.insert("symbol".to_string(), serde_json::json!(sym));
                }
                m
            })
            .collect();

        let count = ids.len();
        col.upsert(ids, Some(documents), Some(metadatas)).await?;
        Ok(Some(count))
    }
}

// ---- helpers ---------------------------------------------------------------

/// Stable chunk id — the tantivy delete key on re-index.
pub fn chunk_id(path: &Path, start: u32, end: u32) -> String {
    format!("{}:{}-{}", path.display(), start, end)
}

fn symbol_kind_tag(k: SymbolKind) -> &'static str {
    match k {
        SymbolKind::Function => "function",
        SymbolKind::Type => "type",
        SymbolKind::Trait => "trait",
        SymbolKind::Module => "module",
        SymbolKind::Binding => "binding",
    }
}

/// Bump whenever the indexer's output schema changes (new edge kinds,
/// new alias-resolution rules, renamed FQN scheme, ...). The version
/// baked into the hash meta key invalidates every previously-stored
/// hash sentinel in one go, so the next indexer run re-parses every
/// file even when its bytes haven't changed.
const PARSER_SCHEMA_VERSION: u32 = 3;

/// Meta key under which we store the blake3 hash of the last-indexed bytes
/// of `path`. Kept private to the indexer — callers shouldn't need to read
/// it. The `hashVER:` prefix carries both the schema version (so a parser
/// upgrade invalidates every sentinel at once) and leaves room for future
/// per-path sentinels (e.g. `mtime:`) without colliding.
fn hash_meta_key(path: &Path) -> String {
    format!("hash{PARSER_SCHEMA_VERSION}:{}", path.display())
}

fn module_fqn(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Pick a sensible default fan-out for [`Indexer::index_path`]. Uses the
/// logical CPU count, capped at 16 so we don't stampede the provider's
/// rate limits or the underlying store mutexes on very large machines.
fn default_concurrency() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get().min(16))
        .unwrap_or(4)
}

/// Read the lines `[start_line..=end_line]` (1-based, inclusive) from a file,
/// returning the body text. Used when retrieval needs to surface the code
/// that FTS/graph only referenced by coordinates.
pub async fn read_span(path: &Path, start_line: u32, end_line: u32) -> Result<String> {
    let text = tokio::fs::read_to_string(path).await?;
    let start = start_line.saturating_sub(1) as usize;
    let end = end_line as usize;
    let mut out = String::new();
    for (i, line) in text.lines().enumerate() {
        if i >= start && i < end {
            out.push_str(line);
            out.push('\n');
        }
        if i >= end {
            break;
        }
    }
    Ok(out)
}
