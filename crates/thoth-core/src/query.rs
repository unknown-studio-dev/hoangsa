//! Query and retrieval types.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// What the caller wants Thoth to find.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    /// The natural-language or keyword text of the query.
    pub text: String,
    /// Optional scope filters.
    #[serde(default)]
    pub scope: QueryScope,
    /// How many chunks to return at most.
    #[serde(default = "default_top_k")]
    pub top_k: usize,
}

fn default_top_k() -> usize {
    12
}

/// Filters constraining the retrieval.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueryScope {
    /// Restrict to these path prefixes.
    #[serde(default)]
    pub paths: Vec<PathBuf>,
    /// Restrict to these languages (tree-sitter names).
    #[serde(default)]
    pub languages: Vec<String>,
    /// Restrict to these symbol names.
    #[serde(default)]
    pub symbols: Vec<String>,
    /// Filter facts/lessons to those matching any of these tags.
    /// Empty means "all tags" (no filter).
    #[serde(default)]
    pub tags: Vec<String>,
}

impl Query {
    /// Shorthand for building a text-only query with default settings.
    pub fn text(s: impl Into<String>) -> Self {
        Self {
            text: s.into(),
            scope: QueryScope::default(),
            top_k: default_top_k(),
        }
    }
}

/// The result of a recall call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retrieval {
    /// Ranked chunks.
    pub chunks: Vec<Chunk>,
    /// Optional LLM-synthesized answer (Mode::Full only).
    pub synthesized: Option<String>,
    /// The correlation id to record outcomes against later.
    pub correlation_id: uuid::Uuid,
}

/// A single retrieved piece of context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    /// Stable id (content hash).
    pub id: String,
    /// Path the chunk originated from.
    pub path: PathBuf,
    /// 1-based starting line.
    pub line: u32,
    /// Line span `(start, end)`.
    pub span: (u32, u32),
    /// Enclosing symbol (e.g. function/class name), if known.
    pub symbol: Option<String>,
    /// Short human-readable preview (trimmed to ~1–3 lines).
    pub preview: String,
    /// Full chunk body.
    pub body: String,
    /// Retrieval score (higher is better).
    pub score: f32,
    /// How the chunk was found.
    pub source: RetrievalSource,
    /// Graph-backed context (callers, callees, imports, siblings,
    /// docstring). Populated by the retriever's enrichment pass when
    /// [`Query::enrich`] is set. `None` on legacy responses and in
    /// Mode::Zero when enrichment is disabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<ChunkContext>,
}

/// Graph-backed context around a chunk. This is what makes Thoth's
/// recall output qualitatively different from a plain RAG hit: instead
/// of just the matched code we also surface who calls it, what it calls,
/// and its neighbours in the module.
///
/// All fields are best-effort — they reflect whatever the parser +
/// graph currently know about. Empty vectors mean "we looked and
/// found nothing", `None` on `doc` means "no leading comment block
/// detected".
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChunkContext {
    /// Symbols that reference / call this chunk's symbol (incoming edges).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub callers: Vec<SymbolRef>,
    /// Symbols that this chunk's symbol references / calls (outgoing edges).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub callees: Vec<SymbolRef>,
    /// Imports declared at the top of the same file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<String>,
    /// Other symbols defined in the same file (for a quick sense of the
    /// surrounding module layout). Excludes the chunk's own symbol.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub siblings: Vec<SymbolRef>,
    /// Leading documentation comment extracted from the chunk body,
    /// stripped of comment markers. E.g. for Rust we collect the `///`
    /// lines that precede the item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub doc: Option<String>,
}

