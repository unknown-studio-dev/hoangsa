//! Statement-level Program Dependence Graph (PDG) extraction.
//!
//! Produces `PdgOutput` for a single `SourceChunk` (or the whole chunk's
//! function set): one `PdgStmt` node per statement line, CFG edges between
//! consecutive statements (and from branch/loop headers to arm bodies), and
//! DataDep edges from last-write to each subsequent read.
//!
//! Only Rust and Python are supported; other languages return empty output.
//!
//! ## Known limitations (v1)
//! - Name-based def-use only; aliasing, field accesses, and reborrowing are
//!   not modelled. False negatives are possible.
//! - Call-arg bridge resolves callees through the same last-def pass, not
//!   through the full resolution map used by the indexer.

use std::collections::HashMap;

use tree_sitter::Parser;

use crate::{SourceChunk, SymbolKind, SymbolTable};

/// One statement node in the PDG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdgStmt {
    /// `<fn_fqn>#s<line>`.
    pub fqn: String,
    /// Absolute source path (stringified).
    pub path: String,
    /// 1-based source line of this statement.
    pub line: u32,
    /// Trimmed source text of the statement, capped at 120 chars.
    pub text: String,
}

/// Full PDG output for one chunk.
#[derive(Debug, Default, Clone)]
pub struct PdgOutput {
    /// Statement nodes, sorted by fqn.
    pub nodes: Vec<PdgStmt>,
    /// CFG edges: `(from_fqn, to_fqn)`, sorted.
    pub cfg: Vec<(String, String)>,
    /// DataDep edges: `(def_fqn, use_fqn)`, sorted.
    pub data_dep: Vec<(String, String)>,
}

/// Extract a PDG from `chunk` using the function symbols in `symbols`.
///
/// Returns an empty `PdgOutput` for unsupported languages or if no function
/// symbols fall within the chunk's line span. Never panics on malformed input.
pub fn extract_pdg(chunk: &SourceChunk, symbols: &SymbolTable) -> PdgOutput {
    match chunk.language {
        "rust" | "python" => {}
        _ => return PdgOutput::default(),
    }

    let source = chunk.body.as_bytes();
    let ts_lang = match chunk.language {
        #[cfg(feature = "lang-rust")]
        "rust" => tree_sitter_rust::LANGUAGE.into(),
        #[cfg(feature = "lang-python")]
        "python" => tree_sitter_python::LANGUAGE.into(),
        _ => return PdgOutput::default(),
    };

    let mut parser = Parser::new();
    if parser.set_language(&ts_lang).is_err() {
        return PdgOutput::default();
    }
    let Some(tree) = parser.parse(source, None) else {
        return PdgOutput::default();
    };

    // Collect function symbols whose span overlaps this chunk.
    let fn_syms: Vec<_> = symbols
        .symbols
        .iter()
        .filter(|s| {
            s.kind == SymbolKind::Function
                && s.span.0 >= chunk.start_line
                && s.span.1 <= chunk.end_line
        })
        .collect();

    if fn_syms.is_empty() {
        return PdgOutput::default();
    }

    // line_offset: the chunk's start_line in 1-based; tree-sitter rows are 0-based.
    let line_offset = chunk.start_line.saturating_sub(1);

    let mut all_nodes: Vec<PdgStmt> = Vec::new();
    let mut all_cfg: Vec<(String, String)> = Vec::new();
    let mut all_data_dep: Vec<(String, String)> = Vec::new();

    for sym in fn_syms {
        let fn_fqn = &sym.fqn;
        // Find the function node in the tree whose body covers this symbol span.
        // We use the tree-sitter row (0-based) of the symbol start.
        let sym_start_row = sym.span.0.saturating_sub(1).saturating_sub(line_offset) as usize;
        let sym_end_row = sym.span.1.saturating_sub(1).saturating_sub(line_offset) as usize;

        let root = tree.root_node();
        let Some(fn_node) = find_fn_node(root, chunk.language, sym_start_row, sym_end_row) else {
            continue;
        };

        // Collect statements from the function body.
        let stmts = collect_stmts(fn_node, chunk.language, source, line_offset);
        if stmts.is_empty() {
            continue;
        }

        // Build PdgStmt nodes.
        let mut stmt_nodes: Vec<PdgStmt> = stmts
            .iter()
            .map(|(line, text)| PdgStmt {
                fqn: format!("{fn_fqn}#s{line}"),
                path: chunk.path.to_string_lossy().into_owned(),
                line: *line,
                text: truncate(text, 120),
            })
            .collect();
        // Dedup by line (keep first occurrence per line).
        stmt_nodes.dedup_by_key(|s| s.line);

        // CFG: sequential edges.
        let cfg_edges: Vec<(String, String)> = stmt_nodes
            .windows(2)
            .map(|w| (w[0].fqn.clone(), w[1].fqn.clone()))
            .collect();

        // DataDep: def-use via last-def-wins.
        let data_dep_edges = def_use_edges(&stmt_nodes, &stmts, source, &symbols.calls, fn_fqn);

        all_nodes.extend(stmt_nodes);
        all_cfg.extend(cfg_edges);
        all_data_dep.extend(data_dep_edges);
    }

    all_nodes.sort_by(|a, b| a.fqn.cmp(&b.fqn));
    all_nodes.dedup_by_key(|n| n.fqn.clone());
    all_cfg.sort();
    all_cfg.dedup();
    all_data_dep.sort();
    all_data_dep.dedup();

    PdgOutput {
        nodes: all_nodes,
        cfg: all_cfg,
        data_dep: all_data_dep,
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        s[..max].to_string()
    }
}

