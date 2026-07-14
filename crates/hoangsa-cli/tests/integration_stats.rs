use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn run_cli(args: &[&str]) -> (String, String, bool) {
    let output = Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
        .args(args)
        .output()
        .expect("failed to run hoangsa-cli");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Parse a CLI stdout string as JSON, panicking with a clear message on failure.
fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|_| panic!("stdout must be valid JSON; got: {stdout}"))
}

/// Write one session fixture: phase-stats.jsonl lines plus optional plan.json.
fn write_session(session_dir: &Path, jsonl_lines: &[&str], plan: Option<&Value>) {
    fs::create_dir_all(session_dir).expect("create session dir");
    fs::write(
        session_dir.join("phase-stats.jsonl"),
        format!("{}\n", jsonl_lines.join("\n")),
    )
    .expect("write phase-stats.jsonl");
    if let Some(p) = plan {
        fs::write(
            session_dir.join("plan.json"),
            serde_json::to_string_pretty(p).expect("serialize plan"),
        )
        .expect("write plan.json");
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn report_all_empty_workspace_is_valid() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path().to_str().expect("tempdir path str");

    let (stdout, stderr, ok) = run_cli(&["stats", "report", "--all", dir]);
    assert!(ok, "exit status must be success; stderr: {stderr}");

    let v = parse_json(&stdout);
    assert!(
        v.get("error").is_none(),
        "output must not contain an error field: {v}"
    );
    assert_eq!(v["sessions"], serde_json::json!([]));
    assert_eq!(v["totals"]["sessions"], 0);
    assert_eq!(v["totals"]["total_tokens"], 0);
    assert_eq!(v["totals"]["completed_tasks"], 0);
    assert_eq!(v["totals"]["failed_tasks"], 0);
    assert_eq!(v["totals"]["fix_rounds"], 0);
    assert_eq!(v["totals"]["tokens_per_completed_task"], 0);
}

#[test]
fn cli_stats_report_all_end_to_end() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let root = tmp.path();
    write_session(
        &root.join(".hoangsa/sessions/feat/alpha"),
        &[
            r#"{"phase":"prepare","tokens":1000}"#,
            r#"{"phase":"cook","tokens":3000}"#,
            r#"{"phase":"fix","tokens":500}"#,
        ],
        Some(&serde_json::json!({ "tasks": [
            { "id": "T-01", "status": "completed" },
            { "id": "T-02", "status": "failed" },
            { "id": "T-03", "status": "completed" },
        ]})),
    );
    write_session(
        &root.join(".hoangsa/sessions/fix/beta"),
        &[
            r#"{"phase":"prepare","tokens":700}"#,
            r#"{"phase":"cook","tokens":800}"#,
        ],
        Some(&serde_json::json!({ "tasks": [{ "id": "T-01", "status": "completed" }] })),
    );

    let (stdout, stderr, ok) = run_cli(&[
        "stats",
        "report",
        "--all",
        root.to_str().expect("root path str"),
    ]);
    assert!(ok, "exit status must be success; stderr: {stderr}");

    let v = parse_json(&stdout);
    let sessions = v["sessions"].as_array().expect("sessions must be an array");
    assert_eq!(sessions.len(), 2, "both fixture sessions must be reported");

    // Sessions are sorted by id.
    assert_eq!(sessions[0]["id"], "feat/alpha");
    assert_eq!(sessions[0]["total_tokens"], 4500);
    assert_eq!(sessions[0]["phases"]["prepare"], 1000);
    assert_eq!(sessions[0]["phases"]["cook"], 3000);
    assert_eq!(sessions[0]["phases"]["fix"], 500);
    assert_eq!(sessions[0]["tasks"]["total"], 3);
    assert_eq!(sessions[0]["tasks"]["completed"], 2);
    assert_eq!(sessions[0]["tasks"]["failed"], 1);
    assert_eq!(sessions[0]["fix_rounds"], 1);

    assert_eq!(sessions[1]["id"], "fix/beta");
    assert_eq!(sessions[1]["total_tokens"], 1500);
    assert_eq!(sessions[1]["tasks"]["total"], 1);
    assert_eq!(sessions[1]["tasks"]["completed"], 1);

    assert_eq!(v["totals"]["sessions"], 2);
    assert_eq!(v["totals"]["total_tokens"], 6000);
    assert_eq!(v["totals"]["completed_tasks"], 3);
    assert_eq!(v["totals"]["failed_tasks"], 1);
    assert_eq!(v["totals"]["fix_rounds"], 1);
    // 6000 total tokens / 3 completed tasks
    assert_eq!(v["totals"]["tokens_per_completed_task"], 2000);
}
