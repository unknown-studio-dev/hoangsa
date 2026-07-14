use serde_json::Value;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Run the hoangsa-cli binary with args, piping `stdin_data` to its stdin.
/// Returns (stdout, stderr, success).
fn run_cli_stdin(args: &[&str], stdin_data: &str) -> (String, String, bool) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hoangsa-cli");

    if let Some(mut s) = child.stdin.take() {
        s.write_all(stdin_data.as_bytes())
            .expect("write stdin");
    }
    let output = child.wait_with_output().expect("wait for output");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Parse stdout as JSON, panicking with context on failure.
fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout must be valid JSON ({e}); got: {stdout}"))
}

/// Create a minimal hoangsa project fixture so `is_hoangsa_project` and
/// `append_event` work (they gate on `.hoangsa/config.json` existing).
fn make_fixture(dir: &Path) {
    let hoangsa = dir.join(".hoangsa");
    fs::create_dir_all(hoangsa.join("state")).expect("create .hoangsa/state");
    fs::write(hoangsa.join("config.json"), "{}").expect("write config.json");
}

fn enforcement_events_path(dir: &Path) -> std::path::PathBuf {
    dir.join(".hoangsa").join("state").join("enforcement.events")
}

// ─── Test 5: cli_prompt_guard_end_to_end ──────────────────────────────────────

#[test]
fn cli_prompt_guard_end_to_end() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let fixture = tmp.path();
    make_fixture(fixture);
    let cwd_str = fixture.to_str().expect("fixture path str");

    // ── (a) frustration prompt → hookSpecificOutput with additionalContext
    //        + enforcement events log contains a frustration event ────────────
    let frustration_payload =
        r#"{"prompt":"đm lại lỗi nữa à"}"#;

    let (stdout, stderr, ok) = run_cli_stdin(
        &["hook", "prompt-guard", "--cwd", cwd_str],
        frustration_payload,
    );
    assert!(
        ok,
        "prompt-guard must exit 0 on frustration; stderr: {stderr}"
    );

    let v = parse_json(&stdout);
    let additional_context = v
        .get("hookSpecificOutput")
        .and_then(|h| h.get("additionalContext"))
        .and_then(|c| c.as_str())
        .unwrap_or_else(|| panic!("expected hookSpecificOutput.additionalContext; got: {v}"));
    assert!(
        additional_context.contains("frustration") || additional_context.contains("lesson"),
        "additionalContext must mention frustration or lesson; got: {additional_context}"
    );

    // Verify enforcement events log contains a frustration event
    let events_path = enforcement_events_path(fixture);
    assert!(
        events_path.exists(),
        "enforcement.events must be created after prompt-guard detects frustration"
    );
    let events_text = fs::read_to_string(&events_path).expect("read enforcement.events");
    let has_frustration = events_text
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .any(|e| e.get("event").and_then(|v| v.as_str()) == Some("frustration"));
    assert!(
        has_frustration,
        "enforcement.events must contain a frustration event; content:\n{events_text}"
    );

    // ── (b) Stop payload → decision block mentioning lesson (frustration block)
    let stop_payload = r#"{"stop_hook_active":false}"#;

    let (stdout2, stderr2, ok2) = run_cli_stdin(
        &["hook", "stop-check", "--cwd", cwd_str],
        stop_payload,
    );
    assert!(
        ok2,
        "stop-check must exit 0; stderr: {stderr2}"
    );
    let v2 = parse_json(&stdout2);
    // Should be a frustration block because there's a frustration event but no lesson_saved
    let decision = v2.get("decision").and_then(|d| d.as_str()).unwrap_or("");
    let reason = v2.get("reason").and_then(|r| r.as_str()).unwrap_or("");
    assert_eq!(
        decision, "block",
        "stop-check must block when frustration has no lesson_saved; stdout: {stdout2}"
    );
    assert!(
        reason.contains("lesson") || reason.contains("FRUSTRATION"),
        "block reason must mention lesson; got: {reason}"
    );

    // ── (c) Append a lesson_saved event → stop-check no longer frustration-blocks
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(&events_path)
        .expect("open events for append");
    writeln!(f, r#"{{"event":"lesson_saved"}}"#).expect("append lesson_saved");
    drop(f);

    // Reset sentinel so stop-check re-evaluates (it short-circuits on sentinel)
    let sentinel = fixture
        .join(".hoangsa")
        .join("state")
        .join("reflected.sentinel");
    if sentinel.exists() {
        fs::remove_file(&sentinel).expect("remove sentinel");
    }

    let (stdout3, stderr3, ok3) = run_cli_stdin(
        &["hook", "stop-check", "--cwd", cwd_str],
        stop_payload,
    );
    assert!(
        ok3,
        "stop-check must exit 0 after lesson_saved; stderr: {stderr3}"
    );
    let v3 = parse_json(&stdout3);
    let decision3 = v3.get("decision").and_then(|d| d.as_str()).unwrap_or("");
    let reason3 = v3.get("reason").and_then(|r| r.as_str()).unwrap_or("");
    // Must NOT be a frustration-block. It will be a normal memory-reflect prompt
    // (because we have work events and no sentinel) — that is correct and expected.
    assert!(
        !(decision3 == "block" && (reason3.contains("FRUSTRATION BLOCK"))),
        "after lesson_saved, stop-check must not frustration-block; stdout: {stdout3}"
    );
    // The normal reflect prompt or approve is both acceptable here
    if decision3 == "block" {
        // Must be the normal memory-reflect prompt, not the frustration block
        assert!(
            reason3.contains("memory-reflect"),
            "if blocking, must be normal memory-reflect prompt (not frustration); got: {reason3}"
        );
    }
}
