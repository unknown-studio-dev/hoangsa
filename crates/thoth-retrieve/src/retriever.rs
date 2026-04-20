//! Hybrid retrieval orchestrator.
//!
//! In **Mode::Zero** four independent recallers run over the local indexes:
//!
//! 1. **Symbol lookup** over [`KvStore`] (exact + prefix on FQN tokens).
//! 2. **BM25** via [`FtsIndex`].
//! 3. **Graph fan-out** from whichever symbols the first two steps hit.
//! 4. **Markdown grep** over `MEMORY.md` (fact bullets surface as chunks).
//!
//! In **Mode::Full** an additional ChromaDB vector stage runs — the query
//! text is sent to ChromaDB for server-side embedding and ANN search, and
//! the top-k nearest neighbours are folded into the fusion alongside the
//! other sources. If a [`Synthesizer`] is configured, the fused chunks are
//! then handed to it to produce a natural-language answer.
//!
//! Results are fused with [Reciprocal Rank Fusion][rrf] and the top-K by
//! fused score becomes the [`Retrieval`].
//!
//! [rrf]: https://plg.uwaterloo.ca/~gvcormac/cormacksigir09-rrf.pdf

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use thoth_core::{
    Chunk, Event, Prompt, Query, QueryScope, Result, Retrieval, RetrievalSource, Synthesizer,
};
use thoth_graph::Graph;
use thoth_store::{ChromaCol, EpisodeHit, FtsHit, KvStore, MarkdownStore, StoreRoot, SymbolRow};
use uuid::Uuid;

use crate::indexer::{chunk_id, read_span};

/// Reciprocal-Rank-Fusion constant. The classic Cormack/Clarke default is
/// 60, designed for web-scale result lists (100s of candidates per source).
/// Thoth's per-source stages typically return < 20 candidates, so K=60
/// compresses all single-stage hits into a ~0.3% score band (0.0164→0.0154),
/// making ranking effectively random. K=10 gives a ~9% spread per rank step,
/// which lets multi-stage hits stand out clearly while still dampening
/// outlier ranks in larger result sets.
const RRF_K: f32 = 10.0;

/// Top-level retrieval orchestrator.
///
/// Built on top of an opened [`StoreRoot`]. Cheap to clone.
#[derive(Clone)]
pub struct Retriever {
    store: StoreRoot,
    graph: Graph,
    chroma: Option<Arc<ChromaCol>>,
    synthesizer: Option<Arc<dyn Synthesizer>>,
    /// Multiplier applied to the fused score of every
    /// `RetrievalSource::Markdown` hit after RRF, before top-K selection.
    /// `1.0` is the identity (no boost). Set via
    /// [`Retriever::with_markdown_boost`] or the `[retrieve]
    /// rerank_markdown_boost` config key.
    markdown_boost: f32,
}

impl Retriever {
    /// Create a Mode::Zero retriever — no synthesis. ChromaDB is used for
    /// semantic search if configured.
    pub fn new(store: StoreRoot) -> Self {
        let graph = Graph::new(store.kv.clone());
        Self {
            store,
            graph,
            chroma: None,
            synthesizer: None,
            markdown_boost: 1.0,
        }
    }

    /// Create a Mode::Full retriever with any of the optional providers.
    pub fn with_full(
        store: StoreRoot,
        chroma: Option<Arc<ChromaCol>>,
        synthesizer: Option<Arc<dyn Synthesizer>>,
    ) -> Self {
        let graph = Graph::new(store.kv.clone());
        Self {
            store,
            graph,
            chroma,
            synthesizer,
            markdown_boost: 1.0,
        }
    }

    /// Attach a ChromaDB store + collection ID for semantic vector search.
    pub fn with_chroma(mut self, chroma: Option<Arc<ChromaCol>>) -> Self {
        self.chroma = chroma;
        self
    }

