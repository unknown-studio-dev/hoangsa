//! Integration — `thoth rule check` with no args returns non-zero
//! exit when two rules share the same trigger signature.
//!
//! Covers TEST-SPEC `rule_check_detects_overlap` (REQ-25).

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

fn run_thoth(root: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_thoth");
    let mut cmd = Command::new(bin);
    cmd.arg("--root").arg(root);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("spawn thoth")
}

fn add_rule(root: &Path, id: &str) {
    let out = run_thoth(
        root,
        &[
            "rule",
            "add",
            "--id",
            id,
            "--inline",
            "--tool",
            "Bash",
            "--cmd-regex",
            "rm\\s+-rf",
            "--enforcement",
            "block",
        ],
    );
    assert!(
        out.status.success(),
        "rule add {id} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// No overlap on a fresh repo — the shipped defaults are authored
/// with distinct triggers.
#[test]
fn rule_check_no_overlap_on_defaults() {
    let dir = TempDir::new().unwrap();
    let out = run_thoth(dir.path(), &["rule", "check"]);
    assert!(
        out.status.success(),
        "rule check on defaults should be clean: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Two user rules with identical (tool + cmd_regex) → overlap.
/// Exit code must be non-zero and both rule ids appear in the
/// combined stdout/stderr stream.
#[test]
fn rule_check_detects_two_identical_triggers() {
    let dir = TempDir::new().unwrap();
    add_rule(dir.path(), "dup-a");
    add_rule(dir.path(), "dup-b");

    let out = run_thoth(dir.path(), &["rule", "check"]);
    assert!(
        !out.status.success(),
        "overlap must produce non-zero exit; stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        combined.contains("dup-a"),
        "output must name dup-a: {combined}"
    );
    assert!(
        combined.contains("dup-b"),
        "output must name dup-b: {combined}"
    );
}
