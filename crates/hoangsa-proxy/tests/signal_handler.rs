//! Signal-forwarding test. `hsp` is a thin proxy — when a signal arrives
//! (Ctrl+C from a terminal, SIGTERM from a process manager, cancellation
//! from an agent harness), we must kill the child too. Without this,
//! long-running proxied commands (`hsp cargo test` etc.) survive as
//! orphans after the user bails.

#![cfg(unix)]

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

fn hsp_bin() -> String {
    env!("CARGO_BIN_EXE_hsp").to_string()
}

/// Poll `pgrep -f PATTERN` until it either returns no hits (→ ok) or the
/// deadline elapses (→ panic). We use pgrep over `ps` because it handles
/// the PID→name lookup without parsing columns.
fn wait_for_no_match(pattern: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let out = Command::new("pgrep")
            .args(["-f", pattern])
            .output()
            .expect("pgrep");
        if !out.status.success() {
            // pgrep exit=1 when no match. That's what we want.
            return;
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        if stdout.trim().is_empty() {
            return;
        }
        if Instant::now() > deadline {
            panic!(
                "child still alive after {timeout:?}: pgrep -f {pattern:?} returned:\n{stdout}"
            );
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn kill_pid(pid: u32, sig: &str) {
    let _ = Command::new("kill")
        .args([&format!("-{sig}"), &pid.to_string()])
        .status();
}

#[test]
fn sigterm_on_hsp_kills_child() {
    // Sentinel: a unique marker in the sleep cmdline so pgrep can spot
    // only our child, not other sleeps on the machine.
    let marker = format!("hsp-sigterm-test-{}", std::process::id());
    let script = format!("exec -a {marker} sleep 60");

    let mut hsp = Command::new(hsp_bin())
        .args(["run", "sh", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hsp");

    // Give hsp + child ~300ms to start before we kill.
    thread::sleep(Duration::from_millis(300));

    // Child should be alive by now.
    let pgrep = Command::new("pgrep")
        .args(["-f", &marker])
        .output()
        .expect("pgrep");
    assert!(
        pgrep.status.success(),
        "child sleep never started; marker={marker}"
    );

    // Send SIGTERM to hsp. The handler must forward it to the child.
    kill_pid(hsp.id(), "TERM");

    // hsp itself must die, then child must follow.
    let _ = hsp.wait();
    wait_for_no_match(&marker, Duration::from_secs(3));
}

#[test]
fn sigint_on_hsp_kills_child() {
    let marker = format!("hsp-sigint-test-{}", std::process::id());
    let script = format!("exec -a {marker} sleep 60");

    let mut hsp = Command::new(hsp_bin())
        .args(["run", "sh", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn hsp");
    thread::sleep(Duration::from_millis(300));

    kill_pid(hsp.id(), "INT");
    let _ = hsp.wait();

    wait_for_no_match(&marker, Duration::from_secs(3));
}

#[test]
fn normal_exit_still_works_with_handler_installed() {
    // Regression: the signal handler install path must not break the
    // plain-happy case.
    let mut hsp = Command::new(hsp_bin())
        .args(["run", "sh", "-c", "echo hi"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");
    let mut buf = String::new();
    hsp.stdout.as_mut().unwrap().read_to_string(&mut buf).unwrap();
    let status = hsp.wait().unwrap();
    assert_eq!(status.code(), Some(0));
    assert_eq!(buf.trim(), "hi");
}
