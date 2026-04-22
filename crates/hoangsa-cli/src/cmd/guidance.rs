//! `hoangsa-cli memory-guidance sync` — seed project-level CLAUDE.md /
//! AGENTS.md pointers to a canonical memory-guidance document so Claude
//! Code and subagents know this project runs hoangsa-memory at SessionStart.
//!
//! The CLI only maintains the **pointer** block between
//! `<!-- hoangsa-memory-start -->` / `<!-- hoangsa-memory-end -->` markers,
//! plus writes the canonical body to `.hoangsa/memory-guidance.md`. The body
//! is intentionally short — detailed discipline lives in the
//! `memory-discipline` skill.

use serde_json::json;
use std::fs;
use std::path::Path;

const START_MARKER: &str = "<!-- hoangsa-memory-start -->";
const END_MARKER: &str = "<!-- hoangsa-memory-end -->";
const GUIDANCE_REL_PATH: &str = ".hoangsa/memory-guidance.md";

/// Body written to `.hoangsa/memory-guidance.md`. Kept intentionally short:
/// SessionStart injects USER.md / MEMORY.md / LESSONS.md verbatim, and the
/// `memory-discipline` skill carries the full protocol. This file is a
/// lightweight, hand-editable reminder that the project is memory-backed.
const MEMORY_GUIDANCE_MD: &str = r#"# hoangsa-memory — project guidance

This project is instrumented with `hoangsa-memory`, a local MCP server that
gives you persistent memory and a semantic code graph. Use the MCP tools
below instead of guessing from filenames or fabricating APIs.

## Recall before acting

Run these before editing code, answering "how does X work", refactoring,
or debugging:

- `memory_recall({query})` — hybrid BM25 + symbol + vector search over
  the indexed codebase. Returns chunks with `path:line-span` you can cite.
- `memory_wakeup()` — compact one-line-per-entry index of `MEMORY.md` +
  `LESSONS.md`.
- `memory_symbol_context({fqn})` — callers / callees / parent types /
  subtypes / references for a known symbol.
- `memory_impact({fqn})` — blast-radius analysis before editing a symbol.
  Required by the `require-memory-impact` rule on first edit to a file.

`USER.md`, `MEMORY.md`, and `LESSONS.md` are already injected at
SessionStart — scan them for preferences and applicable lessons before
you act.

## Reflect after acting

When a task finishes, resolves a bug, or reveals a durable pattern:

- `memory_remember_fact({text, tags})` — project-specific invariants →
  `MEMORY.md`. Example: "HTTP retry lives in `crates/net/retry.rs`".
- `memory_remember_lesson({trigger, advice})` — action-triggered advice
  → `LESSONS.md`. Example: `trigger: "when editing migrations"`,
  `advice: "run sqlx prepare after changing SQL"`.
- `memory_remember_preference({text})` — first-person, cross-project
  workflow choices → `USER.md`. Example: "user prefers Vietnamese
  responses".

Be conservative. Only persist memory that is specific, durable, and
non-obvious. Ephemeral session state belongs in a handoff note, not
memory.

## Anti-hallucination rules

- Never assert a name, signature, or behaviour without a recall hit.
  If `memory_recall` returns nothing relevant, say so and ask the user.
- Quote chunk ids when you rely on recalled context.
- If a `LESSONS.md` trigger advises against your plan, stop and ask
  the user — do not proceed silently.

## See also

- Skill: `memory-discipline` — full protocol and cap-exceeded handling.
- CLI: `hoangsa-memory memory show` / `hoangsa-memory memory log`.
- Config: `.hoangsa/memory/config.toml` (memory mode, caps, policy).
"#;

/// Pointer block inserted into project `CLAUDE.md` and `AGENTS.md`. Uses
/// Claude Code's `@path` import syntax so the full guidance loads at
/// SessionStart without bloating the root instruction file.
fn pointer_block() -> String {
    format!(
        "{START_MARKER}\n\
## hoangsa-memory\n\
\n\
This project uses hoangsa-memory (local MCP server) for persistent memory \
and a semantic code graph. Before non-trivial edits, call `memory_recall` / \
`memory_impact`; after finishing work, call `memory_remember_fact` / \
`memory_remember_lesson` / `memory_remember_preference` to persist what's \
durable.\n\
\n\
Full protocol: @{GUIDANCE_REL_PATH}\n\
{END_MARKER}"
    )
}