    /// Set the post-RRF multiplier applied to Markdown-sourced hits.
    ///
    /// Useful when a fact or lesson in `MEMORY.md` / `LESSONS.md` ranks
    /// below code chunks for queries whose tokens also appear literally
    /// in source (e.g. identifier-heavy phrasing). Mirrors the
    /// `[retrieve] rerank_markdown_boost` config knob. Values are taken
    /// as-is — the config loader already clamps into `[0.0, 10.0]`.
    pub fn with_markdown_boost(mut self, boost: f32) -> Self {
        self.markdown_boost = boost;
        self
    }

    /// Mode::Zero recall. Runs each local source, then fuses with RRF.
    pub async fn recall(&self, q: &Query) -> Result<Retrieval> {
        self.recall_inner(q, /* with_vector */ false, /* with_synth */ false)
            .await
    }

    /// Mode::Full recall. Adds the vector stage (if an embedder + vector
    /// store are configured) and runs the synthesizer (if one is configured)
    /// against the top-K fused chunks.
    pub async fn recall_full(&self, q: &Query) -> Result<Retrieval> {
        self.recall_inner(q, /* with_vector */ true, /* with_synth */ true)
            .await
    }

    async fn recall_inner(
        &self,
        q: &Query,
        with_vector: bool,
        with_synth: bool,
    ) -> Result<Retrieval> {
        let k = q.top_k.max(1);
        let scope = &q.scope;

        // 0. (Mode::Full) optional query rewrite via the Synthesizer. We fall
        //    back to the original on `Ok(None)` or any error — retrieval
        //    should never harden-fail because a rewrite failed.
        let search_text: String = if with_synth && let Some(s) = self.synthesizer.as_ref() {
            match s.rewrite_query(&q.text).await {
                Ok(Some(rewritten)) if !rewritten.trim().is_empty() => rewritten,
                _ => q.text.clone(),
            }
        } else {
            q.text.clone()
        };

        let mut fused: HashMap<String, FusedRow> = HashMap::new();

        // 1. symbol lookup
        let sym_hits = filter_scope(self.symbol_stage(&search_text).await?, scope);
        for (rank, cand) in sym_hits.iter().enumerate() {
            fuse(&mut fused, cand, rank);
        }

        // 2. BM25
        let fts_hits = filter_scope(self.bm25_stage(&search_text, k * 3).await?, scope);
        for (rank, cand) in fts_hits.iter().enumerate() {
            fuse(&mut fused, cand, rank);
        }

        // 3. graph fan-out (depth 1) from the first handful of symbol seeds
        let seeds: Vec<String> = sym_hits
            .iter()
            .chain(fts_hits.iter())
            .filter_map(|c| c.symbol.clone())
            .take(8)
            .collect();

        // Stages 3 (graph), 4 (markdown), 4b (lessons), 5 (episodic)
        // are all independent once `seeds` are known — run them concurrently.
        let (graph_hits, md_hits, lesson_hits, ep_hits) = tokio::join!(
            self.graph_stage(&seeds),
            self.markdown_stage(&search_text, &scope.tags),
            self.lessons_stage(&search_text),
            self.episodic_stage(&search_text, k * 2),
        );
        let graph_hits = filter_scope(graph_hits?, scope);
        let md_hits = md_hits?;
        let lesson_hits = lesson_hits?;
        let ep_hits = ep_hits?;

        for (rank, cand) in graph_hits.iter().enumerate() {
            fuse(&mut fused, cand, rank);
        }

        // 4. markdown grep (scope doesn't apply — MEMORY.md is global)
        for (rank, cand) in md_hits.iter().enumerate() {
            fuse(&mut fused, cand, rank);
        }

        // 4b. reflective lessons — keyed by trigger / advice, also global.
        //     Separate from (4) so a future query can weight them differently;
        //     today they share `RetrievalSource::Markdown` and only differ by
        //     path (`LESSONS.md` vs `MEMORY.md`), which is enough for callers
        //     to tell them apart in renders.
        for (rank, cand) in lesson_hits.iter().enumerate() {
            fuse(&mut fused, cand, rank);
        }

        // 5. episodic log — past queries / answers / outcomes. Scope filters
        //    do not apply; episodes are cross-cutting.
        for (rank, cand) in ep_hits.iter().enumerate() {
            fuse(&mut fused, cand, rank);
        }

        // 6. (Mode::Full) vector similarity — only if both an embedder and a
        //    vector store are configured. Silent no-op otherwise so that
        //    Mode::Full without a key still degrades to Mode::Zero recall.
        if with_vector && let Some(hits) = self.vector_stage(&search_text, k * 3).await? {
            let scoped = filter_scope(hits, scope);
            for (rank, cand) in scoped.iter().enumerate() {
                fuse(&mut fused, cand, rank);
            }
        }

        // Apply the Markdown rerank boost before sorting so boosted
        // lessons/facts get the full benefit against code hits. Skipped
        // (as a micro-opt) when the boost is the identity — avoids
        // touching the score array in the overwhelmingly common case.
        let mut ranked: Vec<FusedRow> = fused.into_values().collect();
        if (self.markdown_boost - 1.0).abs() > f32::EPSILON {
            for row in ranked.iter_mut() {
                if matches!(row.cand.source, RetrievalSource::Markdown) {
                    row.score *= self.markdown_boost;
                }
            }
        }

        // Sort by fused score, take top-k.
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Dedup by (path, symbol) *before* truncating to k so we don't
        // lose a slot to a near-duplicate hit. Different spans of the
        // same symbol (e.g. the symbol stage reported the declaration
        // and BM25 reported a paragraph inside it) collapse to the
        // highest-scoring representative.
        let ranked = dedupe_by_path_symbol(ranked);

        let mut chunks = Vec::with_capacity(k);
        for row in ranked.into_iter().take(k) {
            chunks.push(self.materialize(row).await?);
        }

        // Enrich the top-K with graph context (callers/callees/imports/
        // siblings/doc). Best-effort — a failure here should never sink
        // an otherwise-successful recall, so log and continue.
        if let Err(e) = crate::enrich::enrich_chunks(&self.graph, &mut chunks).await {
            tracing::debug!(error = %e, "enrichment failed; returning unenriched chunks");
        }

        // 7. (Mode::Full) synthesis.
        let synthesized = if with_synth {
            self.synthesize(&q.text, &chunks).await?
        } else {
            None
        };

        Ok(Retrieval {
            chunks,
            synthesized,
            correlation_id: Uuid::new_v4(),
        })
    }

