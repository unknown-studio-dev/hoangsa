//! Language registry.
//!
//! Each enabled grammar is wrapped in a [`Language`] value that knows:
//!
//! - its canonical name
//! - the file extensions it claims
//! - the tree-sitter `Language` object
//! - which node kinds mark a symbol boundary and how to extract the name
//! - which node kinds are imports
//!
//! Everything is `Copy`/`Clone` so the registry can be moved between threads
//! cheaply.

use std::path::Path;

use crate::SymbolKind;

/// A registered language.
#[derive(Debug, Clone, Copy)]
pub struct Language {
    inner: LanguageKind,
}

#[derive(Debug, Clone, Copy)]
enum LanguageKind {
    #[cfg(feature = "lang-rust")]
    Rust,
    #[cfg(feature = "lang-python")]
    Python,
    #[cfg(feature = "lang-javascript")]
    JavaScript,
    #[cfg(feature = "lang-typescript")]
    TypeScript,
    #[cfg(feature = "lang-go")]
    Go,
    // Kept to ensure the enum is non-empty even with every feature disabled.
    #[allow(dead_code)]
    _Unreachable,
}

impl Language {
    /// Canonical name.
    pub fn name(self) -> &'static str {
        match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => "rust",
            #[cfg(feature = "lang-python")]
            LanguageKind::Python => "python",
            #[cfg(feature = "lang-javascript")]
            LanguageKind::JavaScript => "javascript",
            #[cfg(feature = "lang-typescript")]
            LanguageKind::TypeScript => "typescript",
            #[cfg(feature = "lang-go")]
            LanguageKind::Go => "go",
            LanguageKind::_Unreachable => unreachable!(),
        }
    }

    /// Underlying tree-sitter grammar.
    ///
    /// tree-sitter 0.23 grammar crates expose a `LANGUAGE: LanguageFn`
    /// constant; `.into()` yields a `tree_sitter::Language`.
    pub fn tree_sitter(self) -> tree_sitter::Language {
        match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => tree_sitter_rust::LANGUAGE.into(),
            #[cfg(feature = "lang-python")]
            LanguageKind::Python => tree_sitter_python::LANGUAGE.into(),
            #[cfg(feature = "lang-javascript")]
            LanguageKind::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
            #[cfg(feature = "lang-typescript")]
            LanguageKind::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            #[cfg(feature = "lang-go")]
            LanguageKind::Go => tree_sitter_go::LANGUAGE.into(),
            LanguageKind::_Unreachable => unreachable!(),
        }
    }

    /// Map a tree-sitter node kind to a [`SymbolKind`] iff this node should
    /// become a chunk boundary.
    pub(crate) fn symbol_kind_for(self, node_kind: &str) -> Option<SymbolKind> {
        match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => match node_kind {
                "function_item" | "function_signature_item" => Some(SymbolKind::Function),
                "impl_item" | "struct_item" | "enum_item" | "union_item" => Some(SymbolKind::Type),
                "trait_item" => Some(SymbolKind::Trait),
                "mod_item" => Some(SymbolKind::Module),
                "const_item" | "static_item" => Some(SymbolKind::Binding),
                _ => None,
            },
            #[cfg(feature = "lang-python")]
            LanguageKind::Python => match node_kind {
                "function_definition" => Some(SymbolKind::Function),
                "class_definition" => Some(SymbolKind::Type),
                _ => None,
            },
            #[cfg(feature = "lang-javascript")]
            LanguageKind::JavaScript => match node_kind {
                "function_declaration" | "method_definition" | "arrow_function" => {
                    Some(SymbolKind::Function)
                }
                "class_declaration" => Some(SymbolKind::Type),
                _ => None,
            },
            #[cfg(feature = "lang-typescript")]
            LanguageKind::TypeScript => match node_kind {
                "function_declaration" | "method_definition" => Some(SymbolKind::Function),
                "class_declaration" => Some(SymbolKind::Type),
                "interface_declaration" => Some(SymbolKind::Trait),
                _ => None,
            },
            #[cfg(feature = "lang-go")]
            LanguageKind::Go => match node_kind {
                "function_declaration" | "method_declaration" => Some(SymbolKind::Function),
                "type_declaration" => Some(SymbolKind::Type),
                _ => None,
            },
            LanguageKind::_Unreachable => None,
        }
    }

    /// Extract a human-readable name from a definition node.
    pub(crate) fn extract_name(self, node: tree_sitter::Node<'_>, source: &[u8]) -> Option<String> {
        // Rust `impl` blocks don't have a `name` field — their identity
        // is the *target* type under the `type` field. Named after the
        // target (`English` in `impl Greet for English`) so call graphs
        // can route method lookups to the type that actually provides
        // them. The supertrait (if any) is recorded separately by
        // `extract_extends`.
        #[cfg(feature = "lang-rust")]
        if matches!(self.inner, LanguageKind::Rust) && node.kind() == "impl_item" {
            if let Some(t) = node.child_by_field_name("type")
                && let Ok(text) = t.utf8_text(source)
            {
                return Some(strip_generics(text.trim()).to_string());
            }
            return None;
        }
        // Most grammars expose a `name` field on definitions.
        if let Some(name_node) = node.child_by_field_name("name")
            && let Ok(text) = name_node.utf8_text(source)
        {
            return Some(text.to_string());
        }
        // Fallback: scan named children for an identifier.
        let mut cursor = node.walk();
        for c in node.named_children(&mut cursor) {
            if (c.kind().contains("identifier") || c.kind() == "type_identifier")
                && let Ok(text) = c.utf8_text(source)
            {
                return Some(text.to_string());
            }
        }
        None
    }

    /// Whether this node kind represents an import / use.
    pub(crate) fn is_import_node(self, node_kind: &str) -> bool {
        match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => {
                matches!(node_kind, "use_declaration" | "extern_crate_declaration")
            }
            #[cfg(feature = "lang-python")]
            LanguageKind::Python => matches!(
                node_kind,
                "import_statement" | "import_from_statement" | "future_import_statement"
            ),
            #[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
            LanguageKind::JavaScript | LanguageKind::TypeScript => {
                matches!(node_kind, "import_statement")
            }
            #[cfg(feature = "lang-go")]
            LanguageKind::Go => matches!(node_kind, "import_declaration"),
            _ => false,
        }
    }

    /// Whether this node kind represents a call / invocation site.
    ///
    /// Includes macro invocations (Rust `foo!(...)`) since those are the way
    /// a lot of "real" cross-module references surface in Rust code.
    pub(crate) fn is_call_node(self, node_kind: &str) -> bool {
        match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => matches!(
                node_kind,
                "call_expression" | "method_call_expression" | "macro_invocation"
            ),
            #[cfg(feature = "lang-python")]
            LanguageKind::Python => node_kind == "call",
            #[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
            LanguageKind::JavaScript | LanguageKind::TypeScript => node_kind == "call_expression",
            #[cfg(feature = "lang-go")]
            LanguageKind::Go => node_kind == "call_expression",
            _ => false,
        }
    }

    /// Extract the callee's name from a call-site node.
    ///
    /// The returned string is whatever the grammar exposes as the "thing
    /// being called": for `foo()` it's `"foo"`; for `obj.bar()` it's
    /// `"bar"` (the last segment, since that's what the graph will match
    /// against `SymbolRow::fqn` simple names); for `a::b::c()` it's `"c"`;
    /// for `println!(..)` it's `"println"`.
    pub(crate) fn extract_callee(
        self,
        node: tree_sitter::Node<'_>,
        source: &[u8],
    ) -> Option<String> {
        let raw = match (self.inner, node.kind()) {
            #[cfg(feature = "lang-rust")]
            (LanguageKind::Rust, "call_expression") => node
                .child_by_field_name("function")?
                .utf8_text(source)
                .ok()?,
            #[cfg(feature = "lang-rust")]
            (LanguageKind::Rust, "method_call_expression") => {
                node.child_by_field_name("method")?.utf8_text(source).ok()?
            }
            #[cfg(feature = "lang-rust")]
            (LanguageKind::Rust, "macro_invocation") => {
                node.child_by_field_name("macro")?.utf8_text(source).ok()?
            }
            #[cfg(feature = "lang-python")]
            (LanguageKind::Python, "call") => node
                .child_by_field_name("function")?
                .utf8_text(source)
                .ok()?,
            #[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
            (LanguageKind::JavaScript | LanguageKind::TypeScript, "call_expression") => node
                .child_by_field_name("function")?
                .utf8_text(source)
                .ok()?,
            #[cfg(feature = "lang-go")]
            (LanguageKind::Go, "call_expression") => node
                .child_by_field_name("function")?
                .utf8_text(source)
                .ok()?,
            _ => return None,
        };
        Some(last_name_segment(raw))
    }
}

