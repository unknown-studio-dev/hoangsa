//! End-to-end exit-code passthrough audit. The `hsp` binary must never
//! invent, wrap, or swallow a child's exit status — a  parallel tool-call harness (Claude Code) will cancel peer tasks
//! when a proxy misreports an expected non-zero as an error.
//!
//! Every case here runs the real binary with `hsp run` and asserts the
//! child's exit code flows through unchanged.

use std::io::Write;
use std::process::{Command, Stdio};

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

#[cfg(unix)]
fn run_via_hsp(shell_snippet: &str) -> (String, String, i32) {
    // `hsp run <cmd> <args…>`. We spawn `sh -c <snippet>` under hsp so the
    // snippet can control its own exit code freely.
    let child = Command::new(hsp_bin())
        .args(["run", "sh", "-c", shell_snippet])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hsp");
    let output = child.wait_with_output().expect("wait hsp");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

#[cfg(unix)]
#[test]
fn exit_zero_passes_through() {
    let (_out, _err, code) = run_via_hsp("true");
    assert_eq!(code, 0);
}

#[cfg(unix)]
#[test]
fn exit_one_passes_through() {
    // grep-style "no match" exit — MUST stay 1, not get wrapped into a hsp
    // error.
    let (_out, _err, code) = run_via_hsp("exit 1");
    assert_eq!(code, 1, "grep-style exit 1 must pass through unchanged");
}

#[cfg(unix)]
#[test]
fn exit_101_passes_through() {
    // cargo test panic exit
    let (_out, _err, code) = run_via_hsp("exit 101");
    assert_eq!(code, 101);
}

#[cfg(unix)]
#[test]
fn exit_42_passes_through() {
    let (_out, _err, code) = run_via_hsp("exit 42");
    assert_eq!(code, 42);
}

#[cfg(unix)]
#[test]
fn exit_255_passes_through() {
    // Near the u8 wrap boundary. ExitCode is u8; upstream exit must be
    // truncated consistently (0xff & 255 = 255).
    let (_out, _err, code) = run_via_hsp("exit 255");
    assert_eq!(code, 255);
}

#[cfg(unix)]
#[test]
fn killed_by_sigterm_reports_signal_exit() {
    // Child that kills itself with SIGTERM (15). exec::run maps signal kills
    // to 128+signum (standard POSIX shell convention).
    let (_out, _err, code) = run_via_hsp("kill -TERM $$; sleep 1");
    assert_eq!(code, 128 + 15);
}

#[cfg(unix)]
#[test]
fn binary_not_found_returns_127() {
    // `hsp run <nonexistent>`: we bypass sh entirely so exec::run hits the
    // spawn error path directly.
    let child = Command::new(hsp_bin())
        .args(["run", "this-command-does-not-exist-hsp-test"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn hsp");
    assert_eq!(child.status.code(), Some(127));
    let err = String::from_utf8_lossy(&child.stderr);
    assert!(err.contains("exec failed"), "stderr = {err:?}");
}

#[cfg(unix)]
#[test]
fn ansi_strip_on_piped_stdout() {
    // A command emitting SGR color codes must land on our piped stdout
    // with the codes stripped (we're not a TTY here).
    let (out, _err, code) = run_via_hsp("printf '\\033[38;5;231mtest\\033[0m\\n'");
    assert_eq!(code, 0);
    assert_eq!(out, "test\n", "ANSI must be stripped when stdout is piped");
}

#[cfg(unix)]
#[test]
fn ansi_kept_with_keep_color_flag() {
    let child = Command::new(hsp_bin())
        .args([
            "run",
            "--keep-color",
            "sh",
            "-c",
            "printf '\\033[31mred\\033[0m\\n'",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hsp");
    let output = child.wait_with_output().expect("wait hsp");
    let out = String::from_utf8_lossy(&output.stdout);
    assert_eq!(output.status.code(), Some(0));
    assert!(
        out.contains("\x1b[31m"),
        "--keep-color must preserve SGR; got {out:?}"
    );
}

#[cfg(unix)]
#[test]
fn no_color_env_strips_even_on_forced_run() {
    // Even without a flag, NO_COLOR=1 forces strip. This mirrors the
    // no-color.org convention so users can kill color globally.
    let mut cmd = Command::new(hsp_bin());
    cmd.args(["run", "sh", "-c", "printf '\\033[31mred\\033[0m'"])
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let out = cmd.output().expect("spawn hsp");
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "red");
}

#[cfg(unix)]
#[test]
fn mutually_exclusive_color_flags_error() {
    let out = Command::new(hsp_bin())
        .args(["run", "--no-color", "--keep-color", "true"])
        .output()
        .expect("spawn hsp");
    assert_eq!(out.status.code(), Some(2));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("mutually exclusive"), "stderr = {err:?}");
}

#[cfg(unix)]
#[test]
fn stderr_exit_code_preserved_with_stderr_output() {
    // Command writes to stderr AND exits non-zero. Both must survive.
    let (out, err, code) = run_via_hsp("printf 'err line' >&2; exit 2");
    assert_eq!(code, 2);
    assert!(err.contains("err line"), "stderr = {err:?}");
    assert!(out.is_empty() || !out.contains("err line"));
}

#[cfg(unix)]
#[test]
fn direct_routing_preserves_exit_code() {
    // `hsp sh -c 'exit 7'` — bypasses the `run` subcommand and takes the
    // direct-routing path. Exit code path audit.
    //
    // `sh` is in the known-commands list? No — it's not. The direct router
    // only catches cmds in RESERVED-check (anything non-reserved runs
    // through proxy_run). So `sh` goes through proxy_run with no filter
    // registered — filter_result is default, exit should pass through.
    let out = Command::new(hsp_bin())
        .args(["sh", "-c", "exit 9"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn hsp");
    assert_eq!(out.status.code(), Some(9));
}

#[cfg(unix)]
#[test]
fn unknown_flag_passes_through_to_child() {
    // Subtle: `hsp run --weird-flag-for-child cmd` — clap on `hsp run`
    // should consume it or reject. Test intent: a user flag our CLI doesn't
    // know about must not crash hsp with a bogus exit.
    //
    // Since `run` uses trailing_var_arg(true) after `args`, the first non-
    // flag positional starts args. A `--unknown` before the command would
    // hit clap. This test just documents behaviour — we expect a clap
    // usage error (exit 2), NOT a silent wrap.
    let out = Command::new(hsp_bin())
        .args(["run", "--definitely-not-a-flag", "true"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn hsp");
    assert_eq!(out.status.code(), Some(2));
}

/// Builder helper for use in streaming tests later.
#[cfg(unix)]
#[allow(dead_code)]
fn run_via_hsp_stdin(shell_snippet: &str, stdin_data: &str) -> (String, String, i32) {
    let mut child = Command::new(hsp_bin())
        .args(["run", "sh", "-c", shell_snippet])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn hsp");
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_data.as_bytes())
        .unwrap();
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("wait hsp");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}
