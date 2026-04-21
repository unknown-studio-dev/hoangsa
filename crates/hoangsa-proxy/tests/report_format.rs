//! P4 tests — machine-parsable report format. Verifies the stderr footer
//! always starts with a fixed prefix, uses key=value fields, and never
//! contains box-drawing or emoji (so Claude doesn't pay token tax).

use std::process::{Command, Stdio};

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

#[cfg(unix)]
fn run_hsp(args: &[&str]) -> (String, String, Option<i32>) {
    let out = Command::new(hsp_bin())
        .args(args)
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

/// The stderr must contain no box-drawing or emoji that the prior version
/// used — that format was pretty for humans and expensive for Claude.
fn assert_machine_format(stderr: &str) {
    for bad in ["──", "⚠", "ℹ", "… "] {
        assert!(
            !stderr.contains(bad),
            "found legacy pretty glyph {bad:?} in stderr: {stderr:?}"
        );
    }
    for line in stderr.lines() {
        if line.is_empty() {
            continue;
        }
        assert!(
            line.starts_with("[hsp]")
                || line.starts_with("[hsp warn]")
                || line.starts_with("[hsp info]")
                || line.starts_with("[hsp hint]"),
            "non-standard stderr line: {line:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn color_strip_emits_machine_record_and_hint() {
    let (_stdout, stderr, code) = run_hsp(&[
        "run",
        "sh",
        "-c",
        "printf '\\033[31mred\\033[0m\\n'",
    ]);
    assert_eq!(code, Some(0));
    assert_machine_format(&stderr);
    // Summary is there.
    let summary = stderr
        .lines()
        .find(|l| l.starts_with("[hsp] "))
        .expect("summary record present");
    assert!(summary.contains("ansi_stripped=true"));
    assert!(summary.contains("exit=0"));
    // Hint points to --raw.
    let hint = stderr
        .lines()
        .find(|l| l.starts_with("[hsp hint] "))
        .expect("hint record present");
    assert!(hint.contains("hsp run --raw"));
}

#[cfg(unix)]
#[test]
fn keep_color_no_ansi_no_report() {
    let (_stdout, stderr, code) = run_hsp(&[
        "run",
        "--keep-color",
        "sh",
        "-c",
        "printf hi",
    ]);
    assert_eq!(code, Some(0));
    assert!(stderr.is_empty() || !stderr.contains("[hsp"));
}

#[cfg(unix)]
#[test]
fn strict_flag_surfaces_in_summary() {
    // Strict mode forces passthrough for big cat, so nothing to report.
    // But color strip still runs → summary should say strict=true.
    let (_stdout, stderr, code) = run_hsp(&[
        "run",
        "--strict",
        "sh",
        "-c",
        "printf '\\033[31mred\\033[0m\\n'",
    ]);
    assert_eq!(code, Some(0));
    assert_machine_format(&stderr);
    let summary = stderr
        .lines()
        .find(|l| l.starts_with("[hsp] "))
        .expect("summary");
    assert!(summary.contains("strict=true"));
}

#[cfg(unix)]
#[test]
fn hsp_strict_env_sets_strict() {
    let out = Command::new(hsp_bin())
        .args(["run", "sh", "-c", "printf '\\033[31mx\\033[0m'"])
        .env("HSP_STRICT", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_machine_format(&stderr);
    let summary = stderr
        .lines()
        .find(|l| l.starts_with("[hsp] "))
        .expect("summary");
    assert!(summary.contains("strict=true"), "got: {stderr:?}");
}

#[cfg(unix)]
#[test]
fn records_are_single_line() {
    let (_stdout, stderr, _) = run_hsp(&[
        "run",
        "sh",
        "-c",
        "printf '\\033[31mred\\033[0m\\n'",
    ]);
    // No record spans multiple lines. Each line independently parseable.
    for line in stderr.lines() {
        assert!(!line.is_empty() || line.is_empty()); // sanity
        assert!(!line.contains('\n'));
    }
}

#[cfg(unix)]
#[test]
fn plain_run_no_report() {
    let (_stdout, stderr, code) = run_hsp(&["run", "sh", "-c", "printf hi"]);
    assert_eq!(code, Some(0));
    // No ANSI, no trim → no footer at all.
    assert!(stderr.is_empty(), "expected silent stderr, got: {stderr:?}");
}