impl Language {
    /// Parse an import statement's source text and push `(local_name,
    /// resolved_target)` pairs into `out`. Grammar-agnostic (works off
    /// the raw text the walker already captured); never errors.
    ///
    /// For each supported language the mapping is:
    /// - **Rust** `use foo::Bar;` → `("Bar", "foo::Bar")`; `use foo::Bar as B;`
    ///   → `("B", "foo::Bar")`; group imports `use foo::{A, B as Bee};` fan out.
    /// - **Python** `import x` / `import x as y` / `from m import y[, z as zz]`
    ///   all emit appropriate pairs.
    /// - **TS/JS** `import def from 'm'` / `import * as ns from 'm'` /
    ///   `import { a, b as bb } from 'm'`. Side-effect `import 'm'` emits
    ///   nothing.
    /// - **Go** `import "path/to/pkg"` / `import alias "path/to/pkg"` /
    ///   grouped `import ( ... )` blocks.
    pub(crate) fn extract_import_aliases(self, text: &str, out: &mut Vec<(String, String)>) {
        match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => rust_aliases(text, out),
            #[cfg(feature = "lang-python")]
            LanguageKind::Python => python_aliases(text, out),
            #[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
            LanguageKind::JavaScript | LanguageKind::TypeScript => ts_js_aliases(text, out),
            #[cfg(feature = "lang-go")]
            LanguageKind::Go => go_aliases(text, out),
            _ => {}
        }
    }

    /// Extract the name of a type this node references, if the node is
    /// a type-reference leaf (Rust `type_identifier` / `scoped_type_identifier`,
    /// TS/JS `type_identifier`, Python `type` / `identifier` inside an
    /// annotation). Returns a bare / qualified name exactly as it
    /// appears in source — the indexer resolves it through the file's
    /// alias map before writing a `References` edge.
    ///
    /// Scope is deliberately narrow: only the leaves whose sole job is
    /// to name a type, not every `identifier` in the tree. Call sites
    /// (already captured as `Calls`), variable bindings, and literal
    /// strings are untouched. Yields `None` for unsupported kinds,
    /// which is the common case — callers guard with `is_some()`.
    pub(crate) fn extract_type_ref(
        self,
        node: tree_sitter::Node<'_>,
        source: &[u8],
    ) -> Option<String> {
        let kind = node.kind();
        let matches = match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => kind == "type_identifier" || kind == "scoped_type_identifier",
            #[cfg(feature = "lang-typescript")]
            LanguageKind::TypeScript => kind == "type_identifier",
            _ => false,
        };
        if !matches {
            return None;
        }
        let text = node.utf8_text(source).ok()?;
        let bare = strip_generics(text.trim()).to_string();
        if bare.is_empty() {
            return None;
        }
        Some(bare)
    }

    /// Extract the unresolved names of any parent types this definition
    /// extends / implements. Returns bare or qualified names exactly as
    /// they appear in source; the indexer resolves them through the
    /// file's alias map.
    ///
    /// Rules per language (on the definition node itself):
    /// - **Rust** `impl Trait for Type` → Type extends Trait; `trait Sub: Super1 + Super2`
    ///   → Sub extends Super1, Super2.
    /// - **Python** `class Foo(Bar, Baz, metaclass=Meta)` → Foo extends
    ///   Bar, Baz (keyword args ignored).
    /// - **TS/JS** `class X extends Y implements I1, I2` / `interface I extends A, B`.
    /// - **Go** struct embeddings — currently disabled (false positives on
    ///   non-type anonymous fields). Revisit when we have field-type
    ///   resolution.
    pub(crate) fn extract_extends(self, node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
        match self.inner {
            #[cfg(feature = "lang-rust")]
            LanguageKind::Rust => rust_extends(node, source),
            #[cfg(feature = "lang-python")]
            LanguageKind::Python => python_extends(node, source),
            #[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
            LanguageKind::JavaScript | LanguageKind::TypeScript => ts_js_extends(node, source),
            _ => Vec::new(),
        }
    }
}