    // ---- per-source stages -------------------------------------------------

    async fn symbol_stage(&self, text: &str) -> Result<Vec<Candidate>> {
        let kv: &KvStore = &self.store.kv;
        let mut out = Vec::new();
        for tok in tokens(text) {
            let rows: Vec<SymbolRow> = kv.symbols_with_prefix(tok.clone()).await?;
            for r in rows {
                out.push(Candidate::from_symbol(r));
            }
        }
        dedupe(&mut out);
        Ok(out)
    }

    async fn bm25_stage(&self, text: &str, k: usize) -> Result<Vec<Candidate>> {
        let hits: Vec<FtsHit> = self.store.fts.search(text, k).await?;
        Ok(hits.into_iter().map(Candidate::from_fts).collect())
    }

    async fn graph_stage(&self, seeds: &[String]) -> Result<Vec<Candidate>> {
        use futures::StreamExt;
        let seeds_owned: Vec<String> = seeds.to_vec();
        let graph = self.graph.clone();
        let results = futures::stream::iter(seeds_owned)
            .map(move |seed| {
                let graph = graph.clone();
                async move {
                    let ns = graph.neighbors(&seed, 1).await?;
                    let cands: Vec<Candidate> = ns
                        .into_iter()
                        .map(|n| Candidate {
                            id: chunk_id(&n.path, n.line, n.line),
                            path: n.path,
                            start_line: n.line,
                            end_line: n.line,
                            symbol: Some(n.fqn),
                            source: RetrievalSource::Graph,
                            preview: None,
                        })
                        .collect();
                    Ok::<Vec<Candidate>, thoth_core::Error>(cands)
                }
            })
            .buffer_unordered(8)
            .collect::<Vec<_>>()
            .await;
        let mut out = Vec::new();
        for batch in results {
            out.extend(batch?);
        }
        dedupe(&mut out);
        Ok(out)
    }

