//! End-to-end: `hsp doctor` emits machine-format records and uses exit 0
//! when every check is non-fail.

#![cfg(unix)]

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

fn run_doctor_in(dir: &std::path::Path) -> (String, i32) {
    let out = Command::new(hsp_bin())
        .args(["doctor"])
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("spawn");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

#[test]
fn doctor_emits_core_checks() {
    let tmp = TempDir::new().unwrap();
    let (out, code) = run_doctor_in(tmp.path());
    // Core checks must all appear.
    for item in [
        "item=version",
        "item=platform",
        "item=binary_path",
        "item=hook_global",
        "item=hook_project",
        "item=hook_rewrite",
        "item=config",
        "item=handlers",
    ] {
        assert!(out.contains(item), "missing {item} in doctor output:\n{out}");
    }
    // Everything is a machine record, no prose.
    for line in out.lines() {
        if line.is_empty() {
            continue;
        }
        assert!(
            line.starts_with("[hsp check] "),
            "non-record line: {line:?}"
        );
    }
    // Warn-only (no failures) → exit 0.
    assert_eq!(code, 0);
}

#[test]
fn doctor_hook_rewrite_reports_latency() {
    let tmp = TempDir::new().unwrap();
    let (out, _code) = run_doctor_in(tmp.path());
    let line = out
        .lines()
        .find(|l| l.contains("item=hook_rewrite"))
        .expect("hook_rewrite record");
    assert!(
        line.contains("latency_ms="),
        "hook_rewrite should include latency: {line}"
    );
    assert!(line.contains("response_valid_json=true"));
    assert!(line.contains("status=ok"));
}

#[test]
fn doctor_flags_broken_config() {
    let tmp = TempDir::new().unwrap();
    let cfg_dir = tmp.path().join(".hoangsa-proxy");
    fs::create_dir_all(&cfg_dir).unwrap();
    fs::write(cfg_dir.join("config.toml"), "not = valid = toml").unwrap();
    let (out, _code) = run_doctor_in(tmp.path());
    // config status must flip to warn
    let line = out.lines().find(|l| l.contains("item=config ")).unwrap();
    assert!(line.contains("status=warn"), "got: {line}");
    assert!(line.contains("warnings_count="));
    // The specific warning detail surfaces as its own record
    assert!(
        out.lines().any(|l| l.contains("item=config_warning")),
        "expected config_warning record: {out}"
    );
}

#[test]
fn doctor_sees_installed_hook() {
    let tmp = TempDir::new().unwrap();
    let settings = tmp.path().join(".claude/settings.local.json");
    fs::create_dir_all(settings.parent().unwrap()).unwrap();
    fs::write(
        &settings,
        r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"hsp hook rewrite # __hsp"}]}]}}"#,
    )
    .unwrap();
    let (out, code) = run_doctor_in(tmp.path());
    assert_eq!(code, 0);
    let line = out
        .lines()
        .find(|l| l.contains("item=hook_project"))
        .unwrap();
    assert!(line.contains("installed=true"), "got: {line}");
    assert!(line.contains("status=ok"));
}
