//! P2 tests — hook rewrite must skip compound/piped/redirected commands
//! so downstream parsers see raw bytes.

use std::io::Write;
use std::process::{Command, Stdio};

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

fn hook(payload: &str) -> (serde_json::Value, i32) {
    let mut child = Command::new(hsp_bin())
        .args(["hook", "rewrite"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().expect("wait");
    let body: serde_json::Value =
        serde_json::from_slice(&out.stdout).unwrap_or(serde_json::json!({}));
    (body, out.status.code().unwrap_or(-1))
}

fn is_passthrough(v: &serde_json::Value) -> bool {
    v.as_object().map(|m| m.is_empty()).unwrap_or(false)
}

#[test]
fn pipe_to_wc_skips_rewrite() {
    // repro: grep | wc -l depends on EXACT line count.
    let (body, code) =
        hook(r#"{"tool_name":"Bash","tool_input":{"command":"grep -n foo x.txt | wc -l"}}"#);
    assert_eq!(code, 0);
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn redirect_to_file_skips_rewrite() {
    let (body, _) =
        hook(r#"{"tool_name":"Bash","tool_input":{"command":"grep -n foo x.txt > out.txt"}}"#);
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn append_redirect_skips() {
    let (body, _) =
        hook(r#"{"tool_name":"Bash","tool_input":{"command":"git log >> audit.log"}}"#);
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn command_substitution_skips() {
    let (body, _) = hook(
        r#"{"tool_name":"Bash","tool_input":{"command":"echo $(git log -1)"}}"#,
    );
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn backtick_substitution_skips() {
    let (body, _) = hook(
        r#"{"tool_name":"Bash","tool_input":{"command":"echo `git log -1`"}}"#,
    );
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn and_chain_skips() {
    let (body, _) = hook(
        r#"{"tool_name":"Bash","tool_input":{"command":"cd foo && git log"}}"#,
    );
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn semicolon_chain_skips() {
    let (body, _) = hook(
        r#"{"tool_name":"Bash","tool_input":{"command":"git status; git log"}}"#,
    );
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn background_skips() {
    let (body, _) =
        hook(r#"{"tool_name":"Bash","tool_input":{"command":"git log &"}}"#);
    assert!(is_passthrough(&body), "got: {body}");
}

#[test]
fn quoted_pipe_inside_arg_still_rewrites() {
    // `grep "hi | there" file` — pipe is inside a double-quoted pattern,
    // not a shell operator. We SHOULD still rewrite.
    let (body, _) = hook(
        r#"{"tool_name":"Bash","tool_input":{"command":"grep \"hi | there\" x.txt"}}"#,
    );
    assert!(
        !is_passthrough(&body),
        "quoted pipe must not block rewrite, got: {body}"
    );
    let rewritten = body["hookSpecificOutput"]["modifiedToolInput"]["command"]
        .as_str()
        .unwrap();
    assert!(rewritten.starts_with("hsp "), "got: {rewritten}");
}

#[test]
fn single_quoted_pipe_still_rewrites() {
    let (body, _) = hook(
        r#"{"tool_name":"Bash","tool_input":{"command":"grep 'foo|bar' x.txt"}}"#,
    );
    assert!(!is_passthrough(&body), "got: {body}");
}

#[test]
fn escaped_pipe_still_rewrites() {
    // `git log \| cat` — escaped pipe is semantically two args to git, no
    // subshell. This is a pathological case; current impl treats ESC then
    // the pipe byte as an escape pair, so `|` is skipped → rewrite fires.
    let (body, _) = hook(
        r#"{"tool_name":"Bash","tool_input":{"command":"git log \\| cat"}}"#,
    );
    assert!(!is_passthrough(&body), "got: {body}");
}

#[test]
fn plain_command_still_rewrites() {
    let (body, _) = hook(r#"{"tool_name":"Bash","tool_input":{"command":"git log -5"}}"#);
    assert!(!is_passthrough(&body));
    assert_eq!(
        body["hookSpecificOutput"]["modifiedToolInput"]["command"],
        "hsp git log -5"
    );
}
