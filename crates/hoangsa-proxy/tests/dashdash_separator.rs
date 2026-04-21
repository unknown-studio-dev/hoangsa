//! Regression: `--` separator must survive clap on `hsp run`.
//!
//! Some proxies built on clap's `trailing_var_arg(true)` accidentally drop
//! `--` when it appears as the first positional — which breaks commands
//! like `git diff -- path/to/file` (git reads the post-`--` token as a
//! path instead of a revision).
//!
//! Our shape is immune because the first positional is always the child
//! command name, so `--` never lands in slot zero. This test pins that
//! behaviour so a future refactor can't regress it.

#![cfg(unix)]

use std::process::{Command, Stdio};

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

fn run(args: &[&str]) -> (String, Option<i32>) {
    let out = Command::new(hsp_bin())
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .expect("spawn hsp");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.code(),
    )
}

#[test]
fn dashdash_survives_to_child() {
    // `hsp run sh -c 'echo "$@"' -- -v a.txt` — the child's `$@` must
    // include the literal `--`.
    let (out, code) = run(&[
        "run",
        "sh",
        "-c",
        "echo \"$@\"",
        "sh",
        "--",
        "-v",
        "a.txt",
    ]);
    assert_eq!(code, Some(0));
    assert_eq!(out.trim(), "-- -v a.txt", "got: {out:?}");
}

#[test]
fn dashdash_between_flags_and_paths_preserved() {
    // The git-diff-style invocation: revision/flags, then `--`, then paths.
    let (out, code) = run(&[
        "run",
        "sh",
        "-c",
        "printf '%s\\n' \"$@\"",
        "sh",
        "HEAD",
        "--",
        "path/to/file",
    ]);
    assert_eq!(code, Some(0));
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("HEAD"));
    assert_eq!(lines.next(), Some("--"));
    assert_eq!(lines.next(), Some("path/to/file"));
}

#[test]
fn double_dashdash_preserved() {
    // Pathological: two `--` tokens. Second must still arrive at the child
    // because clap only treats position-zero `--` specially (if at all).
    let (out, code) = run(&[
        "run",
        "sh",
        "-c",
        "printf '%s\\n' \"$@\"",
        "sh",
        "--",
        "-v",
        "--",
        "file.rs",
    ]);
    assert_eq!(code, Some(0));
    let mut lines = out.lines();
    assert_eq!(lines.next(), Some("--"));
    assert_eq!(lines.next(), Some("-v"));
    assert_eq!(lines.next(), Some("--"));
    assert_eq!(lines.next(), Some("file.rs"));
}

#[test]
fn direct_routing_preserves_dashdash() {
    // The direct-routing path (`hsp git diff -- foo`) bypasses the `run`
    // subcommand entirely, so clap isn't involved for the positional args.
    // Belt-and-braces check against a future change that routes through
    // clap.
    let (out, code) = run(&[
        "sh", // not a registered handler — passthrough
        "-c",
        "printf '%s\\n' \"$@\"",
        "sh",
        "HEAD",
        "--",
        "file.rs",
    ]);
    assert_eq!(code, Some(0));
    assert!(out.contains("--"), "got: {out:?}");
    assert!(out.contains("file.rs"));
}