/// Write or replace the marker block in `path`. Creates the file when
/// missing; preserves anything outside the markers. Returns true if the
/// on-disk content actually changed.
fn upsert_marker_block(path: &Path, block: &str) -> std::io::Result<bool> {
    let existing = match fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };

    let updated = match (existing.find(START_MARKER), existing.find(END_MARKER)) {
        (Some(start), Some(end)) if end > start => {
            let end_of_end = end + END_MARKER.len();
            format!("{}{}{}", &existing[..start], block, &existing[end_of_end..])
        }
        _ if existing.is_empty() => format!("{block}\n"),
        _ if existing.ends_with("\n\n") => format!("{existing}{block}\n"),
        _ if existing.ends_with('\n') => format!("{existing}\n{block}\n"),
        _ => format!("{existing}\n\n{block}\n"),
    };

    if updated == existing {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, updated)?;
    Ok(true)
}

/// Write `MEMORY_GUIDANCE_MD` to `.hoangsa/memory-guidance.md` under
/// `project_dir`. Overwrites unconditionally so the canonical body stays
/// in sync with the installed CLI.
fn write_guidance_body(project_dir: &Path) -> std::io::Result<bool> {
    let path = project_dir.join(GUIDANCE_REL_PATH);
    let prev = fs::read_to_string(&path).ok();
    if prev.as_deref() == Some(MEMORY_GUIDANCE_MD) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&path, MEMORY_GUIDANCE_MD)?;
    Ok(true)
}

pub fn cmd_sync(project_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new(project_dir);
    let body_changed = write_guidance_body(root)?;
    let block = pointer_block();
    let claude_changed = upsert_marker_block(&root.join("CLAUDE.md"), &block)?;
    let agents_changed = upsert_marker_block(&root.join("AGENTS.md"), &block)?;

    println!(
        "{}",
        json!({
            "success": true,
            "guidance_path": root.join(GUIDANCE_REL_PATH).to_string_lossy(),
            "guidance_written": body_changed,
            "claude_md_updated": claude_changed,
            "agents_md_updated": agents_changed,
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn sync_creates_guidance_and_both_pointer_files() {
        let dir = TempDir::new().unwrap();
        cmd_sync(dir.path().to_str().unwrap()).unwrap();

        let body = fs::read_to_string(dir.path().join(GUIDANCE_REL_PATH)).unwrap();
        assert!(body.contains("memory_recall"));

        let claude = fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(claude.contains(START_MARKER));
        assert!(claude.contains(END_MARKER));
        assert!(claude.contains(&format!("@{GUIDANCE_REL_PATH}")));

        let agents = fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(agents.contains(START_MARKER));
        assert!(agents.contains(&format!("@{GUIDANCE_REL_PATH}")));
    }

    #[test]
    fn sync_preserves_existing_claude_md_content_outside_markers() {
        let dir = TempDir::new().unwrap();
        let claude_path = dir.path().join("CLAUDE.md");
        fs::write(&claude_path, "# Project\n\nHand-written rules.\n").unwrap();

        cmd_sync(dir.path().to_str().unwrap()).unwrap();

        let claude = fs::read_to_string(&claude_path).unwrap();
        assert!(claude.contains("# Project"));
        assert!(claude.contains("Hand-written rules."));
        assert!(claude.contains(START_MARKER));
    }

    #[test]
    fn sync_replaces_block_between_existing_markers_idempotently() {
        let dir = TempDir::new().unwrap();
        let claude_path = dir.path().join("CLAUDE.md");
        let stale = format!(
            "# Project\n\n{START_MARKER}\nSTALE CONTENT\n{END_MARKER}\n\nafter\n"
        );
        fs::write(&claude_path, &stale).unwrap();

        cmd_sync(dir.path().to_str().unwrap()).unwrap();
        let first = fs::read_to_string(&claude_path).unwrap();
        assert!(!first.contains("STALE CONTENT"));
        assert!(first.contains("after"));

        cmd_sync(dir.path().to_str().unwrap()).unwrap();
        let second = fs::read_to_string(&claude_path).unwrap();
        assert_eq!(first, second, "second sync must be a no-op");
    }
}
