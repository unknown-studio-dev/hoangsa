//! End-to-end: `.hoangsa/proxy/config.toml` actually changes behaviour of
//! the `hsp` binary. Project-scoped — we chdir into a tempdir and use it
//! as CWD, so the global config on the dev machine doesn't interfere.

#![cfg(unix)]

use std::fs;
use std::process::{Command, Stdio};
use tempfile::TempDir;

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

/// Run `hsp run <args>` with `cwd` set to `dir`. Returns stdout, stderr,
/// exit code.
fn run_in(dir: &std::path::Path, args: &[&str]) -> (String, String, Option<i32>) {
    let out = Command::new(hsp_bin())
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn hsp");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code(),
    )
}

fn write_config(dir: &std::path::Path, body: &str) {
    let p = dir.join(".hoangsa").join("proxy");
    fs::create_dir_all(&p).unwrap();
    fs::write(p.join("config.toml"), body).unwrap();
}

#[test]
fn strict_default_from_config() {
    let tmp = TempDir::new().unwrap();
    write_config(tmp.path(), "[runtime]\nstrict = true\n");
    // Trigger a report by forcing color strip (piped stdout is not a TTY).
    let (_out, stderr, code) = run_in(
        tmp.path(),
        &["run", "sh", "-c", "printf '\\033[31mX\\033[0m'"],
    );
    assert_eq!(code, Some(0));
    let summary = stderr
        .lines()
        .find(|l| l.starts_with("[hsp] "))
        .unwrap_or_else(|| panic!("no summary. stderr={stderr:?}"));
    assert!(
        summary.contains("strict=true"),
        "config-driven strict missed: {summary:?}"
    );
}

#[test]
fn cli_flag_beats_config() {
    let tmp = TempDir::new().unwrap();
    // Config says strict=true, but `--strict` / no-flag test the layering.
    // Here we write strict=true then run without the flag → still strict.
    // Layering is correct when env/flag can override in the other direction,
    // which we verify via env below. No explicit "--no-strict" flag exists,
    // so the config-false-flag-true direction is the testable one.
    write_config(tmp.path(), "[runtime]\nstrict = false\n");
    let (_out, stderr, _) = run_in(
        tmp.path(),
        &[
            "run",
            "--strict",
            "sh",
            "-c",
            "printf '\\033[31mX\\033[0m'",
        ],
    );
    let summary = stderr.lines().find(|l| l.starts_with("[hsp] ")).unwrap();
    assert!(summary.contains("strict=true"));
}

#[test]
fn disabled_handler_forces_passthrough() {
    // With `cargo` disabled, the cargo compile filter must not run. We
    // simulate `cargo build`-ish stderr via `sh` and verify our default
    // drops "Compiling" lines only when the handler IS active.
    //
    // Easier: exec a known-handler cmd (grep) and disable it, compare to
    // baseline.
    let tmp = TempDir::new().unwrap();
    let big: String = (0..500).map(|i| format!("line{i}\n")).collect();
    fs::write(tmp.path().join("big.txt"), &big).unwrap();

    // Baseline: grep handler trims long output.
    let (out_default, _err, _) = run_in(tmp.path(), &["run", "grep", "line", "big.txt"]);
    let default_lines = out_default.lines().count();
    assert!(
        default_lines < 500,
        "default grep should have trimmed, got {default_lines}"
    );

    // With handler disabled: full output.
    write_config(tmp.path(), "[handlers]\ndisabled = [\"grep\"]\n");
    let (out_disabled, _err, _) = run_in(tmp.path(), &["run", "grep", "line", "big.txt"]);
    let disabled_lines = out_disabled.lines().count();
    assert_eq!(
        disabled_lines, 500,
        "disabled grep must passthrough all 500 lines, got {disabled_lines}"
    );
}

#[test]
fn broken_config_warns_then_runs() {
    let tmp = TempDir::new().unwrap();
    write_config(tmp.path(), "not-valid-toml == broken");
    let (out, stderr, code) = run_in(tmp.path(), &["run", "sh", "-c", "printf hi"]);
    // Command still ran successfully.
    assert_eq!(code, Some(0));
    assert_eq!(out, "hi");
    // Warning surfaced.
    assert!(
        stderr.contains("config_parse_error"),
        "stderr must surface parse warning: {stderr:?}"
    );
}

#[test]
fn max_output_mb_applies_at_capture() {
    let tmp = TempDir::new().unwrap();
    // Cap at 1 MB. Child writes 2 MB — must see hard_cap.
    write_config(tmp.path(), "[runtime]\nmax_output_mb = 1\n");
    let (_out, stderr, code) = run_in(
        tmp.path(),
        &[
            "run",
            "sh",
            "-c",
            &format!("yes A | head -c {}", 2 * 1024 * 1024),
        ],
    );
    assert_eq!(code, Some(0));
    assert!(
        stderr.contains("event=hard_cap"),
        "expected hard_cap record, got stderr={stderr:?}"
    );
}