// ------------------------------------------------------ alias extractors

#[cfg(feature = "lang-rust")]
fn rust_aliases(text: &str, out: &mut Vec<(String, String)>) {
    // Strip `use ` / `pub use ` / `extern crate ` preambles and a
    // trailing `;`. Rust parsers are complicated — tree-sitter would
    // give us a clean AST, but we're already re-parsing the raw text
    // below to flatten group imports, so a compact text scan pays off.
    let mut body = text.trim();
    for p in ["pub use ", "pub(crate) use ", "pub(super) use ", "use "] {
        if let Some(rest) = body.strip_prefix(p) {
            body = rest;
            break;
        }
    }
    if let Some(rest) = body.strip_prefix("extern crate ") {
        // `extern crate foo as bar;` → alias `bar` -> `foo`.
        let stripped = rest.trim_end_matches(';').trim();
        if let Some((orig, alias)) = split_as(stripped) {
            out.push((alias.to_string(), orig.to_string()));
        } else {
            out.push((stripped.to_string(), stripped.to_string()));
        }
        return;
    }
    let body = body.trim_end_matches(';').trim();
    emit_rust_use_tree("", body, out);
}

/// Recursive flattener for Rust use-trees. `prefix` is the accumulated
/// module path (e.g. `"std::sync"`); `tree` is the remaining right side
/// (`"Arc"`, `"{Arc, Mutex as M}"`, `"*"`, ...).
#[cfg(feature = "lang-rust")]
fn emit_rust_use_tree(prefix: &str, tree: &str, out: &mut Vec<(String, String)>) {
    let tree = tree.trim();
    if tree.is_empty() || tree == "*" {
        return;
    }
    // Group import: split on top-level commas inside the braces.
    if let Some(inner) = tree.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        for piece in split_top_level(inner, ',') {
            emit_rust_use_tree(prefix, piece.trim(), out);
        }
        return;
    }
    // `a::b::c` or `a::b::{...}`. Find the last `::` before any `{`.
    let split_point = tree.find('{').unwrap_or(tree.len());
    let head = &tree[..split_point];
    let braced = &tree[split_point..];
    if !braced.is_empty() {
        // head ends with `::`; strip it.
        let new_prefix = join_prefix(prefix, head.trim_end_matches("::"));
        emit_rust_use_tree(&new_prefix, braced, out);
        return;
    }
    // Leaf like `foo::bar` or `foo::bar as Baz`.
    let (path, alias) = match split_as(tree) {
        Some((p, a)) => (p, Some(a)),
        None => (tree, None),
    };
    // `self` inside a group means "the prefix itself" — `std::io::{self, Read}`.
    if path.trim() == "self" {
        if !prefix.is_empty() {
            let leaf = prefix.rsplit("::").next().unwrap_or(prefix).to_string();
            let name = alias.map(|s| s.to_string()).unwrap_or(leaf);
            out.push((name, prefix.to_string()));
        }
        return;
    }
    // Glob leaf (`use foo::*;` or `use foo::{bar::*};`) — nothing to alias.
    if path.trim().ends_with('*') {
        return;
    }
    let full = join_prefix(prefix, path.trim());
    let leaf = full.rsplit("::").next().unwrap_or(&full).to_string();
    let name = alias.map(|s| s.to_string()).unwrap_or(leaf);
    if !name.is_empty() && !full.is_empty() && name != "*" {
        out.push((name, full));
    }
}