    async fn markdown_stage(&self, text: &str, tags: &[String]) -> Result<Vec<Candidate>> {
        let md: &MarkdownStore = &self.store.markdown;
        let toks = tokens(text);
        let facts = md.grep_facts_multi(&toks).await?;
        let mut out = Vec::new();
        for f in facts {
            if !tags.is_empty()
                && !f
                    .tags
                    .iter()
                    .any(|t| tags.iter().any(|q| t.eq_ignore_ascii_case(q)))
            {
                continue;
            }
            let preview = first_nonempty_line(&f.text);
            let id = format!("memory.md:{}", blake3::hash(f.text.as_bytes()).to_hex());
            out.push(Candidate {
                id,
                path: PathBuf::from("MEMORY.md"),
                start_line: 0,
                end_line: 0,
                symbol: None,
                source: RetrievalSource::Markdown,
                preview: Some(preview),
            });
        }
        Ok(out)
    }

    /// Grep `LESSONS.md` for lessons whose `trigger` or `advice` match any
    /// meaningful token in the query. Same shape as [`Self::markdown_stage`]
    /// but renders a `trigger → advice` preview so the caller can see at a
    /// glance why the lesson fired. Ids are namespaced under `lessons.md:`
    /// so they never collide with fact ids from MEMORY.md.
    async fn lessons_stage(&self, text: &str) -> Result<Vec<Candidate>> {
        let md: &MarkdownStore = &self.store.markdown;
        let toks = tokens(text);
        let lessons = md.grep_lessons_multi(&toks).await?;
        let mut out = Vec::new();
        for l in lessons {
            let advice_line = first_nonempty_line(&l.advice);
            let preview = if advice_line.is_empty() {
                format!("lesson — {}", l.trigger.trim())
            } else {
                format!("lesson — {} → {}", l.trigger.trim(), advice_line)
            };
            // Stable id on the trigger alone: advice edits (e.g. bumped
            // success counters) must not spawn a new chunk.
            let id = format!(
                "lessons.md:{}",
                blake3::hash(l.trigger.trim().to_lowercase().as_bytes()).to_hex()
            );
            out.push(Candidate {
                id,
                path: PathBuf::from("LESSONS.md"),
                start_line: 0,
                end_line: 0,
                symbol: None,
                source: RetrievalSource::Markdown,
                preview: Some(preview),
            });
        }
        Ok(out)
    }