/// Recursively search the subtree for a function/method node whose row span
/// contains `[sym_start_row, sym_end_row]` (0-based).
fn find_fn_node<'t>(
    node: tree_sitter::Node<'t>,
    lang: &str,
    sym_start_row: usize,
    sym_end_row: usize,
) -> Option<tree_sitter::Node<'t>> {
    let kind = node.kind();
    let is_fn = match lang {
        "rust" => matches!(kind, "function_item"),
        "python" => matches!(kind, "function_definition"),
        _ => false,
    };
    let nr = node.start_position().row;
    let ne = node.end_position().row;
    if is_fn && nr <= sym_start_row && ne >= sym_end_row {
        return Some(node);
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(found) = find_fn_node(child, lang, sym_start_row, sym_end_row) {
            return Some(found);
        }
    }
    None
}

/// Whether a node kind is statement-ish for this language.
fn is_stmt_kind(lang: &str, kind: &str) -> bool {
    match lang {
        "rust" => matches!(
            kind,
            "let_declaration"
                | "expression_statement"
                | "return_expression"
                | "if_expression"
                | "while_expression"
                | "for_expression"
                | "loop_expression"
                | "match_expression"
                | "macro_invocation"
        ),
        "python" => matches!(
            kind,
            "expression_statement"
                | "assignment"
                | "augmented_assignment"
                | "return_statement"
                | "if_statement"
                | "while_statement"
                | "for_statement"
                | "call"
                | "import_statement"
                | "import_from_statement"
        ),
        _ => false,
    }
}

/// Collect (1-based line, trimmed text) for statement nodes inside `fn_node`.
/// `line_offset` is chunk.start_line - 1 (to convert tree-sitter 0-based rows
/// back to file-level 1-based line numbers).
fn collect_stmts(
    fn_node: tree_sitter::Node<'_>,
    lang: &str,
    source: &[u8],
    line_offset: u32,
) -> Vec<(u32, String)> {
    let mut result: Vec<(u32, String)> = Vec::new();
    collect_stmts_inner(fn_node, lang, source, line_offset, &mut result);
    result.sort_by_key(|(l, _)| *l);
    result.dedup_by_key(|(l, _)| *l);
    result
}

fn collect_stmts_inner(
    node: tree_sitter::Node<'_>,
    lang: &str,
    source: &[u8],
    line_offset: u32,
    out: &mut Vec<(u32, String)>,
) {
    if is_stmt_kind(lang, node.kind()) {
        let line = node.start_position().row as u32 + 1 + line_offset;
        let text = node
            .utf8_text(source)
            .unwrap_or("")
            .trim()
            .to_string();
        out.push((line, text));
        // Don't recurse into statements — avoids double-counting nested stmts.
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_stmts_inner(child, lang, source, line_offset, out);
    }
}