/// A lightweight reference to another symbol in the graph.
///
/// This is deliberately flat and cheap to serialize — it exists so
/// enrichment lookups can surface "callers: foo::bar at path:123"
/// without forcing consumers to understand the full graph schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRef {
    /// Fully-qualified symbol name (e.g. `crate::module::fn_name`).
    pub fqn: String,
    /// File the symbol is defined in, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    /// 1-based line the symbol starts on, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Provenance of a retrieved chunk.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalSource {
    /// Exact symbol lookup.
    Symbol,
    /// Graph traversal (callers, callees, imports, ...).
    Graph,
    /// Full-text / BM25 match via tantivy.
    FullText,
    /// Vector similarity via LanceDB (Mode::Full only).
    Vector,
    /// Text from a markdown memory file (MEMORY.md / LESSONS.md).
    Markdown,
    /// Hit from the episodic log (past queries/answers/outcomes).
    Episodic,
}

/// Render-time budget applied to [`Retrieval::render_with`] and
/// [`Chunk::render_into_with`].
///
/// Separated from [`Query`] because budgets are a pure presentation concern
/// — the underlying `Retrieval` always carries complete chunk bodies so
/// callers that want structured JSON (CLI `--json`, MCP `data`) see the
/// full data. Only the text surface gets capped.
///
/// Defaults are tuned for agent-facing output where context tokens are
/// precious: 200 body lines per chunk, 32 KiB total. Change via the
/// `[output]` table in `config.toml` (see `thoth_retrieve::OutputConfig`).
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Maximum body lines rendered per chunk. Excess lines are replaced
    /// with a single `[… truncated, M more lines. Read <path>:L<a>-L<b>
    /// for full body]` marker pointing at the full range on disk.
    ///
    /// `0` disables body truncation (renders the full body always).
    pub max_body_lines: usize,
    /// Soft cap on the rendered output size in bytes. Enforced between
    /// chunks — a chunk already in progress finishes rendering, but no
    /// new chunk starts once the cap is crossed. Remaining chunks are
    /// elided with a footer.
    ///
    /// `0` disables the size budget (renders every chunk always).
    pub max_total_bytes: usize,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            max_body_lines: 200,
            max_total_bytes: 32 * 1024,
        }
    }
}

impl RenderOptions {
    /// Opt out of both caps. Mostly for tests that assert on the full
    /// body or that mock wide retrievals.
    pub fn unlimited() -> Self {
        Self {
            max_body_lines: 0,
            max_total_bytes: 0,
        }
    }
}

impl Retrieval {
    /// Render this retrieval as the human-readable text surface that both
    /// the CLI (`thoth query`) and MCP (`thoth_recall` tool text) display.
    ///
    /// The format is designed so an agent reading the output can answer
    /// three questions per chunk without opening the file:
    ///
    /// * **Where does this live?** — symbol, path, line span, score.
    /// * **What does it do?** — leading docstring, then the full body.
    /// * **What's around it?** — callers, callees, imports, siblings.
    ///
    /// Empty sections are omitted rather than shown as `(none)` so the
    /// common case (a rich hit) stays compact.
    ///
    /// Uses [`RenderOptions::default`]. Call [`Self::render_with`] to
    /// override the per-chunk / total-size budgets.
    pub fn render(&self) -> String {
        self.render_with(&RenderOptions::default())
    }

    /// Like [`Self::render`] but honours `opts`. See [`RenderOptions`].
    pub fn render_with(&self, opts: &RenderOptions) -> String {
        if self.chunks.is_empty() {
            return "(no matches — did you run thoth_index?)".to_string();
        }
        let mut out = String::new();
        for (i, c) in self.chunks.iter().enumerate() {
            // Soft budget: once we've exceeded max_total_bytes, stop
            // starting new chunks. A chunk already in progress below
            // isn't interrupted — the cap is about bounding amplification,
            // not chopping mid-line. `i` doubles as the count of chunks
            // rendered before this check fires.
            if opts.max_total_bytes > 0 && out.len() >= opts.max_total_bytes {
                let dropped = self.chunks.len() - i;
                out.push_str(&format!(
                    "\n[… output budget exhausted at {} bytes after {} chunk(s). \
                     {} more chunk(s) dropped — narrow the query, lower top_k, \
                     or raise `output.max_total_bytes` in config.toml]\n",
                    out.len(),
                    i,
                    dropped,
                ));
                break;
            }
            c.render_into_with(&mut out, i, opts);
        }
        if let Some(answer) = &self.synthesized {
            out.push_str("\n─── synthesized ───\n");
            out.push_str(answer);
            out.push('\n');
        }
        out
    }
}

