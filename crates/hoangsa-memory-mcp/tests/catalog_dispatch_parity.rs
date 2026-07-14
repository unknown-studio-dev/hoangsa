//! Drift gate: every tool advertised in `tools_catalog` must have a dispatch arm
//! in `dispatch_tool`, and vice versa.
//!
//! Parses the two source files with simple line-by-line regex — no proc macros,
//! no external deps, fully std-only.

use std::collections::HashSet;
use std::path::Path;

/// Tools that are deliberately allowed to appear in one side only.
/// Start empty; add entries with a reason comment when a legitimate gap exists.
const ALLOWLIST: &[&str] = &[];

fn workspace_root() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR = <root>/crates/hoangsa-memory-mcp
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// Extract `memory_*` names from lines of the form:
///   `name: "memory_...".to_string(),`
/// Only inside `fn tools_catalog` (up to but not including `fn prompts_catalog`).
fn parse_catalog_tools(src: &str) -> HashSet<String> {
    let mut tools = HashSet::new();
    let mut in_fn = false;
    for line in src.lines() {
        if line.contains("fn tools_catalog") {
            in_fn = true;
            continue;
        }
        if in_fn && line.contains("fn prompts_catalog") {
            break;
        }
        if !in_fn {
            continue;
        }
        // Match: name: "memory_XXXX".to_string()
        if let Some(after) = line.find("name: \"memory_") {
            let rest = &line[after + "name: \"".len()..];
            if let Some(end) = rest.find('"') {
                tools.insert(rest[..end].to_string());
            }
        }
    }
    tools
}

/// Extract `memory_*` names from `dispatch_tool`'s match arms only.
///
/// Scans from the line containing `fn dispatch_tool` down to (and excluding)
/// the closing brace of its outer match block (first unindented `}` after the
/// match). This avoids picking up prompt names from `prompts_get`.
fn parse_dispatch_arms(src: &str) -> HashSet<String> {
    let mut arms = HashSet::new();
    let mut in_fn = false;
    let mut brace_depth: i32 = 0;
    for line in src.lines() {
        if !in_fn {
            if line.contains("fn dispatch_tool") {
                in_fn = true;
            }
            continue;
        }
        // Track brace depth to know when dispatch_tool ends.
        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if brace_depth < 0 {
                        // Exited dispatch_tool.
                        return arms;
                    }
                }
                _ => {}
            }
        }
        // Match: "memory_XXXX" =>
        if let Some(after) = line.find("\"memory_") {
            let rest = &line[after + 1..]; // skip the opening quote
            if let Some(end) = rest.find('"') {
                let name = &rest[..end];
                if line[after + 1 + end + 1..].trim_start().starts_with("=>") {
                    arms.insert(name.to_string());
                }
            }
        }
    }
    arms
}

#[test]
fn catalog_and_dispatch_are_in_sync() {
    let root = workspace_root();
    let catalog_src =
        std::fs::read_to_string(root.join("crates/hoangsa-memory-mcp/src/server/catalog.rs"))
            .expect("read catalog.rs");
    let dispatch_src =
        std::fs::read_to_string(root.join("crates/hoangsa-memory-mcp/src/server/dispatch.rs"))
            .expect("read dispatch.rs");

    let catalog = parse_catalog_tools(&catalog_src);
    let dispatch = parse_dispatch_arms(&dispatch_src);

    // Sanity: parsers must have found something non-trivial.
    assert!(
        catalog.len() >= 20,
        "catalog parse found only {} tools — likely a parse regression (expected ≥20)",
        catalog.len()
    );
    assert!(
        dispatch.len() >= 20,
        "dispatch parse found only {} arms — likely a parse regression (expected ≥20)",
        dispatch.len()
    );

    let allowlisted: HashSet<&str> = ALLOWLIST.iter().copied().collect();

    // Catalog tools missing from dispatch → "advertised but unrouted" (errors when called).
    let missing_in_dispatch: Vec<&str> = catalog
        .iter()
        .map(|s| s.as_str())
        .filter(|name| !dispatch.contains(*name) && !allowlisted.contains(name))
        .collect::<std::collections::BTreeSet<&str>>()
        .into_iter()
        .collect();

    // Dispatch arms missing from catalog → "routed but not advertised" (dead / renamed).
    let missing_in_catalog: Vec<&str> = dispatch
        .iter()
        .map(|s| s.as_str())
        .filter(|name| !catalog.contains(*name) && !allowlisted.contains(name))
        .collect::<std::collections::BTreeSet<&str>>()
        .into_iter()
        .collect();

    assert!(
        missing_in_dispatch.is_empty(),
        "catalog tools with NO dispatch arm (advertised but unrouted — will error when called):\n  {}\n\
         catalog={} dispatch={}",
        missing_in_dispatch.join(", "),
        catalog.len(),
        dispatch.len(),
    );
    assert!(
        missing_in_catalog.is_empty(),
        "dispatch arms with NO catalog entry (routed but not advertised — dead or renamed):\n  {}\n\
         catalog={} dispatch={}",
        missing_in_catalog.join(", "),
        catalog.len(),
        dispatch.len(),
    );
}
