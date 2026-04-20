//! Graph-backed enrichment for retrieved chunks.
//!
//! Given a [`Chunk`] that already has `path` + `symbol` populated, we pull
//! callers / callees / imports / siblings from the [`Graph`] and a
//! leading-comment docstring from the chunk's own body. The result is
//! stuffed into [`Chunk::context`] so renderers can surface it.
//!
//! Enrichment is intentionally **best-effort**:
//!
//! * An empty vector means "we looked and found nothing", not "we failed".
//! * A graph miss (unknown FQN, no edges) short-circuits silently — the
//!   chunk still renders, it just won't have a callers/callees section.
//! * Docstring extraction is language-aware but conservative; when in
//!   doubt we return `None` rather than misattributing unrelated comments.
//!
//! Why a separate module?
//!
//! The retriever's core responsibility is fusion + ranking; enrichment is
//! a read-only post-processing pass that only touches the top-K winners.
//! Keeping it here means `Retriever::recall_inner` stays legible, and we
//! can unit-test docstring rules without standing up a whole store.

use std::collections::HashSet;
use std::path::Path;

use thoth_core::{Chunk, ChunkContext, Result, SymbolRef};
use thoth_graph::{Graph, Node};

/// How many lines above a chunk's first line we scan for a leading
/// documentation block. Most real docstrings fit well inside this window;
/// going larger risks pulling in unrelated comments from a previous item.
const DOC_LOOKBACK_LINES: usize = 40;

/// Upper bound on how many callers / callees / siblings we surface per
/// chunk. Enough to be useful, small enough to keep prompts tight.
const FANOUT_LIMIT: usize = 8;

/// Depth for the caller/callee BFS. 1 is "direct"; we stop there so the
/// surfaced set stays a concrete, auditable shortlist rather than a
/// graph-wide blob.
const FANOUT_DEPTH: usize = 1;

/// Enrich every file-backed chunk in-place with graph context. Chunks
/// whose source is [`thoth_core::RetrievalSource::Markdown`] or
/// [`thoth_core::RetrievalSource::Episodic`] are left alone — there's
/// no file/symbol to enrich against.
pub async fn enrich_chunks(graph: &Graph, chunks: &mut [Chunk]) -> Result<()> {
    for chunk in chunks.iter_mut() {
        use thoth_core::RetrievalSource::*;
        match chunk.source {
            Symbol | Graph | FullText | Vector => {}
            Markdown | Episodic => continue,
        }
        let ctx = enrich_one(graph, chunk).await?;
        if !is_empty_context(&ctx) {
            chunk.context = Some(ctx);
        }
    }
    Ok(())
}

/// Build the [`ChunkContext`] for a single chunk. Any step can return
/// empty without failing the whole enrichment.
async fn enrich_one(graph: &Graph, chunk: &Chunk) -> Result<ChunkContext> {
    let mut ctx = ChunkContext::default();

    // Docstring — purely textual, no graph needed. Run first so we get
    // something even if the graph stages all miss. The chunk body starts
    // at the symbol's own declaration line, so doc comments that sit
    // *above* it (Rust `///`, C-style `/** */`, etc.) are usually *not*
    // in `chunk.body`. We peek backwards in the file for a lookback
    // window and prepend anything that looks like a doc block before
    // handing it to the language-aware extractor.
    let lookback = leading_doc_lookback(&chunk.path, chunk.line).await;
    let combined_body = match &lookback {
        Some(above) => format!("{above}{}", chunk.body),
        None => chunk.body.clone(),
    };
    ctx.doc = extract_docstring(&chunk.path, &combined_body);

    // Everything else wants an FQN. Without a symbol we still surface
    // siblings for the file (rare but useful for markdown-ish code).
    if let Some(fqn) = chunk.symbol.as_deref() {
        ctx.callers = collect_neighbors(graph, fqn, Direction::Callers).await?;
        ctx.callees = collect_neighbors(graph, fqn, Direction::Callees).await?;
    }

    // Siblings: other symbols declared in the same file. Deliberately
    // excludes the chunk's own symbol so the listing feels like
    // "neighbouring definitions" rather than a redundant echo.
    ctx.siblings = siblings(graph, &chunk.path, chunk.symbol.as_deref()).await?;

    // Imports are file-scoped (not symbol-scoped) — the whole file's
    // outgoing `Imports` edges, deduped.
    ctx.imports = graph.imports_of_file(&chunk.path).await.unwrap_or_default();

    Ok(ctx)
}