impl Chunk {
    /// Append a human-readable rendering of this chunk to `out`.
    ///
    /// Shared by [`Retrieval::render`] and the CLI; extracted so each
    /// consumer gets consistent formatting. Uses [`RenderOptions::default`]
    /// — call [`Self::render_into_with`] to override.
    pub fn render_into(&self, out: &mut String, index: usize) {
        self.render_into_with(out, index, &RenderOptions::default());
    }

    /// Like [`Self::render_into`] but honours `opts`. Currently only
    /// `max_body_lines` affects the per-chunk rendering; the total-size
    /// budget is enforced by [`Retrieval::render_with`] between chunks.
    pub fn render_into_with(&self, out: &mut String, index: usize, opts: &RenderOptions) {
        let sym = self.symbol.as_deref().unwrap_or("-");
        out.push_str(&format!(
            "\n[{index:>2}] score={:.4} src={:?}  {}  {}:{}-{}\n",
            self.score,
            self.source,
            sym,
            self.path.display(),
            self.span.0,
            self.span.1
        ));

        if let Some(ctx) = &self.context
            && let Some(doc) = &ctx.doc
        {
            // Indent the doc block so it's visually distinct from the
            // body. Trim once so we don't double-indent multi-line docs.
            for line in doc.lines() {
                out.push_str("  │ ");
                out.push_str(line);
                out.push('\n');
            }
        }

        // Body with real newlines. We indent two spaces so the chunk
        // header/sections stand out in scrollback. Cap at
        // `opts.max_body_lines` to bound the context cost of a single
        // huge function — the elided tail is recoverable via Read on
        // the printed line range.
        if !self.body.is_empty() {
            let cap = opts.max_body_lines;
            let total = self.body.lines().count();
            let rendered_lines = if cap == 0 { total } else { total.min(cap) };
            for line in self.body.lines().take(rendered_lines) {
                out.push_str("    ");
                out.push_str(line);
                out.push('\n');
            }
            if total > rendered_lines {
                let dropped = total - rendered_lines;
                // Line of the first omitted body line, 1-based. `span.0`
                // is the chunk's first line; we've rendered `rendered_lines`
                // body lines, so the next on-disk line is span.0 + N.
                let resume_line = self.span.0.saturating_add(rendered_lines as u32);
                out.push_str(&format!(
                    "    [… truncated, {dropped} more line(s). Read {}:L{}-L{} for full body]\n",
                    self.path.display(),
                    resume_line,
                    self.span.1,
                ));
            }
        } else if !self.preview.is_empty() {
            // Fallback: markdown/episodic hits don't have a body — show
            // the preview instead.
            out.push_str("     ");
            out.push_str(&self.preview);
            out.push('\n');
        }

        if let Some(ctx) = &self.context {
            render_symbol_row(out, "↑ callers", &ctx.callers);
            render_symbol_row(out, "↓ calls  ", &ctx.callees);
            render_symbol_row(out, "↔ siblings", &ctx.siblings);
            render_string_row(out, "📦 imports", &ctx.imports);
        }
    }
}

fn render_symbol_row(out: &mut String, label: &str, refs: &[SymbolRef]) {
    if refs.is_empty() {
        return;
    }
    let items: Vec<String> = refs
        .iter()
        .map(|r| match (&r.path, r.line) {
            (Some(p), Some(l)) => format!("{} @ {}:{}", r.fqn, p.display(), l),
            (Some(p), None) => format!("{} @ {}", r.fqn, p.display()),
            _ => r.fqn.clone(),
        })
        .collect();
    out.push_str(&format!("  {label}: {}\n", items.join(", ")));
}

fn render_string_row(out: &mut String, label: &str, items: &[String]) {
    if items.is_empty() {
        return;
    }
    out.push_str(&format!("  {label}: {}\n", items.join(", ")));
}