#[cfg(feature = "lang-python")]
fn python_aliases(text: &str, out: &mut Vec<(String, String)>) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("from ") {
        // `from m import a, b as bb`.
        let (module, imports) = match rest.split_once(" import ") {
            Some(p) => p,
            None => return,
        };
        let module = module.trim();
        if module.is_empty() {
            return;
        }
        let imports = imports.trim().trim_start_matches('(').trim_end_matches(')');
        for piece in imports.split(',') {
            let piece = piece.trim();
            if piece.is_empty() || piece == "*" {
                continue;
            }
            let (name, alias) = match split_as(piece) {
                Some((n, a)) => (n, Some(a)),
                None => (piece, None),
            };
            let full = format!("{module}::{name}");
            let local = alias.unwrap_or(name).to_string();
            out.push((local, full));
        }
        return;
    }
    let rest = match text
        .strip_prefix("import ")
        .or_else(|| text.strip_prefix("from __future__ import "))
    {
        Some(r) => r,
        None => return,
    };
    for piece in rest.split(',') {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        let (module, alias) = match split_as(piece) {
            Some((m, a)) => (m, Some(a)),
            None => (piece, None),
        };
        // `import foo.bar` — the binding in the current scope is `foo`.
        let leaf = alias
            .map(|s| s.to_string())
            .unwrap_or_else(|| module.split('.').next().unwrap_or(module).to_string());
        out.push((leaf, module.to_string()));
    }
}

