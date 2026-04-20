//! Integration tests for `thoth compact` — the LLM-driven MEMORY.md /
//! LESSONS.md consolidator.
//!
//! A real `thoth compact` invocation hits an external LLM endpoint, which
//! is not available in CI. These tests exercise the CLI wiring around the
//! compaction loop: argv parsing, the empty-store short-circuit, and the
//! missing-`.thoth/` short-circuit. The retry-on-oversize and drop-rule
//! logic (REQ-08) are covered by the companion unit tests in
//! `src/compact.rs::tests` — `compact_drops_session_handoff_entries` and
//! `compact_retries_on_oversize_output` — which inject a mock backend
//! closure the binary entrypoint cannot reach.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn run_compact(cwd: &Path, args: &[&str]) -> (String, String, bool) {
    let bin = env!("CARGO_BIN_EXE_thoth");
    let mut cmd = Command::new(bin);
    cmd.current_dir(cwd).args(["--root", ".thoth", "compact"]);
    cmd.args(args);
    let out = cmd.output().expect("spawn thoth compact");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn compact_short_circuits_when_thoth_dir_missing() {
    // No `.thoth/` at all — CLI must print a friendly bail, not spawn the
    // backend. This is the first gate in `cmd_compact` (see src/main.rs).
    let tmp = TempDir::new().expect("tempdir");
    let (stdout, _stderr, ok) = run_compact(tmp.path(), &["--dry-run"]);
    assert!(ok, "missing `.thoth/` must not error out: stdout={stdout}");
    assert!(
        stdout.to_lowercase().contains("nothing to compact") || stdout.contains("no .thoth"),
        "expected nothing-to-compact message, got: {stdout}"
    );
}

#[test]
fn compact_rejects_empty_memory_files() {
    // Fresh `.thoth/` with header-only MEMORY.md + LESSONS.md — no fact
    // rows to merge. `run_compact` must bail before attempting a backend
    // call so dry-runs against a brand-new install don't burn a token
    // quota for nothing.
    let tmp = TempDir::new().expect("tempdir");
    let thoth = tmp.path().join(".thoth");
    std::fs::create_dir_all(&thoth).unwrap();
    std::fs::write(thoth.join("MEMORY.md"), "# MEMORY.md\n").unwrap();
    std::fs::write(thoth.join("LESSONS.md"), "# LESSONS.md\n").unwrap();

    let (stdout, stderr, _) = run_compact(tmp.path(), &["--dry-run"]);
    let combined = format!("{stdout}\n{stderr}");
    assert!(
        combined.to_lowercase().contains("empty")
            || combined.to_lowercase().contains("nothing to compact"),
        "expected empty-store bail in output: {combined}"
    );
}

#[test]
fn compact_help_mentions_dry_run_and_backend_flags() {
    // Smoke-test the CLI surface: verifying the user-facing flags that
    // REQ-08 surfaces (dry-run + backend/model overrides) stay wired up.
    let bin = env!("CARGO_BIN_EXE_thoth");
    let out = Command::new(bin)
        .args(["compact", "--help"])
        .output()
        .expect("spawn thoth compact --help");
    let help = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "help failed: {help}");
    assert!(help.contains("--dry-run"), "--dry-run flag missing: {help}");
    assert!(help.contains("--backend"), "--backend flag missing: {help}");
    assert!(help.contains("--model"), "--model flag missing: {help}");
}