    /// Search the episodic log for relevant past events. Silently returns
    /// an empty vec if the FTS5 query can't be built (e.g. the user query
    /// contained only stopwords or punctuation).
    ///
    /// Every hit also has its `access_count` / `last_accessed_ns` bumped on
    /// the way out so the decay-based forget pass (DESIGN §9) sees the
    /// retrieval. Bump failures are logged and ignored — they must not
    /// break recall.
    async fn episodic_stage(&self, text: &str, k: usize) -> Result<Vec<Candidate>> {
        let match_expr = match fts5_match_expr(text) {
            Some(m) => m,
            None => return Ok(Vec::new()),
        };
        let hits: Vec<EpisodeHit> = match self.store.episodes.search(match_expr, k).await {
            Ok(h) => h,
            // A malformed MATCH expression, a locked DB, etc. shouldn't
            // break the rest of retrieval. Log-worthy, but not fatal.
            Err(_) => return Ok(Vec::new()),
        };
        let now_ns = now_unix_ns();
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            let row_id = h.id;
            if let Some(c) = Candidate::from_episode(h) {
                out.push(c);
                if let Err(e) = self.store.episodes.bump_access_by_id(row_id, now_ns).await {
                    tracing::debug!(error = %e, row_id, "episodic: bump_access failed");
                }
            }
        }
        Ok(out)
    }

    /// Semantic vector search via ChromaDB. Returns `Ok(None)` when
    /// ChromaDB is not configured.
    async fn vector_stage(&self, text: &str, k: usize) -> Result<Option<Vec<Candidate>>> {
        let Some(col) = self.chroma.as_ref() else {
            return Ok(None);
        };
        let hits = col.query_text(text, k, None).await?;
        let mut out = Vec::with_capacity(hits.len());
        for h in hits {
            if let Some((path, start, end)) = parse_chunk_id(&h.id) {
                out.push(Candidate {
                    id: h.id,
                    path,
                    start_line: start,
                    end_line: end,
                    symbol: None,
                    source: RetrievalSource::Vector,
                    preview: h.document,
                });
            }
        }
        Ok(Some(out))
    }

    // ---- synthesis ---------------------------------------------------------

    async fn synthesize(&self, question: &str, chunks: &[Chunk]) -> Result<Option<String>> {
        let Some(synth) = self.synthesizer.as_ref() else {
            return Ok(None);
        };
        if chunks.is_empty() {
            return Ok(None);
        }
        // Pull lessons so Claude can weave them in. Failing to read lessons
        // is not fatal — treat as empty.
        let lessons = self.store.markdown.read_lessons().await.unwrap_or_default();

        let prompt = Prompt {
            question: question.to_string(),
            chunks: chunks.to_vec(),
            lessons,
            max_tokens: None,
        };
        let syn = synth.synthesize(&prompt).await?;
        Ok(Some(syn.answer))
    }

    // ---- chunk materialisation --------------------------------------------

    async fn materialize(&self, row: FusedRow) -> Result<Chunk> {
        let (body, preview) = match row.cand.source {
            RetrievalSource::Markdown | RetrievalSource::Episodic => {
                // Both already carry their body inline — no on-disk span to
                // re-read. Episodic payloads come from the SQLite log, and
                // markdown facts from the MEMORY.md grep above.
                let p = row.cand.preview.clone().unwrap_or_default();
                (p.clone(), p)
            }
            _ => {
                let body = read_span(
                    &row.cand.path,
                    row.cand.start_line.max(1),
                    row.cand.end_line.max(row.cand.start_line.max(1)),
                )
                .await
                .unwrap_or_default();
                let prev = short_preview(&body);
                (body, prev)
            }
        };

        Ok(Chunk {
            id: row.cand.id,
            path: row.cand.path,
            line: row.cand.start_line,
            span: (row.cand.start_line, row.cand.end_line),
            symbol: row.cand.symbol,
            preview,
            body,
            score: row.score,
            source: row.cand.source,
            context: None,
        })
    }
}

// ---- fusion helpers --------------------------------------------------------

#[derive(Clone)]
struct Candidate {
    id: String,
    path: PathBuf,
    start_line: u32,
    end_line: u32,
    symbol: Option<String>,
    source: RetrievalSource,
    preview: Option<String>,
}

impl Candidate {
    fn from_symbol(r: SymbolRow) -> Self {
        Self {
            id: chunk_id(&r.path, r.start_line, r.end_line),
            path: r.path,
            start_line: r.start_line,
            end_line: r.end_line,
            symbol: Some(r.fqn),
            source: RetrievalSource::Symbol,
            preview: None,
        }
    }

    fn from_fts(h: FtsHit) -> Self {
        Self {
            id: h.id,
            path: PathBuf::from(h.path),
            start_line: h.start_line,
            end_line: h.end_line,
            symbol: h.symbol,
            source: RetrievalSource::FullText,
            preview: None,
        }
    }

    /// Build a candidate from an episodic log hit. Returns `None` when the
    /// event kind carries no surfaceable text (e.g. `FileDeleted`, which is
    /// useful for audit but nothing for retrieval to show).
    fn from_episode(h: EpisodeHit) -> Option<Self> {
        let preview = episode_preview(&h.event)?;
        // Stable id scoped under an `episode:` prefix so it never collides
        // with file-backed chunk ids or markdown memory ids.
        let id = format!("episode:{}", h.id);
        Some(Self {
            id,
            path: PathBuf::from("<episodes.db>"),
            start_line: 0,
            end_line: 0,
            symbol: None,
            source: RetrievalSource::Episodic,
            preview: Some(preview),
        })
    }
}

