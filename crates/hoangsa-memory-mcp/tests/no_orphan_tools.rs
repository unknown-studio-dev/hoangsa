//! Wiring guard: every MCP **tool** advertised in `tools_catalog` must be
//! surfaced to the agent somewhere — a skill, workflow, the worker envelope,
//! a hook, the guidance snippet, or another genuine wiring surface. A tool
//! that exists in the catalog but is referenced nowhere is "built but not
//! wired": it works over MCP yet no agent is ever told it exists (exactly how
//! the code-graph tools sat unused, and `memory_event_trace` before it).
//!
//! This test turns that recurring smell into a machine gate. It reads the
//! catalog source (tools only — prompts are invoked via `prompts/get` and
//! wired differently) and checks each tool name against a POSITIVE list of
//! wiring surfaces. Positive (not "everything except definitions") on purpose:
//! it must not count a tool's own `#[cfg(test)]` dispatch test as "wired", or
//! the guard would pass for the very tools that follow the house test pattern.
//!
//! When this fails: either surface the new tool (add it to a skill / the
//! guidance list / the envelope) or, if it is genuinely internal, add it to
//! `ALLOWLIST` with a one-line reason.

use std::fs;
use std::path::{Path, PathBuf};

/// Tools that are intentionally NOT surfaced to the agent, with the reason.
/// Keep this empty unless a tool is truly internal — an allowlist entry is a
/// deliberate "this capability is not for the agent", not an escape hatch.
const ALLOWLIST: &[(&str, &str)] = &[];

/// Positive wiring surfaces — the places a tool legitimately reaches the agent
/// or a flow. Paths are relative to the workspace root. Directories are walked
/// recursively; files are read directly.
const SURFACES: &[&str] = &[
    "templates",                                          // skills, workflows, commands, agents
    "crates/hoangsa-cli/src/cmd/guidance.rs",             // memory-guidance recall list
    "crates/hoangsa-cli/src/cmd/envelope.rs",             // worker envelope instructions
    "crates/hoangsa-cli/src/cmd/hook",                    // hooks invoke tools
    "crates/hoangsa-memory-core/src/event.rs",            // event-bus nudge
    "crates/hoangsa-memory-policy/src",                   // policy-surfaced tools
    "crates/hoangsa-memory/src/init_cmd.rs",              // install-time surfaced
];

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR = <root>/crates/hoangsa-memory-mcp
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("workspace root is two levels above the crate manifest")
        .to_path_buf()
}

/// Extract every `name: "memory_..."` inside the `tools_catalog` function only
/// (stop at `prompts_catalog`). Simple line scan — no regex dep needed.
fn catalog_tool_names(catalog_src: &str) -> Vec<String> {
    let start = catalog_src
        .find("fn tools_catalog")
        .expect("tools_catalog fn present");
    let end = catalog_src
        .find("fn prompts_catalog")
        .unwrap_or(catalog_src.len());
    let tools_region = &catalog_src[start..end];

    let mut names = Vec::new();
    for line in tools_region.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("name: \"")
            && let Some(end_q) = rest.find('"')
        {
            let name = &rest[..end_q];
            if name.starts_with("memory_") {
                names.push(name.to_string());
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Concatenate the text of every wiring surface into one haystack.
fn wiring_haystack(root: &Path) -> String {
    let mut buf = String::new();
    for surface in SURFACES {
        let p = root.join(surface);
        if p.is_dir() {
            collect_dir(&p, &mut buf);
        } else if p.is_file()
            && let Ok(s) = fs::read_to_string(&p)
        {
            buf.push_str(&s);
            buf.push('\n');
        }
    }
    buf
}

fn collect_dir(dir: &Path, buf: &mut String) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_dir(&path, buf);
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "md" | "rs" | "json" | "toml"))
            && let Ok(s) = fs::read_to_string(&path)
        {
            buf.push_str(&s);
            buf.push('\n');
        }
    }
}

#[test]
fn every_catalog_tool_is_wired_to_a_surface() {
    let root = workspace_root();
    let catalog_src = fs::read_to_string(root.join("crates/hoangsa-memory-mcp/src/server/catalog.rs"))
        .expect("read catalog.rs");

    let tools = catalog_tool_names(&catalog_src);
    assert!(
        tools.len() >= 20,
        "sanity: expected to parse the tool catalog, got only {} names",
        tools.len()
    );

    let haystack = wiring_haystack(&root);
    let allowed: std::collections::HashSet<&str> = ALLOWLIST.iter().map(|(t, _)| *t).collect();

    let orphans: Vec<&String> = tools
        .iter()
        .filter(|t| !allowed.contains(t.as_str()) && !haystack.contains(t.as_str()))
        .collect();

    assert!(
        orphans.is_empty(),
        "MCP tools built but not wired to any agent-facing surface: {orphans:?}\n\
         Surface each in a skill / workflow / guidance / envelope, or add it to \
         ALLOWLIST in this test with a reason. Wiring surfaces checked: {SURFACES:?}"
    );
}