#[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
fn ts_js_aliases(text: &str, out: &mut Vec<(String, String)>) {
    // Accepts all of:
    //   import def from 'm';
    //   import * as ns from 'm';
    //   import { a, b as bb } from 'm';
    //   import def, { a } from 'm';
    //   import def, * as ns from 'm';
    //   import 'm';                            // side-effect only
    let text = text.trim().trim_end_matches(';');
    let body = match text.strip_prefix("import ") {
        Some(b) => b,
        None => return,
    };
    // Split into `clause` and `'module'` halves. `from` as a bare word is
    // only legal as the separator.
    let (clause, module) = match body.rfind(" from ") {
        Some(idx) => (body[..idx].trim(), body[idx + 6..].trim()),
        None => {
            // Side-effect-only import — no local bindings.
            return;
        }
    };
    let module = module
        .trim()
        .trim_matches(|c| c == '\'' || c == '"' || c == '`');
    if module.is_empty() {
        return;
    }
    // The clause may have shape `def`, `{ a, b }`, `* as ns`, or combinations
    // separated by a top-level comma (e.g. `def, { a }`).
    for part in split_top_level(clause, ',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(namespace) = part.strip_prefix("* as ") {
            // Whole module bound to a local name.
            out.push((namespace.trim().to_string(), module.to_string()));
        } else if let Some(inner) = part.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
            for spec in inner.split(',') {
                let spec = spec.trim();
                if spec.is_empty() {
                    continue;
                }
                let (orig, local) = match split_as(spec) {
                    Some((o, a)) => (o, a),
                    None => (spec, spec),
                };
                out.push((local.to_string(), format!("{module}::{orig}")));
            }
        } else {
            // Default import: `import foo from 'm'` → local `foo` -> `m::default`.
            out.push((part.to_string(), format!("{module}::default")));
        }
    }
}

#[cfg(feature = "lang-go")]
fn go_aliases(text: &str, out: &mut Vec<(String, String)>) {
    // `import "path/to/pkg"` | `import alias "path/to/pkg"` |
    // `import ( alias "path/to/a"\n   "path/to/b" )`.
    let text = text.trim();
    let body = match text.strip_prefix("import") {
        Some(b) => b.trim(),
        None => return,
    };
    let lines: Vec<&str> =
        if let Some(inner) = body.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
            inner
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .collect()
        } else {
            vec![body]
        };
    for line in lines {
        // Strip a trailing line comment if present.
        let line = line.split("//").next().unwrap_or(line).trim();
        if line.is_empty() {
            continue;
        }
        let (alias_part, path_part) = match line.rfind('"') {
            Some(end) => {
                let before_end = &line[..end];
                match before_end.rfind('"') {
                    Some(start) => (line[..start].trim(), &line[start + 1..end]),
                    None => continue,
                }
            }
            None => continue,
        };
        if path_part.is_empty() {
            continue;
        }
        let alias = if alias_part.is_empty() {
            // Default alias is the last path segment.
            path_part.rsplit('/').next().unwrap_or(path_part)
        } else {
            // Go alias-only modifiers — skip `_` (blank import, no binding)
            // and `.` (dot import brings all names into scope; too broad
            // to represent as a single alias).
            match alias_part {
                "_" | "." => continue,
                a => a,
            }
        };
        out.push((alias.to_string(), path_part.to_string()));
    }
}

// ------------------------------------------------------ extends extractors

