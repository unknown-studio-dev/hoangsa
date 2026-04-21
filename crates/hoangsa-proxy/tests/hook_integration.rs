//! Integration test for `hsp hook rewrite`: invokes the binary with a JSON
//! payload on stdin and checks the rewrite decision.

use std::io::Write;
use std::process::{Command, Stdio};

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

fn run_hook(stdin: &str) -> (String, i32) {
    let mut child = Command::new(hsp_bin())
        .args(["hook", "rewrite"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hsp");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("wait hsp");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

#[test]
fn rewrites_known_command() {
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git log -5"}}"#;
    let (out, code) = run_hook(payload);
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["decision"], "approve");
    let rewritten = v["hookSpecificOutput"]["modifiedToolInput"]["command"]
        .as_str()
        .unwrap();
    assert_eq!(rewritten, "hsp git log -5");
}

#[test]
fn passes_through_unknown_command() {
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"rustc --version"}}"#;
    let (out, code) = run_hook(payload);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "{}");
}

#[test]
fn passes_through_non_bash_tool() {
    let payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"foo.rs"}}"#;
    let (out, code) = run_hook(payload);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "{}");
}

#[test]
fn does_not_double_wrap() {
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"hsp git log"}}"#;
    let (out, code) = run_hook(payload);
    assert_eq!(code, 0);
    assert_eq!(out.trim(), "{}");
}