#[derive(Clone, Copy)]
enum Direction {
    Callers,
    Callees,
}

async fn collect_neighbors(graph: &Graph, fqn: &str, dir: Direction) -> Result<Vec<SymbolRef>> {
    let nodes = match dir {
        Direction::Callers => graph.callers(fqn, FANOUT_DEPTH).await?,
        Direction::Callees => graph.callees(fqn, FANOUT_DEPTH).await?,
    };
    Ok(nodes
        .into_iter()
        .take(FANOUT_LIMIT)
        .map(node_to_symbol_ref)
        .collect())
}

async fn siblings(graph: &Graph, path: &Path, own_fqn: Option<&str>) -> Result<Vec<SymbolRef>> {
    let mut out = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for n in graph.symbols_in_file(path).await? {
        if Some(n.fqn.as_str()) == own_fqn {
            continue;
        }
        if !seen.insert(n.fqn.clone()) {
            continue;
        }
        out.push(node_to_symbol_ref(n));
        if out.len() >= FANOUT_LIMIT {
            break;
        }
    }
    Ok(out)
}

fn node_to_symbol_ref(n: Node) -> SymbolRef {
    SymbolRef {
        fqn: n.fqn,
        path: if n.path.as_os_str().is_empty() {
            None
        } else {
            Some(n.path)
        },
        line: if n.line == 0 { None } else { Some(n.line) },
    }
}

fn is_empty_context(ctx: &ChunkContext) -> bool {
    ctx.callers.is_empty()
        && ctx.callees.is_empty()
        && ctx.imports.is_empty()
        && ctx.siblings.is_empty()
        && ctx.doc.is_none()
}