struct FusedRow {
    cand: Candidate,
    score: f32,
}

fn fuse(map: &mut HashMap<String, FusedRow>, c: &Candidate, rank: usize) {
    let delta = 1.0 / (RRF_K + rank as f32 + 1.0);
    map.entry(c.id.clone())
        .and_modify(|row| row.score += delta)
        .or_insert_with(|| FusedRow {
            cand: c.clone(),
            score: delta,
        });
}

fn dedupe(v: &mut Vec<Candidate>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|c| seen.insert(c.id.clone()));
}

/// Collapse near-duplicate hits into a single ranked slot.
///
/// Two `FusedRow`s are considered the "same symbol" when they share a
/// path and a non-empty `symbol` field. The highest-scoring representative
/// wins; the loser's score is *added* to the winner so the fusion signal
/// isn't silently discarded.
///
/// Rows without a symbol (markdown/episodic/vector hits that never hit
/// the graph) pass through untouched — there's no meaningful way to fold
/// them together, and they each carry their own stable id.
///
/// The output retains the original descending-score ordering.
fn dedupe_by_path_symbol(ranked: Vec<FusedRow>) -> Vec<FusedRow> {
    let mut out: Vec<FusedRow> = Vec::with_capacity(ranked.len());
    let mut index: HashMap<(PathBuf, String), usize> = HashMap::new();
    for row in ranked {
        let Some(sym) = row.cand.symbol.clone() else {
            out.push(row);
            continue;
        };
        let key = (row.cand.path.clone(), sym);
        if let Some(&i) = index.get(&key) {
            // Fold the loser's score into the winner. We don't swap —
            // the winner was already ranked higher.
            out[i].score += row.score;
        } else {
            index.insert(key, out.len());
            out.push(row);
        }
    }
    out
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != ':')
        .filter(|t| t.len() >= 3)
        .map(str::to_ascii_lowercase)
        .collect()
}

fn first_nonempty_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

fn short_preview(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .take(3)
        .collect::<Vec<_>>()
        .join(" ⏎ ")
}

/// Current wall-clock as Unix nanoseconds, clamped into `i64`. Used when
/// stamping `last_accessed_ns` on episodic rows.
fn now_unix_ns() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Apply a [`QueryScope`] filter to a batch of candidates. An empty scope is
/// a no-op. When any of `paths` / `languages` / `symbols` is non-empty, a
/// candidate must pass every populated axis to survive:
///
/// * **paths** — candidate path starts with *any* listed prefix.
/// * **languages** — candidate file extension maps to *any* listed language.
/// * **symbols** — candidate symbol contains (case-insensitive) *any* listed
///   token. Candidates with no symbol are dropped when this filter is set.
fn filter_scope(cands: Vec<Candidate>, scope: &QueryScope) -> Vec<Candidate> {
    if scope.paths.is_empty() && scope.languages.is_empty() && scope.symbols.is_empty() {
        return cands;
    }
    cands
        .into_iter()
        .filter(|c| {
            if !scope.paths.is_empty() && !scope.paths.iter().any(|p| path_has_prefix(&c.path, p)) {
                return false;
            }
            if !scope.languages.is_empty()
                && !scope
                    .languages
                    .iter()
                    .any(|l| path_language_matches(&c.path, l))
            {
                return false;
            }
            if !scope.symbols.is_empty() {
                let Some(sym) = c.symbol.as_deref() else {
                    return false;
                };
                let sym_lc = sym.to_ascii_lowercase();
                if !scope
                    .symbols
                    .iter()
                    .any(|s| sym_lc.contains(&s.to_ascii_lowercase()))
                {
                    return false;
                }
            }
            true
        })
        .collect()
}

