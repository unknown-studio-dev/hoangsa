//! Integration tests for `thoth memory migrate --yes` — the legacy-memory
//! triage pass (REQ-09).
//!
//! Seeds a `.thoth/MEMORY.md` with one entry per classification bucket
//! (session-handoff → Drop, commit-sha-only → Drop, user-preference →
//! Move to USER.md, neutral invariant → Keep), runs the migrate command
//! in `--yes` mode, and asserts the post-migration markdown layout.
//!
//! The underlying `classify_text` heuristics are unit-tested inline in
//! `src/migrate.rs::tests`; this file proves the CLI entrypoint + apply
//! loop route each verdict to the right storage surface end-to-end.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn run_migrate(cwd: &Path) -> (String, String, bool) {
    let bin = env!("CARGO_BIN_EXE_thoth");
    let out = Command::new(bin)
        .current_dir(cwd)
        .args(["--root", ".thoth", "memory", "migrate", "--yes"])
        .output()
        .expect("spawn thoth memory migrate");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

/// Minimal MEMORY.md with one entry per classification bucket the DESIGN-SPEC
/// cares about. Each entry follows the `### heading\n\nbody\ntags: ...\n`
/// layout that `MarkdownStore::read_facts` parses.
// MEMORY.md layout reminder: each fact is a `### title` block, optionally
// followed by a body paragraph and a `tags: a, b` line. `MarkdownStore`
// parses the block into `Fact { text = "title\nbody" }` — so the title
// line is what `pick_entry` substring-matches against when `migrate`
// calls `remove`. We encode the classification signal into the title so
// the heuristics fire AND the remove query disambiguates correctly.
const SEEDED_MEMORY: &str = "\
# MEMORY.md

### Session 2026-04-17 shipped memory migrate module
tags: session, handoff

### a1b2c3d4e5f6
tags: git

### user prefers concise answers over long explanations
tags: style, preference

### Thoth must always recall before editing code
tags: invariant, policy
";

#[test]
fn migrate_drops_session_handoff_and_commit_sha() {
    let tmp = TempDir::new().expect("tempdir");
    let thoth = tmp.path().join(".thoth");
    std::fs::create_dir_all(&thoth).unwrap();
    std::fs::write(thoth.join("MEMORY.md"), SEEDED_MEMORY).unwrap();
    // LESSONS.md must exist (header-only is fine) so MarkdownStore::open
    // doesn't bail.
    std::fs::write(thoth.join("LESSONS.md"), "# LESSONS.md\n").unwrap();

    let (stdout, stderr, ok) = run_migrate(tmp.path());
    assert!(ok, "migrate failed.\nSTDOUT:\n{stdout}\nSTDERR:\n{stderr}");

    let after = std::fs::read_to_string(thoth.join("MEMORY.md")).expect("read MEMORY.md");

    // Drop verdicts: neither the session-handoff prose nor the bare SHA
    // should survive. We match on substrings unique to each entry.
    assert!(
        !after.contains("Session 2026-04-17 shipped memory migrate module"),
        "session-handoff entry was not dropped:\n{after}"
    );
    assert!(
        !after.contains("a1b2c3d4e5f6"),
        "commit-sha-only entry was not dropped:\n{after}"
    );

    // Keep verdict: the neutral invariant must remain in MEMORY.md.
    assert!(
        after.contains("Thoth must always recall before editing code"),
        "invariant entry was erroneously removed:\n{after}"
    );
}

#[test]
fn migrate_moves_user_preference_to_user_md() {
    let tmp = TempDir::new().expect("tempdir");
    let thoth = tmp.path().join(".thoth");
    std::fs::create_dir_all(&thoth).unwrap();
    std::fs::write(thoth.join("MEMORY.md"), SEEDED_MEMORY).unwrap();
    std::fs::write(thoth.join("LESSONS.md"), "# LESSONS.md\n").unwrap();

    let (stdout, stderr, ok) = run_migrate(tmp.path());
    assert!(ok, "migrate failed: stdout={stdout} stderr={stderr}");

    let memory_after = std::fs::read_to_string(thoth.join("MEMORY.md")).unwrap();
    // Moved out of MEMORY.md.
    assert!(
        !memory_after
            .to_lowercase()
            .contains("user prefers concise answers"),
        "user preference still in MEMORY.md after migrate:\n{memory_after}"
    );

    // Appended into USER.md (created on-demand by append_preference).
    let user_md = thoth.join("USER.md");
    assert!(user_md.exists(), "USER.md was not created by migrate");
    let user_after = std::fs::read_to_string(&user_md).unwrap();
    assert!(
        user_after.to_lowercase().contains("concise answers"),
        "user preference missing from USER.md:\n{user_after}"
    );
}

#[test]
fn migrate_on_empty_store_is_noop() {
    let tmp = TempDir::new().expect("tempdir");
    let thoth = tmp.path().join(".thoth");
    std::fs::create_dir_all(&thoth).unwrap();
    std::fs::write(thoth.join("MEMORY.md"), "# MEMORY.md\n").unwrap();
    std::fs::write(thoth.join("LESSONS.md"), "# LESSONS.md\n").unwrap();

    let (stdout, stderr, ok) = run_migrate(tmp.path());
    assert!(ok, "migrate on empty store failed: {stdout}\n{stderr}");
    assert!(
        stdout.contains("nothing to do") || stdout.to_lowercase().contains("empty"),
        "expected empty-store message, got: {stdout}"
    );
}