/// Peek at lines of `path` just above `start_line`, returning the
/// trailing run of comment-ish / blank lines as a single newline-
/// terminated string so the caller can prepend them to the chunk body.
///
/// The walk stops at the first real code line (or [`DOC_LOOKBACK_LINES`]
/// back, whichever comes first), which means we never pick up comments
/// belonging to a *previous* item. A "comment-ish" line is one that,
/// after trimming leading whitespace, starts with `/`, `*`, `#`, `'`,
/// `"`, or is blank — the union of the doc-block shapes we care about.
///
/// Returns `None` when the file can't be read, when `start_line <= 1`,
/// or when no doc-like lines precede the symbol.
async fn leading_doc_lookback(path: &Path, start_line: u32) -> Option<String> {
    if start_line <= 1 {
        return None;
    }
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let above: Vec<&str> = text
        .lines()
        .take((start_line as usize).saturating_sub(1))
        .collect();
    if above.is_empty() {
        return None;
    }

    // Walk backwards collecting comment-ish lines, bounded by the
    // lookback window, stopping at the first line of real code.
    let mut collected: Vec<&str> = Vec::new();
    for line in above.iter().rev().take(DOC_LOOKBACK_LINES) {
        if is_doc_ish(line) {
            collected.push(*line);
        } else {
            break;
        }
    }
    if collected.is_empty() {
        return None;
    }

    let mut out = String::new();
    for line in collected.iter().rev() {
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

/// Heuristic: does this line look like part of a doc / comment block
/// rather than real code? Intentionally permissive — false positives
/// are harmless (the language-aware extractor rejects non-matches) but
/// false negatives would cause us to miss docstrings entirely.
fn is_doc_ish(line: &str) -> bool {
    let t = line.trim_start();
    if t.is_empty() {
        return true;
    }
    matches!(
        t.as_bytes().first(),
        Some(b'/') | Some(b'*') | Some(b'#') | Some(b'\'') | Some(b'"')
    )
}

// ---------------------------------------------------------------------------
// Docstring extraction
// ---------------------------------------------------------------------------

/// Pull the leading documentation block from a chunk body. Dispatches on
/// file extension; unknown extensions return `None`.
///
/// The returned string has had comment markers stripped and leading
/// whitespace trimmed uniformly (keeping relative indentation). Trailing
/// blank lines are dropped. If no recognisable comment block is at the
/// top of the body we return `None` rather than guessing.
pub fn extract_docstring(path: &Path, body: &str) -> Option<String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    let lines: Vec<&str> = body.lines().collect();
    if lines.is_empty() {
        return None;
    }
    let raw = match ext.as_str() {
        "rs" => rust_doc(&lines),
        "py" | "pyi" => python_doc(&lines),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "java" | "c" | "cpp" | "cc" | "h" | "hpp" => {
            c_style_doc(&lines)
        }
        "go" => go_doc(&lines),
        _ => return None,
    }?;

    let collapsed = raw.trim_end_matches('\n').to_string();
    if collapsed.trim().is_empty() {
        None
    } else {
        Some(collapsed)
    }
}

/// Rust: `///` or `//!` lines at the top, optionally interleaved with
/// attribute lines (`#[...]`) which we skip over. Returns `None` if the
/// first non-attribute line isn't a doc comment.
fn rust_doc(lines: &[&str]) -> Option<String> {
    let mut out = String::new();
    let mut saw_doc = false;
    for line in lines {
        let t = line.trim_start();
        if t.starts_with("///") {
            saw_doc = true;
            out.push_str(strip_prefix_once(t, "///").trim_start());
            out.push('\n');
        } else if t.starts_with("//!") {
            saw_doc = true;
            out.push_str(strip_prefix_once(t, "//!").trim_start());
            out.push('\n');
        } else if t.starts_with("#[") || t.is_empty() {
            // Attributes or blank lines between doc blocks are skipped
            // silently as long as we haven't hit real code yet.
            if saw_doc {
                break;
            }
            continue;
        } else {
            break;
        }
    }
    if saw_doc { Some(out) } else { None }
}

/// Python: triple-quoted docstring on the first non-`def`/`class` line.
/// We accept both `"""..."""` and `'''...'''`, single or multi-line.
fn python_doc(lines: &[&str]) -> Option<String> {
    // Skip decorators, the `def foo(...):` / `class Foo:` header, plus
    // any `#` comments or blank lines the caller's lookback may have
    // prepended. We stop as soon as we see something that looks like
    // the function body proper.
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.is_empty()
            || t.starts_with('#')
            || t.starts_with('@')
            || t.starts_with("def ")
            || t.starts_with("class ")
        {
            i += 1;
            continue;
        }
        break;
    }
    if i >= lines.len() {
        return None;
    }
    let first = lines[i].trim();
    let (open, _close) = if first.starts_with("\"\"\"") {
        ("\"\"\"", "\"\"\"")
    } else if first.starts_with("'''") {
        ("'''", "'''")
    } else {
        return None;
    };

    // Single-line docstring: """text"""
    if first.len() >= 6 && first.ends_with(open) && !first[3..first.len() - 3].is_empty() {
        return Some(first[3..first.len() - 3].trim().to_string());
    }
    if first == open && i + 1 < lines.len() {
        // Multi-line: accumulate until the closing triple-quote.
        let mut out = String::new();
        for line in lines.iter().skip(i + 1) {
            if line.trim_end().ends_with(open) {
                let end = line.trim_end();
                let body = &end[..end.len() - 3];
                if !body.is_empty() {
                    out.push_str(body);
                    out.push('\n');
                }
                return Some(out.trim_end().to_string());
            }
            out.push_str(line);
            out.push('\n');
        }
    }
    None
}

/// C-family: leading `/** ... */` block or a run of `///`/`//`-prefixed
/// lines. JSDoc / TSDoc / JavaDoc all fall under this.
fn c_style_doc(lines: &[&str]) -> Option<String> {
    let mut out = String::new();
    let first = lines[0].trim_start();
    if first.starts_with("/**") {
        let mut closed = false;
        for (i, line) in lines.iter().enumerate() {
            let t = line.trim_start();
            let inner = if i == 0 {
                strip_prefix_once(t, "/**")
            } else {
                t
            };
            if let Some(idx) = inner.find("*/") {
                let head = &inner[..idx];
                append_c_line(&mut out, head);
                closed = true;
                break;
            }
            append_c_line(&mut out, inner);
        }
        if closed && !out.trim().is_empty() {
            return Some(out.trim_end().to_string());
        }
        return None;
    }
    // `//`-prefixed run.
    let mut saw = false;
    for line in lines {
        let t = line.trim_start();
        if t.starts_with("///") {
            saw = true;
            out.push_str(strip_prefix_once(t, "///").trim_start());
            out.push('\n');
        } else if t.starts_with("//") {
            saw = true;
            out.push_str(strip_prefix_once(t, "//").trim_start());
            out.push('\n');
        } else {
            // Either a non-comment line of real code, or a blank line
            // separating the doc block from what follows — either way,
            // stop rather than keep scanning.
            break;
        }
    }
    if saw {
        Some(out.trim_end().to_string())
    } else {
        None
    }
}