fn path_has_prefix(path: &Path, prefix: &Path) -> bool {
    // Accept both exact-prefix matches and lexical `starts_with` on the
    // string form — lets callers pass either a directory or a substring
    // like `"src/auth"`.
    if path.starts_with(prefix) {
        return true;
    }
    let p = path.to_string_lossy();
    let pre = prefix.to_string_lossy();
    p.contains(pre.as_ref())
}

/// Map an extension to a language name using the same table `thoth-parse`
/// uses. Keeping the mapping inline here avoids pulling `thoth-parse` into
/// `thoth-retrieve`'s dep graph just for a scope filter.
fn path_language_matches(path: &Path, lang: &str) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let lang = lang.to_ascii_lowercase();
    let ext = ext.to_ascii_lowercase();
    match lang.as_str() {
        "rust" => ext == "rs",
        "python" => matches!(ext.as_str(), "py" | "pyi"),
        "javascript" => matches!(ext.as_str(), "js" | "mjs" | "cjs" | "jsx"),
        "typescript" => matches!(ext.as_str(), "ts" | "tsx"),
        "go" => ext == "go",
        "markdown" => matches!(ext.as_str(), "md" | "markdown"),
        other => ext == other, // fall back to raw-extension match
    }
}

/// Render a retrieval-friendly preview of an episodic event. Returns `None`
/// for events that have no useful surface text (e.g. `FileDeleted`, which
/// is audit-only).
fn episode_preview(ev: &Event) -> Option<String> {
    use thoth_core::Outcome;
    match ev {
        Event::QueryIssued { text, .. } => Some(format!("past query: {text}")),
        Event::AnswerReturned { chunk_ids, .. } if !chunk_ids.is_empty() => {
            Some(format!("past answer cited: {}", chunk_ids.join(", ")))
        }
        Event::OutcomeObserved { outcome, .. } => Some(match outcome {
            Outcome::Test { passed, suite } => {
                let status = if *passed { "pass" } else { "fail" };
                format!("past outcome — test {suite}: {status}")
            }
            Outcome::Commit { sha, .. } => format!("past outcome — commit {sha}"),
            Outcome::Revert { sha, reason } => {
                let why = reason.as_deref().unwrap_or("");
                format!("past outcome — revert {sha}: {why}")
                    .trim()
                    .to_string()
            }
            Outcome::UserFeedback { signal, note } => {
                let tail = note.as_deref().unwrap_or("");
                format!("past outcome — feedback {signal:?}: {tail}")
                    .trim()
                    .to_string()
            }
            Outcome::Error { summary, .. } => format!("past outcome — error: {summary}"),
        }),
        Event::FileChanged { path, .. } => Some(format!("file changed: {}", path.display())),
        Event::NudgeInvoked { intent, .. } if !intent.is_empty() => {
            Some(format!("past nudge: {intent}"))
        }
        Event::FileDeleted { .. } | Event::AnswerReturned { .. } | Event::NudgeInvoked { .. } => {
            None
        }
    }
}

/// Build a safe FTS5 MATCH expression from free-form query text. Splits on
/// non-word characters, keeps tokens of length >= 3, and joins them with
/// `OR`. Returns `None` if no usable token remains — in which case the
/// episodic stage is silently skipped rather than raising.
fn fts5_match_expr(text: &str) -> Option<String> {
    let toks: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 3)
        .map(|t| format!("\"{}\"", t.replace('"', "")))
        .collect();
    if toks.is_empty() {
        None
    } else {
        Some(toks.join(" OR "))
    }
}