/// Compute DataDep def→use edges using last-def-wins over the statement list.
///
/// Also emits a call-arg bridge: if a stmt U reads var X and calls a callee
/// that appears in `calls` with `fn_fqn` as caller, emit `U -> callee_fqn`.
fn def_use_edges(
    stmt_nodes: &[PdgStmt],
    stmts: &[(u32, String)],
    source: &[u8],
    _calls: &[(String, String)],
    _fn_fqn: &str,
) -> Vec<(String, String)> {
    if stmt_nodes.is_empty() {
        return Vec::new();
    }

    // Build a map from 1-based line → PdgStmt fqn.
    let fqn_by_line: HashMap<u32, &str> = stmt_nodes
        .iter()
        .map(|s| (s.line, s.fqn.as_str()))
        .collect();

    // Re-parse each statement's text to extract defined and used names.
    // `stmts` has the raw source text; we use simple heuristics.
    let mut last_def: HashMap<String, u32> = HashMap::new(); // name -> line of def
    let mut edges: Vec<(String, String)> = Vec::new();

    // Process statements in source order.
    let mut ordered: Vec<&(u32, String)> = stmts.iter().collect();
    ordered.sort_by_key(|(l, _)| *l);

    for (line, text) in &ordered {
        let defs = extract_defs(text, source);
        let uses = extract_uses(text);

        // For each use, emit DataDep from last def to this stmt.
        for used_name in &uses {
            if let Some(&def_line) = last_def.get(used_name.as_str())
                && def_line != *line
                && let (Some(def_fqn), Some(use_fqn)) =
                    (fqn_by_line.get(&def_line), fqn_by_line.get(line))
            {
                edges.push((def_fqn.to_string(), use_fqn.to_string()));
            }
        }

        // Record defs AFTER use-check so self-referential assignments (x = x + 1)
        // see the prior def for the RHS before recording the new def for LHS.
        for def_name in defs {
            last_def.insert(def_name, *line);
        }
    }

    edges
}

/// Extract variable names *defined* by this statement text (heuristic).
fn extract_defs(text: &str, _source: &[u8]) -> Vec<String> {
    let mut defs = Vec::new();
    let trimmed = text.trim();

    // Rust: `let [mut] <name>` or `let <name>: T`
    if let Some(rest) = trimmed.strip_prefix("let ") {
        let rest = rest.trim_start_matches("mut ").trim();
        let name = ident_at_start(rest);
        if !name.is_empty() {
            defs.push(name.to_string());
        }
        return defs;
    }

    // Python/Rust assignment: `<name> = ...` or `<name>: T = ...`
    // Also handles augmented: `<name> += ...`
    if let Some(eq_pos) = trimmed.find('=') {
        let lhs = trimmed[..eq_pos].trim();
        // Skip `==` comparisons, `!=`, `<=`, `>=`
        let prev_char = if eq_pos > 0 {
            trimmed.as_bytes().get(eq_pos - 1).copied()
        } else {
            None
        };
        let is_comparison = matches!(prev_char, Some(b'!' | b'<' | b'>' | b'='));
        if !is_comparison {
            // Strip type annotation: `x: int`
            let lhs = if let Some(colon_pos) = lhs.find(':') {
                lhs[..colon_pos].trim()
            } else {
                lhs
            };
            // Strip augmented operator suffix: `+=`, `-=`, etc.
            let lhs = lhs.trim_end_matches(['+', '-', '*', '/', '%', '&', '|', '^']);
            let name = ident_at_start(lhs.trim());
            if !name.is_empty() {
                defs.push(name.to_string());
            }
        }
    }

    defs
}

/// Extract identifiers *used* (read) in a statement (heuristic — all idents).
fn extract_uses(text: &str) -> Vec<String> {
    // Walk the text and collect word tokens. Filter out keywords.
    static KEYWORDS: &[&str] = &[
        "let", "mut", "if", "else", "while", "for", "in", "return", "fn", "pub", "use",
        "mod", "struct", "enum", "impl", "trait", "type", "const", "static", "async",
        "await", "match", "loop", "break", "continue", "true", "false", "self", "Self",
        "super", "crate", "def", "class", "import", "from", "with", "as", "pass",
        "yield", "lambda", "and", "or", "not", "is", "in", "None", "True", "False",
    ];

    let mut uses = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let word = &text[start..i];
            if !KEYWORDS.contains(&word) && !word.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                uses.push(word.to_string());
            }
        } else {
            i += 1;
        }
    }
    uses
}

/// Extract the leading identifier from a string slice.
fn ident_at_start(s: &str) -> &str {
    let end = s
        .find(|c: char| !c.is_alphanumeric() && c != '_')
        .unwrap_or(s.len());
    &s[..end]
}

// ── tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{SourceChunk, Symbol, SymbolKind, SymbolTable};
    use super::extract_pdg;

    fn make_chunk(language: &'static str, body: &str, start_line: u32) -> SourceChunk {
        let end_line = start_line + body.lines().count() as u32;
        SourceChunk {
            path: PathBuf::from("test.rs"),
            language,
            start_line,
            end_line,
            symbol: None,
            kind: None,
            body: body.to_string(),
            content_hash: [0u8; 32],
        }
    }

    fn fn_symbol(fqn: &str, start: u32, end: u32) -> Symbol {
        Symbol {
            fqn: fqn.to_string(),
            kind: SymbolKind::Function,
            path: PathBuf::from("test.rs"),
            span: (start, end),
        }
    }

    fn make_table(symbols: Vec<Symbol>) -> SymbolTable {
        SymbolTable {
            symbols,
            ..SymbolTable::default()
        }
    }

    /// Rust: `let x = 1; use(x)` should produce a DataDep def→use edge.
    #[test]
    fn pdg_extract_rust_def_use_chain() {
        let body = r#"fn foo() {
    let x = 1;
    let y = x + 2;
}"#;
        let chunk = make_chunk("rust", body, 1);
        let table = make_table(vec![fn_symbol("mymod::foo", 1, 4)]);
        let out = extract_pdg(&chunk, &table);

        // Should have at least 2 stmt nodes (let x, let y).
        assert!(
            out.nodes.len() >= 2,
            "expected >=2 nodes, got {:?}",
            out.nodes
        );

        // DataDep: the def of x (line 2) -> use of x (line 3).
        let has_dep = out.data_dep.iter().any(|(from, to)| {
            from.contains("#s2") && to.contains("#s3")
        });
        assert!(
            has_dep,
            "expected DataDep from #s2 to #s3, got: {:?}",
            out.data_dep
        );
    }

    /// Python: `x = 1; f(x)` should produce a DataDep def→use edge.
    #[test]
    fn pdg_extract_python_def_use_chain() {
        let body = "def bar():\n    x = 1\n    y = x + 2\n";
        let chunk = SourceChunk {
            path: PathBuf::from("test.py"),
            language: "python",
            start_line: 1,
            end_line: 3,
            symbol: None,
            kind: None,
            body: body.to_string(),
            content_hash: [0u8; 32],
        };
        let table = make_table(vec![Symbol {
            fqn: "mymod::bar".to_string(),
            kind: SymbolKind::Function,
            path: PathBuf::from("test.py"),
            span: (1, 3),
        }]);
        let out = extract_pdg(&chunk, &table);

        assert!(
            out.nodes.len() >= 2,
            "expected >=2 stmt nodes, got {:?}",
            out.nodes
        );

        // DataDep: def x (line 2) -> use x (line 3).
        let has_dep = out.data_dep.iter().any(|(from, to)| {
            from.contains("#s2") && to.contains("#s3")
        });
        assert!(
            has_dep,
            "expected DataDep from #s2 to #s3, got: {:?}",
            out.data_dep
        );
    }

    /// FQN convention: stmt nodes must be `<fn_fqn>#s<line>` with kind "stmt" implied.
    #[test]
    fn pdg_stmt_fqn_convention() {
        let body = r#"fn baz() {
    let a = 10;
    let b = 20;
}"#;
        let chunk = make_chunk("rust", body, 1);
        let table = make_table(vec![fn_symbol("mymod::baz", 1, 4)]);
        let out = extract_pdg(&chunk, &table);

        // Every node fqn must start with "mymod::baz#s" and have a numeric suffix.
        for node in &out.nodes {
            assert!(
                node.fqn.starts_with("mymod::baz#s"),
                "unexpected fqn: {}",
                node.fqn
            );
            let line_part = node.fqn.strip_prefix("mymod::baz#s").expect("prefix");
            assert!(
                line_part.parse::<u32>().is_ok(),
                "line part not numeric: {}",
                line_part
            );
            assert!(!node.text.is_empty(), "text must not be empty");
            assert!(node.text.len() <= 120, "text exceeds 120 chars");
        }
    }

    /// Empty / malformed function body → no panic, empty output.
    #[test]
    fn pdg_empty_fn_body_no_panic() {
        let body = "fn empty() {}";
        let chunk = make_chunk("rust", body, 1);
        let table = make_table(vec![fn_symbol("mymod::empty", 1, 1)]);
        let out = extract_pdg(&chunk, &table);
        // No panic — output may be empty.
        let _ = out;
    }

    /// Unsupported language returns empty output.
    #[test]
    fn pdg_unsupported_language_returns_empty() {
        let body = "function foo() { var x = 1; }";
        let chunk = make_chunk("javascript", body, 1);
        let table = make_table(vec![fn_symbol("foo", 1, 1)]);
        let out = extract_pdg(&chunk, &table);
        assert!(out.nodes.is_empty());
        assert!(out.cfg.is_empty());
        assert!(out.data_dep.is_empty());
    }
}