/// A citation pointing at a chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    /// Chunk id.
    pub chunk_id: String,
    /// Path.
    pub path: PathBuf,
    /// Starting line.
    pub line: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk_with_body(lines: usize, span_start: u32) -> Chunk {
        let body: String = (1..=lines)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        Chunk {
            id: "h".into(),
            path: PathBuf::from("src/lib.rs"),
            line: span_start,
            span: (span_start, span_start + lines as u32 - 1),
            symbol: Some("foo".into()),
            preview: String::new(),
            body,
            score: 1.0,
            source: RetrievalSource::Symbol,
            context: None,
        }
    }

    #[test]
    fn render_truncates_body_beyond_max_body_lines() {
        let c = chunk_with_body(300, 10);
        let mut out = String::new();
        c.render_into_with(
            &mut out,
            0,
            &RenderOptions {
                max_body_lines: 50,
                max_total_bytes: 0,
            },
        );
        // First 50 lines present, line 51 absent.
        assert!(out.contains("line 1\n"));
        assert!(out.contains("line 50\n"));
        assert!(!out.contains("line 51\n"));
        // Marker points at the on-disk range to recover the tail.
        assert!(
            out.contains("truncated, 250 more line(s)"),
            "expected truncation marker in: {out}"
        );
        // Resume line is span.0 + rendered_lines = 10 + 50 = 60.
        assert!(
            out.contains("Read src/lib.rs:L60-L309"),
            "expected range pointer in: {out}"
        );
    }

    #[test]
    fn render_preserves_full_body_when_under_cap_or_unlimited() {
        let c = chunk_with_body(30, 1);

        // Under cap: no marker.
        let mut out = String::new();
        c.render_into_with(
            &mut out,
            0,
            &RenderOptions {
                max_body_lines: 100,
                max_total_bytes: 0,
            },
        );
        assert!(!out.contains("truncated"));
        assert!(out.contains("line 30\n"));

        // Unlimited: 0 disables the cap even on a large body.
        let big = chunk_with_body(500, 1);
        let mut out = String::new();
        big.render_into_with(&mut out, 0, &RenderOptions::unlimited());
        assert!(!out.contains("truncated"));
        assert!(out.contains("line 500\n"));
    }

    #[test]
    fn render_with_drops_tail_chunks_past_byte_budget() {
        // Ten chunks, each well-formed but chunky. A 1KiB budget should
        // cut off well before chunk 9.
        let chunks: Vec<Chunk> = (0..10)
            .map(|i| chunk_with_body(80, (i * 100 + 1) as u32))
            .collect();
        let retrieval = Retrieval {
            chunks,
            synthesized: None,
            correlation_id: uuid::Uuid::nil(),
        };
        let rendered = retrieval.render_with(&RenderOptions {
            max_body_lines: 0,
            max_total_bytes: 1024,
        });
        assert!(
            rendered.contains("output budget exhausted"),
            "expected budget footer in: {rendered}"
        );
        // Footer advertises how many chunks were dropped.
        assert!(
            rendered.contains("more chunk(s) dropped"),
            "expected dropped-count in: {rendered}"
        );
    }

    #[test]
    fn render_with_emits_every_chunk_when_budget_disabled() {
        let chunks: Vec<Chunk> = (0..5)
            .map(|i| chunk_with_body(5, (i * 10 + 1) as u32))
            .collect();
        let retrieval = Retrieval {
            chunks,
            synthesized: None,
            correlation_id: uuid::Uuid::nil(),
        };
        let rendered = retrieval.render_with(&RenderOptions::unlimited());
        assert!(!rendered.contains("output budget exhausted"));
        // All five chunk headers rendered (one per chunk).
        let header_count = rendered.matches("score=1.0000").count();
        assert_eq!(header_count, 5, "unexpected header count in: {rendered}");
    }

    #[test]
    fn render_on_empty_chunks_returns_no_matches_hint() {
        let retrieval = Retrieval {
            chunks: vec![],
            synthesized: None,
            correlation_id: uuid::Uuid::nil(),
        };
        assert_eq!(
            retrieval.render(),
            "(no matches — did you run thoth_index?)"
        );
    }
}