/// Parse a `chunk_id` shaped like `"<path>:<start>-<end>"` back into its
/// components. Returns `None` if the id doesn't match the expected shape —
/// e.g. markdown memory ids which use a different scheme.
fn parse_chunk_id(id: &str) -> Option<(PathBuf, u32, u32)> {
    let colon = id.rfind(':')?;
    let (path_part, span_part) = id.split_at(colon);
    let span_part = &span_part[1..]; // drop the ':'
    let (s, e) = span_part.split_once('-')?;
    let start: u32 = s.parse().ok()?;
    let end: u32 = e.parse().ok()?;
    Some((PathBuf::from(path_part), start, end))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use thoth_core::QueryScope;

    #[test]
    fn parse_plain_chunk_id() {
        let id = "src/lib.rs:10-42";
        let (p, s, e) = parse_chunk_id(id).unwrap();
        assert_eq!(p, PathBuf::from("src/lib.rs"));
        assert_eq!(s, 10);
        assert_eq!(e, 42);
    }

    #[test]
    fn rejects_markdown_memory_id() {
        // memory.md:<64 hex> — span part won't parse as `<u32>-<u32>`.
        let id = "memory.md:deadbeef";
        assert!(parse_chunk_id(id).is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_chunk_id("no-colon-here").is_none());
        assert!(parse_chunk_id("path:not-numbers").is_none());
    }

    fn cand(path: &str, symbol: Option<&str>) -> Candidate {
        Candidate {
            id: format!("{path}:1-1"),
            path: PathBuf::from(path),
            start_line: 1,
            end_line: 1,
            symbol: symbol.map(str::to_owned),
            source: RetrievalSource::Symbol,
            preview: None,
        }
    }

    #[test]
    fn empty_scope_is_a_noop() {
        let cands = vec![
            cand("src/auth.rs", Some("auth::verify")),
            cand("src/user.rs", Some("user::new")),
        ];
        let scope = QueryScope::default();
        let out = filter_scope(cands.clone(), &scope);
        assert_eq!(out.len(), cands.len());
    }

    #[test]
    fn scope_paths_narrows_to_prefix() {
        let cands = vec![
            cand("src/auth/jwt.rs", None),
            cand("src/user/mod.rs", None),
            cand("tests/e2e.rs", None),
        ];
        let scope = QueryScope {
            paths: vec![PathBuf::from("src/auth")],
            ..Default::default()
        };
        let out = filter_scope(cands, &scope);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].path, PathBuf::from("src/auth/jwt.rs"));
    }

    #[test]
    fn scope_languages_matches_extensions() {
        let cands = vec![
            cand("src/lib.rs", None),
            cand("src/a.py", None),
            cand("src/b.ts", None),
        ];
        let scope = QueryScope {
            languages: vec!["rust".into(), "python".into()],
            ..Default::default()
        };
        let out = filter_scope(cands, &scope);
        let exts: Vec<_> = out
            .iter()
            .map(|c| c.path.extension().unwrap().to_str().unwrap().to_owned())
            .collect();
        assert!(exts.contains(&"rs".to_string()));
        assert!(exts.contains(&"py".to_string()));
        assert!(!exts.contains(&"ts".to_string()));
    }

    #[test]
    fn scope_symbols_drops_unsymboled_candidates() {
        let cands = vec![
            cand("a.rs", Some("auth::verify_token")),
            cand("b.rs", Some("user::User")),
            cand("c.rs", None),
        ];
        let scope = QueryScope {
            symbols: vec!["verify".into()],
            ..Default::default()
        };
        let out = filter_scope(cands, &scope);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].symbol.as_deref(), Some("auth::verify_token"));
    }

    #[test]
    fn fts5_expr_builds_or_clause_from_tokens() {
        let m = fts5_match_expr("how does verify_token work").unwrap();
        assert!(m.contains("\"how\""));
        assert!(m.contains("\"does\""));
        assert!(m.contains("\"verify_token\""));
        assert!(m.contains("\"work\""));
        assert!(m.contains(" OR "));
    }

    #[test]
    fn fts5_expr_is_none_when_all_tokens_too_short() {
        assert!(fts5_match_expr("a b c ?").is_none());
        assert!(fts5_match_expr("   ").is_none());
    }

    #[test]
    fn fts5_expr_strips_double_quotes() {
        let m = fts5_match_expr(r#"find "foobarbaz""#).unwrap();
        // `"foobarbaz"` in the input should become the single quoted token
        // `"foobarbaz"` in the output — i.e. the inner quotes are stripped.
        assert!(m.contains("\"foobarbaz\""));
    }
}
