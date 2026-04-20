//! Integration tests for the `thoth hooks exec session-start` command.
//!
//! These tests spawn the real `thoth` binary (via `CARGO_BIN_EXE_thoth`)
//! against a fresh tempdir and assert on the banner stdout that Claude Code
//! would ingest. They complement the inline unit tests in `src/hooks.rs`
//! (which call `run_session_start` directly) by exercising the end-to-end
//! CLI path — argv parsing, `HookEvent::SessionStart` dispatch, and stdout
//! writeback.
//!
//! Covered: REQ-01 (USER.md injection ordering + missing-file skip).

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

/// Invoke `thoth --root <root>/.thoth hooks exec session-start` and return
/// (stdout, stderr). We run from `root` (not `.thoth`) because `thoth`'s
/// global `--root` defaults to `./.thoth` and Project-scope paths are
/// resolved relative to CWD.
fn run_session_start(root: &Path) -> (String, String, bool) {
    let bin = env!("CARGO_BIN_EXE_thoth");
    let out = Command::new(bin)
        .current_dir(root)
        .args(["--root", ".thoth", "hooks", "exec", "session-start"])
        .output()
        .expect("spawn thoth binary");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn session_start_injects_user_md_before_memory_md() {
    let tmp = TempDir::new().expect("tempdir");
    let thoth_dir = tmp.path().join(".thoth");
    std::fs::create_dir_all(&thoth_dir).unwrap();
    std::fs::write(thoth_dir.join("USER.md"), "# USER.md\nuser pref body\n").unwrap();
    std::fs::write(
        thoth_dir.join("MEMORY.md"),
        "# MEMORY.md\nproject fact body\n",
    )
    .unwrap();

    let (stdout, stderr, ok) = run_session_start(tmp.path());
    assert!(ok, "session-start failed: stderr={stderr} stdout={stdout}");

    let user_idx = stdout
        .find("### USER.md")
        .unwrap_or_else(|| panic!("missing ### USER.md in stdout: {stdout}"));
    let mem_idx = stdout
        .find("### MEMORY.md")
        .unwrap_or_else(|| panic!("missing ### MEMORY.md in stdout: {stdout}"));
    assert!(
        user_idx < mem_idx,
        "USER.md must render before MEMORY.md: user={user_idx} mem={mem_idx}\n{stdout}"
    );
    assert!(
        stdout.contains("user pref body"),
        "USER.md body missing from banner: {stdout}"
    );
}

#[test]
fn session_start_skips_missing_user_md() {
    let tmp = TempDir::new().expect("tempdir");
    let thoth_dir = tmp.path().join(".thoth");
    std::fs::create_dir_all(&thoth_dir).unwrap();
    // No USER.md — but MEMORY.md exists so the banner still has content.
    std::fs::write(
        thoth_dir.join("MEMORY.md"),
        "# MEMORY.md\nproject fact body\n",
    )
    .unwrap();

    let (stdout, stderr, ok) = run_session_start(tmp.path());
    assert!(ok, "session-start failed: stderr={stderr} stdout={stdout}");
    assert!(
        !stdout.contains("### USER.md"),
        "USER.md header must not appear when file is absent: {stdout}"
    );
    assert!(
        stdout.contains("### MEMORY.md"),
        "MEMORY.md section missing even though the file exists: {stdout}"
    );
}