/// Strip leading `*` and at most one space, preserving further indent.
fn append_c_line(out: &mut String, s: &str) {
    let s = s.trim_end_matches('\r');
    let t = s.trim_start();
    let stripped = if let Some(rest) = t.strip_prefix('*') {
        rest.strip_prefix(' ').unwrap_or(rest)
    } else {
        t
    };
    out.push_str(stripped);
    out.push('\n');
}

/// Go: a run of `//` lines immediately preceding the declaration.
fn go_doc(lines: &[&str]) -> Option<String> {
    let mut out = String::new();
    let mut saw = false;
    for line in lines {
        let t = line.trim_start();
        if t.starts_with("//") {
            saw = true;
            out.push_str(strip_prefix_once(t, "//").trim_start());
            out.push('\n');
        } else if t.is_empty() && !saw {
            continue;
        } else {
            break;
        }
    }
    if saw {
        Some(out.trim_end().to_string())
    } else {
        None
    }
}

fn strip_prefix_once<'a>(s: &'a str, p: &str) -> &'a str {
    s.strip_prefix(p).unwrap_or(s)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn rust_triple_slash_doc_extracts_multiline() {
        let body = "\
/// First line.
/// Second line.
pub fn do_it() {}
";
        let got = extract_docstring(&p("a.rs"), body).unwrap();
        assert!(got.contains("First line."));
        assert!(got.contains("Second line."));
        assert!(!got.contains("pub fn"));
    }

    #[test]
    fn rust_inner_doc_comment() {
        let body = "\
//! Module docs.
//! More.
pub mod foo;
";
        let got = extract_docstring(&p("a.rs"), body).unwrap();
        assert!(got.contains("Module docs."));
        assert!(got.contains("More."));
    }

    #[test]
    fn rust_no_doc_returns_none() {
        let body = "pub fn do_it() {}\n";
        assert!(extract_docstring(&p("a.rs"), body).is_none());
    }

    #[test]
    fn rust_attrs_above_doc_are_tolerated() {
        let body = "\
#[inline]
/// Compute the thing.
pub fn do_it() {}
";
        let got = extract_docstring(&p("a.rs"), body).unwrap();
        assert_eq!(got.trim(), "Compute the thing.");
    }

    #[test]
    fn python_single_line_docstring() {
        let body = "\
def foo():
    \"\"\"Return the foo.\"\"\"
    return 1
";
        let got = extract_docstring(&p("a.py"), body).unwrap();
        assert_eq!(got, "Return the foo.");
    }

    #[test]
    fn python_multiline_docstring() {
        let body = "\
def foo():
    \"\"\"
    Return the foo.

    Longer description here.
    \"\"\"
    return 1
";
        let got = extract_docstring(&p("a.py"), body).unwrap();
        assert!(got.contains("Return the foo."));
        assert!(got.contains("Longer description here."));
    }

    #[test]
    fn typescript_jsdoc_block() {
        let body = "\
/**
 * Handle login.
 * Rejects on bad creds.
 */
export function login() {}
";
        let got = extract_docstring(&p("auth.ts"), body).unwrap();
        assert!(got.contains("Handle login."));
        assert!(got.contains("Rejects on bad creds."));
    }

    #[test]
    fn typescript_double_slash_run() {
        let body = "\
// Handle login.
// Second line.
export function login() {}
";
        let got = extract_docstring(&p("auth.ts"), body).unwrap();
        assert!(got.contains("Handle login."));
        assert!(got.contains("Second line."));
    }

    #[test]
    fn go_doc_comment_run() {
        let body = "\
// Login authenticates a user.
// Returns the session token.
func Login() {}
";
        let got = extract_docstring(&p("auth.go"), body).unwrap();
        assert!(got.contains("Login authenticates a user."));
        assert!(got.contains("Returns the session token."));
    }

    #[test]
    fn unknown_extension_yields_none() {
        let body = "/// whatever\nfn x() {}\n";
        assert!(extract_docstring(&p("a.weirdlang"), body).is_none());
    }

    #[test]
    fn unclosed_jsdoc_yields_none() {
        let body = "\
/**
 * never closes
 export function login() {}
";
        assert!(extract_docstring(&p("auth.ts"), body).is_none());
    }
}
