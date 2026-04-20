//! Integration test — `thoth rule list/diff/check/compile` against a
//! fresh repo.
//!
//! Covers TEST-SPEC `rule_list_effective_shows_layer_source` (REQ-21) +
//! general exercise of the rule CLI wiring. Drives the shipped
//! `thoth` binary end-to-end via `CARGO_BIN_EXE_thoth`, matching the
//! T-28 `gate_danger_rules` pattern.

use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

/// Spawn `thoth --root <root> <args...>` and capture stdout+stderr.
fn run_thoth(root: &Path, args: &[&str]) -> Output {
    let bin = env!("CARGO_BIN_EXE_thoth");
    let mut cmd = Command::new(bin);
    cmd.arg("--root").arg(root);
    for a in args {
        cmd.arg(a);
    }
    cmd.output().expect("spawn thoth")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

/// `thoth rule list` on a fresh repo prints the 5 shipped default rules.
#[test]
fn rule_list_on_fresh_repo_shows_five_defaults() {
    let dir = TempDir::new().unwrap();
    let out = run_thoth(dir.path(), &["rule", "list"]);
    assert!(out.status.success(), "rule list failed: {}", stderr(&out));
    let text = stdout(&out);
    // All 5 shipped default rule ids must show up in the effective layer.
    for id in &[
        "no-rm-rf",
        "no-force-push-main",
        "no-no-verify",
        "no-reset-hard",
        "no-drop-table",
    ] {
        assert!(text.contains(id), "missing default rule {id} in:\n{text}");
    }
}

/// `thoth rule list --layer default` returns the same 5 defaults.
#[test]
fn rule_list_layer_default_counts_five_via_json() {
    let dir = TempDir::new().unwrap();
    let out = run_thoth(
        dir.path(),
        &["--json", "rule", "list", "--layer", "default"],
    );
    assert!(
        out.status.success(),
        "rule list --json failed: {}",
        stderr(&out)
    );
    let payload: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("parse JSON");
    let arr = payload.as_array().expect("top-level JSON array");
    assert_eq!(arr.len(), 5, "expected 5 default rules, got: {payload}");
}

/// `thoth rule diff` on a fresh repo shows the default layer populated
/// and all other layers empty.
#[test]
fn rule_diff_on_fresh_repo_shows_default_layer_only() {
    let dir = TempDir::new().unwrap();
    let out = run_thoth(dir.path(), &["--json", "rule", "diff"]);
    assert!(out.status.success(), "rule diff failed: {}", stderr(&out));
    let payload: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("parse JSON");
    assert_eq!(
        payload["default"].as_array().map(Vec::len).unwrap_or(0),
        5,
        "default layer must list all 5 shipped rules: {payload}"
    );
    assert_eq!(payload["user"].as_array().map(Vec::len).unwrap_or(99), 0);
    assert_eq!(payload["project"].as_array().map(Vec::len).unwrap_or(99), 0);
    assert_eq!(payload["lesson"].as_array().map(Vec::len).unwrap_or(99), 0);
    assert_eq!(payload["ignore"].as_array().map(Vec::len).unwrap_or(99), 0);
}

/// `thoth rule check --tool Bash --cmd "rm -rf /tmp"` matches the
/// shipped `no-rm-rf` default rule with verdict Block.
#[test]
fn rule_check_rm_rf_matches_default_rule_block() {
    let dir = TempDir::new().unwrap();
    let out = run_thoth(
        dir.path(),
        &[
            "--json",
            "rule",
            "check",
            "--tool",
            "Bash",
            "--cmd",
            "rm -rf /tmp",
        ],
    );
    assert!(out.status.success(), "rule check failed: {}", stderr(&out));
    let payload: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("parse JSON");
    assert_eq!(
        payload["verdict"].as_str(),
        Some("Block"),
        "expected Block verdict, got: {payload}"
    );
    let matches = payload["matches"].as_array().expect("matches array");
    assert!(
        matches.iter().any(|r| r["id"].as_str() == Some("no-rm-rf")),
        "no-rm-rf must be among matches: {payload}"
    );
}

/// `thoth rule compile` on a fresh repo writes a valid project TOML
/// (even if empty).
#[test]
fn rule_compile_writes_project_toml() {
    let dir = TempDir::new().unwrap();
    let out = run_thoth(dir.path(), &["rule", "compile"]);
    assert!(
        out.status.success(),
        "rule compile failed: {}",
        stderr(&out)
    );
    let project_toml = dir.path().join("rules.project.toml");
    assert!(
        project_toml.exists(),
        "rule compile must write {}",
        project_toml.display()
    );
    let body = std::fs::read_to_string(&project_toml).unwrap();
    // Valid TOML (possibly empty).
    let _: toml::Value = toml::from_str(&body).expect("compiled output is valid TOML");
}

/// End-to-end: after a user override, `rule list --layer effective`
/// shows the overridden source (exercises layer merge).
#[test]
fn rule_list_effective_shows_user_override_source() {
    let dir = TempDir::new().unwrap();
    let out = run_thoth(
        dir.path(),
        &["rule", "override", "no-rm-rf", "--tier", "require"],
    );
    assert!(
        out.status.success(),
        "rule override failed: {}",
        stderr(&out)
    );

    let out = run_thoth(
        dir.path(),
        &["--json", "rule", "list", "--layer", "effective"],
    );
    assert!(out.status.success(), "rule list failed: {}", stderr(&out));
    let payload: serde_json::Value = serde_json::from_str(stdout(&out).trim()).expect("parse JSON");
    let rules = payload.as_array().expect("array");
    let no_rm_rf = rules
        .iter()
        .find(|r| r["id"].as_str() == Some("no-rm-rf"))
        .expect("no-rm-rf in effective");
    assert_eq!(no_rm_rf["source"].as_str(), Some("user"));
    assert_eq!(no_rm_rf["enforcement"].as_str(), Some("Require"));
}