#[cfg(feature = "lang-rust")]
fn rust_extends(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    match node.kind() {
        "impl_item" => {
            // `impl Trait for Type` — `trait` is the trait, `type` is the
            // target type. We want `Type extends Trait`, so the parent
            // name is the `trait` field.
            if let Some(t) = node.child_by_field_name("trait")
                && let Ok(text) = t.utf8_text(source)
            {
                out.push(strip_generics(text).to_string());
            }
        }
        "trait_item" => {
            // `trait Sub: Super1 + Super2 + 'a` — the supertrait list
            // lives under the `bounds` field (a `trait_bounds` node).
            if let Some(bounds) = node.child_by_field_name("bounds") {
                let mut cursor = bounds.walk();
                for child in bounds.named_children(&mut cursor) {
                    // Lifetime bounds (`'a`) are not supertraits.
                    if child.kind() == "lifetime" {
                        continue;
                    }
                    if let Ok(text) = child.utf8_text(source) {
                        let s = strip_generics(text.trim());
                        if !s.is_empty() && !s.starts_with('\'') {
                            out.push(s.to_string());
                        }
                    }
                }
            }
        }
        _ => {}
    }
    out
}

#[cfg(feature = "lang-python")]
fn python_extends(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    if node.kind() != "class_definition" {
        return Vec::new();
    }
    // `class Foo(Bar, Baz, metaclass=Meta):` — `superclasses` is the
    // argument_list node containing positional args (the base classes)
    // and keyword args (ignored — `metaclass=...` is not a parent).
    let Some(superclasses) = node.child_by_field_name("superclasses") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut cursor = superclasses.walk();
    for c in superclasses.named_children(&mut cursor) {
        if c.kind() == "keyword_argument" {
            continue;
        }
        if let Ok(text) = c.utf8_text(source) {
            let s = strip_generics(text.trim());
            if !s.is_empty() {
                out.push(s.to_string());
            }
        }
    }
    out
}

#[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
fn ts_js_extends(node: tree_sitter::Node<'_>, source: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    match node.kind() {
        "class_declaration" => {
            // tree-sitter-{javascript,typescript} expose
            // `class_heritage` with `extends_clause` and
            // (TS only) `implements_clause` children.
            let mut cursor = node.walk();
            for c in node.named_children(&mut cursor) {
                if c.kind() == "class_heritage" {
                    collect_heritage_children(c, source, &mut out);
                }
            }
        }
        "interface_declaration" => {
            // `interface I extends A, B` — `extends_type_clause` /
            // `extends_clause` depending on grammar version.
            let mut cursor = node.walk();
            for c in node.named_children(&mut cursor) {
                let k = c.kind();
                if k == "extends_clause"
                    || k == "extends_type_clause"
                    || k == "interface_extends_clause"
                {
                    collect_heritage_children(c, source, &mut out);
                }
            }
        }
        _ => {}
    }
    out
}

#[cfg(any(feature = "lang-javascript", feature = "lang-typescript"))]
fn collect_heritage_children(clause: tree_sitter::Node<'_>, source: &[u8], out: &mut Vec<String>) {
    let mut cursor = clause.walk();
    for c in clause.children(&mut cursor) {
        let k = c.kind();
        if k == "extends_clause"
            || k == "implements_clause"
            || k == "extends_type_clause"
            || k == "interface_extends_clause"
        {
            collect_heritage_children(c, source, out);
            continue;
        }
        // Leaf punctuation / keyword — skip.
        if matches!(k, "extends" | "implements" | "," | "{" | "}") {
            continue;
        }
        if let Ok(text) = c.utf8_text(source) {
            let s = strip_generics(text.trim());
            if !s.is_empty() && s != "extends" && s != "implements" {
                out.push(s.to_string());
            }
        }
    }
}

// ------------------------------------------------------ small string helpers

/// Split `"a as b"` into `("a", "b")` (whitespace-trimmed). Returns `None`
/// when there is no ` as ` separator.
fn split_as(s: &str) -> Option<(&str, &str)> {
    let idx = s.find(" as ")?;
    Some((s[..idx].trim(), s[idx + 4..].trim()))
}

/// Split `input` on `sep` ignoring occurrences inside `{}`, `[]`, `()` or
/// `<>`. Needed to keep group imports like `{A, B as Bee}` intact when
/// splitting outer comma lists.
fn split_top_level(input: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, ch) in input.char_indices() {
        match ch {
            '{' | '[' | '(' | '<' => depth += 1,
            '}' | ']' | ')' | '>' if depth > 0 => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&input[start..i]);
                start = i + ch.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&input[start..]);
    out
}

/// Join two module-path fragments with `::`. Empty prefixes yield just
/// the tail, so the top-level call with `prefix = ""` behaves correctly.
#[cfg(feature = "lang-rust")]
fn join_prefix(prefix: &str, tail: &str) -> String {
    let tail = tail.trim_end_matches("::");
    if prefix.is_empty() {
        tail.to_string()
    } else if tail.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}::{tail}")
    }
}

/// Drop anything inside `<...>` (including nested `<<>>` inside generics).
/// Useful for canonicalising `Vec<T>` / `Option<Box<dyn X>>` into the
/// bare type name the graph keys on.
fn strip_generics(s: &str) -> &str {
    match s.find('<') {
        Some(i) => s[..i].trim(),
        None => s.trim(),
    }
}

/// Take the last identifier segment of a path-ish expression.
///
/// Examples:
/// - `foo::bar::baz` → `baz`
/// - `obj.method`   → `method`
/// - `foo`          → `foo`
/// - `Vec::new`     → `new`
///
/// Stops at the first `<` or `(` so turbofish / argument lists don't leak
/// into the callee name, then returns the last non-empty segment split by
/// `.` or `:`. Empty segments are skipped so `foo::bar` (which tokenises as
/// `["foo", "", "bar"]` when splitting on `:`) still collapses to `bar`.
fn last_name_segment(raw: &str) -> String {
    let s = raw.trim();
    // Strip anything inside `<...>` so turbofish doesn't swallow the real
    // trailing identifier (`Vec::<i32>::new` must collapse to `new`).
    let mut no_gen = String::with_capacity(s.len());
    let mut depth = 0i32;
    for ch in s.chars() {
        match ch {
            '<' => depth += 1,
            '>' if depth > 0 => depth -= 1,
            _ if depth == 0 => no_gen.push(ch),
            _ => {}
        }
    }
    // Drop argument list if the callee text happened to include it.
    let head = no_gen.split('(').next().unwrap_or(&no_gen);
    // Last non-empty segment split by `.` or `:` — skipping the empty
    // pieces `foo::bar` produces when tokenised on single `:`.
    head.rsplit(['.', ':'])
        .find(|p| !p.is_empty())
        .unwrap_or(head)
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_segment_handles_common_forms() {
        assert_eq!(last_name_segment("foo"), "foo");
        assert_eq!(last_name_segment("foo::bar::baz"), "baz");
        assert_eq!(last_name_segment("obj.method"), "method");
        assert_eq!(last_name_segment("Vec::<i32>::new"), "new");
        assert_eq!(last_name_segment("println"), "println");
        assert_eq!(last_name_segment("a::b("), "b");
    }

    #[test]
    fn split_top_level_respects_groups() {
        assert_eq!(split_top_level("a, b, c", ','), vec!["a", " b", " c"],);
        assert_eq!(split_top_level("{a, b}, c", ','), vec!["{a, b}", " c"],);
        assert_eq!(
            split_top_level("foo<Bar, Baz>, quux", ','),
            vec!["foo<Bar, Baz>", " quux"],
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn rust_alias_extraction() {
        let mut out = Vec::new();
        rust_aliases("use foo::Bar;", &mut out);
        assert_eq!(out, vec![("Bar".into(), "foo::Bar".into())]);

        out.clear();
        rust_aliases("use foo::Bar as Baz;", &mut out);
        assert_eq!(out, vec![("Baz".into(), "foo::Bar".into())]);

        out.clear();
        rust_aliases("pub use std::sync::{Arc, Mutex as M};", &mut out);
        // Order follows left-to-right traversal of the group.
        assert_eq!(
            out,
            vec![
                ("Arc".into(), "std::sync::Arc".into()),
                ("M".into(), "std::sync::Mutex".into()),
            ],
        );

        out.clear();
        rust_aliases("use std::io::{self, Read};", &mut out);
        assert_eq!(
            out,
            vec![
                ("io".into(), "std::io".into()),
                ("Read".into(), "std::io::Read".into()),
            ],
        );

        out.clear();
        rust_aliases("use foo::*;", &mut out);
        assert!(out.is_empty(), "glob import should yield no aliases");
    }

    #[cfg(feature = "lang-python")]
    #[test]
    fn python_alias_extraction() {
        let mut out = Vec::new();
        python_aliases("from collections import OrderedDict", &mut out);
        assert_eq!(
            out,
            vec![("OrderedDict".into(), "collections::OrderedDict".into())],
        );

        out.clear();
        python_aliases("from a.b import c as cc, d", &mut out);
        assert_eq!(
            out,
            vec![
                ("cc".into(), "a.b::c".into()),
                ("d".into(), "a.b::d".into()),
            ],
        );

        out.clear();
        python_aliases("import numpy as np", &mut out);
        assert_eq!(out, vec![("np".into(), "numpy".into())]);

        out.clear();
        python_aliases("import os.path", &mut out);
        assert_eq!(out, vec![("os".into(), "os.path".into())]);
    }

    #[cfg(feature = "lang-typescript")]
    #[test]
    fn ts_js_alias_extraction() {
        let mut out = Vec::new();
        ts_js_aliases("import foo from 'lib';", &mut out);
        assert_eq!(out, vec![("foo".into(), "lib::default".into())]);

        out.clear();
        ts_js_aliases("import * as ns from \"./mod\";", &mut out);
        assert_eq!(out, vec![("ns".into(), "./mod".into())]);

        out.clear();
        ts_js_aliases("import { a, b as bb } from 'lib';", &mut out);
        assert_eq!(
            out,
            vec![
                ("a".into(), "lib::a".into()),
                ("bb".into(), "lib::b".into()),
            ],
        );

        out.clear();
        ts_js_aliases("import def, { x } from 'lib';", &mut out);
        assert_eq!(
            out,
            vec![
                ("def".into(), "lib::default".into()),
                ("x".into(), "lib::x".into()),
            ],
        );

        out.clear();
        ts_js_aliases("import 'side-effect';", &mut out);
        assert!(out.is_empty());
    }

    #[cfg(feature = "lang-go")]
    #[test]
    fn go_alias_extraction() {
        let mut out = Vec::new();
        go_aliases("import \"fmt\"", &mut out);
        assert_eq!(out, vec![("fmt".into(), "fmt".into())]);

        out.clear();
        go_aliases("import f \"fmt\"", &mut out);
        assert_eq!(out, vec![("f".into(), "fmt".into())]);

        out.clear();
        go_aliases(
            "import (\n  \"fmt\"\n  log \"github.com/sirupsen/logrus\"\n  _ \"net/http/pprof\"\n)",
            &mut out,
        );
        assert_eq!(
            out,
            vec![
                ("fmt".into(), "fmt".into()),
                ("log".into(), "github.com/sirupsen/logrus".into()),
                // blank import skipped
            ],
        );
    }

    #[test]
    fn strip_generics_behaviour() {
        assert_eq!(strip_generics("Foo"), "Foo");
        assert_eq!(strip_generics("Foo<Bar>"), "Foo");
        assert_eq!(strip_generics("Foo<Bar<T>>"), "Foo");
    }
}

/// Registry of enabled languages. Detection is extension-based.
#[derive(Debug, Default, Clone)]
pub struct LanguageRegistry {
    _private: (),
}

impl LanguageRegistry {
    /// Construct a registry with every compile-enabled language active.
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Detect the language of a given path based on its extension.
    ///
    /// Returns `None` if the extension is unknown or the relevant grammar
    /// feature is disabled.
    pub fn detect(&self, path: &Path) -> Option<Language> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            #[cfg(feature = "lang-rust")]
            "rs" => Language {
                inner: LanguageKind::Rust,
            },
            #[cfg(feature = "lang-python")]
            "py" | "pyi" => Language {
                inner: LanguageKind::Python,
            },
            #[cfg(feature = "lang-javascript")]
            "js" | "mjs" | "cjs" | "jsx" => Language {
                inner: LanguageKind::JavaScript,
            },
            #[cfg(feature = "lang-typescript")]
            "ts" | "tsx" => Language {
                inner: LanguageKind::TypeScript,
            },
            #[cfg(feature = "lang-go")]
            "go" => Language {
                inner: LanguageKind::Go,
            },
            _ => return None,
        })
    }
}
