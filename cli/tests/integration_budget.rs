use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
}

/// Run hoangsa-cli with the given args and an optional working directory.
/// `stats record` / `stats summary` read `std::env::current_dir()` directly,
/// so the subprocess must actually run from that directory when `cwd` is set.
fn run_cli(args: &[&str]) -> (String, String, bool) {
    run_cli_in(None, args)
}

fn run_cli_cwd(cwd: &str, args: &[&str]) -> (String, String, bool) {
    run_cli_in(Some(cwd), args)
}

fn run_cli_in(cwd: Option<&str>, args: &[&str]) -> (String, String, bool) {
    let mut cmd = cli();
    cmd.args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let output = cmd.output().expect("failed to run hoangsa-cli");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Parse a CLI stdout string as JSON, panicking with a clear message on failure.
fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|_| panic!("stdout must be valid JSON; got: {stdout}"))
}

/// Build a minimal plan.json for a single low-complexity task, write it into
/// `dir`, and return the path to the file.
fn write_minimal_plan(dir: &Path) -> PathBuf {
    let dir_str = dir.to_string_lossy();
    let src_file = dir.join("src").join("main.rs");
    fs::create_dir_all(src_file.parent().expect("parent exists"))
        .expect("create src dir");
    fs::write(&src_file, "fn main() {}").expect("write src/main.rs");

    let plan = serde_json::json!({
        "name": "test plan",
        "workspace_dir": dir_str,
        "budget_tokens": 10000,
        "tasks": [
            {
                "id": "T-01",
                "name": "Test task",
                "complexity": "low",
                "budget_tokens": 10000,
                "namespace": null,
                "files": [src_file.to_string_lossy()],
                "depends_on": [],
                "context_pointers": [],
                "covers": ["REQ-01"],
                "acceptance": "echo ok"
            }
        ]
    });

    let plan_path = dir.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&plan).expect("serialize plan"))
        .expect("write plan.json");
    plan_path
}

// ─── Tests ────────────────────────────────────────────────────────────────────

/// [REQ-01] `budget estimate <plan> T-01` should return valid JSON with the
/// three top-level fields that make up a full breakdown response.
#[test]
fn test_budget_estimate_outputs_valid_json() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let plan_path = write_minimal_plan(tmp.path());
    let plan_str = plan_path.to_string_lossy();

    let (stdout, stderr, success) = run_cli(&["budget", "estimate", &plan_str, "T-01"]);
    assert!(success, "expected success; stderr: {stderr}");

    let v = parse_json(&stdout);

    assert!(v.get("breakdown").is_some(), "missing 'breakdown' field");
    assert!(v.get("overhead_constants").is_some(), "missing 'overhead_constants' field");
    assert!(v.get("complexity_profile").is_some(), "missing 'complexity_profile' field");

    let bd = &v["breakdown"];
    assert!(bd.get("work_tokens").is_some(), "breakdown missing work_tokens");
    assert!(bd.get("system_prompt_effective").is_some(), "breakdown missing system_prompt_effective");
    assert!(bd.get("total").is_some(), "breakdown missing total");
}

/// `budget estimate <plan>` without a task_id should still succeed (falls back
/// to the first task in the plan) — or fail gracefully with an error JSON if
/// the plan has no tasks.  Either way the output must be valid JSON.
#[test]
fn test_budget_estimate_missing_task_id() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let plan_path = write_minimal_plan(tmp.path());
    let plan_str = plan_path.to_string_lossy();

    // Omit task_id — CLI should fall back to first task and succeed
    let (stdout, _stderr, _success) = run_cli(&["budget", "estimate", &plan_str]);
    let v = parse_json(&stdout);

    // Either a valid breakdown or an error object — both are acceptable JSON responses.
    // What must NOT happen is a panic / non-JSON output.
    let is_breakdown = v.get("breakdown").is_some();
    let is_error = v.get("error").is_some();
    assert!(
        is_breakdown || is_error,
        "expected either 'breakdown' or 'error' key; got: {v}"
    );

    // Test with a plan that truly has no tasks
    let dir_str = tmp.path().to_string_lossy().to_string();
    let empty_plan = serde_json::json!({
        "name": "empty plan",
        "workspace_dir": dir_str,
        "budget_tokens": 0,
        "tasks": []
    });
    let empty_path = tmp.path().join("empty_plan.json");
    fs::write(&empty_path, serde_json::to_string_pretty(&empty_plan).expect("serialize"))
        .expect("write empty_plan.json");

    let (empty_stdout, _empty_stderr, _) = run_cli(&["budget", "estimate", &empty_path.to_string_lossy()]);
    let empty_v = parse_json(&empty_stdout);
    assert!(empty_v.get("error").is_some(), "expected error for empty tasks plan; got: {empty_v}");
}

/// [REQ-02] `budget breakdown <plan>` with 3 tasks must return a `tasks` array
/// with exactly 3 entries and a `plan_total` object.
#[test]
fn test_budget_breakdown_outputs_all_tasks() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();

    // Create dummy source files for each task
    for i in 1..=3 {
        let src = dir.join(format!("task{i}.rs"));
        fs::write(&src, "fn main() {}").expect("write task file");
    }

    let plan = serde_json::json!({
        "name": "three task plan",
        "workspace_dir": dir_str,
        "budget_tokens": 30000,
        "tasks": [
            {
                "id": "T-01",
                "name": "Task one",
                "complexity": "low",
                "budget_tokens": 10000,
                "namespace": null,
                "files": [dir.join("task1.rs").to_string_lossy()],
                "depends_on": [],
                "context_pointers": [],
                "covers": ["REQ-01"],
                "acceptance": "echo ok"
            },
            {
                "id": "T-02",
                "name": "Task two",
                "complexity": "medium",
                "budget_tokens": 10000,
                "namespace": null,
                "files": [dir.join("task2.rs").to_string_lossy()],
                "depends_on": ["T-01"],
                "context_pointers": [],
                "covers": ["REQ-02"],
                "acceptance": "echo ok"
            },
            {
                "id": "T-03",
                "name": "Task three",
                "complexity": "high",
                "budget_tokens": 10000,
                "namespace": null,
                "files": [dir.join("task3.rs").to_string_lossy()],
                "depends_on": ["T-02"],
                "context_pointers": [],
                "covers": ["REQ-03"],
                "acceptance": "echo ok"
            }
        ]
    });

    let plan_path = dir.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&plan).expect("serialize plan"))
        .expect("write plan.json");

    let (stdout, stderr, success) = run_cli(&["budget", "breakdown", &plan_path.to_string_lossy()]);
    assert!(success, "expected success; stderr: {stderr}");

    let v = parse_json(&stdout);

    let tasks = v["tasks"].as_array().expect("'tasks' must be an array");
    assert_eq!(tasks.len(), 3, "expected 3 task entries in breakdown");

    assert!(v.get("plan_total").is_some(), "missing 'plan_total' field");
    assert!(
        v["plan_total"].get("estimated").is_some(),
        "plan_total missing 'estimated'"
    );
    assert!(
        v["plan_total"].get("breakdown_sum").is_some(),
        "plan_total missing 'breakdown_sum'"
    );
}

/// [REQ-03] [REQ-04] Record a single TaskUsageRecord, then verify `stats summary`
/// reports `total_records` = 1.
#[test]
fn test_stats_record_and_summary_roundtrip() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let cwd = tmp.path().to_string_lossy().to_string();

    let record = serde_json::json!({
        "task_id": "T-01",
        "session_id": "feat/test-session",
        "complexity": "low",
        "estimated_budget": 10000,
        "tracked_usage": 8000,
        "tool_calls_count": 7,
        "turns_count": 12,
        "content_tokens_sent": 4000,
        "content_tokens_received": 4000,
        "cache_scenario": "cold",
        "timestamp": "2026-04-20T00:00:00Z"
    });
    let record_str = serde_json::to_string(&record).expect("serialize record");

    let (record_out, record_err, record_ok) =
        run_cli_cwd(&cwd, &["stats", "record", &record_str]);
    assert!(record_ok, "stats record failed; stderr: {record_err}");

    let record_v = parse_json(&record_out);
    assert_eq!(record_v["success"], true, "expected success=true; got: {record_v}");
    assert_eq!(record_v["records_total"], 1, "expected records_total=1 after first record");

    let (summary_out, summary_err, summary_ok) = run_cli_cwd(&cwd, &["stats", "summary"]);
    assert!(summary_ok, "stats summary failed; stderr: {summary_err}");

    let summary_v = parse_json(&summary_out);
    assert_eq!(
        summary_v["total_records"], 1,
        "summary should show total_records=1; got: {summary_v}"
    );
}

/// [REQ-04] Record entries with different complexities, then verify that
/// `--last 1` returns only 1 filtered record, and `--complexity low` filters
/// correctly.
#[test]
fn test_stats_summary_with_filters() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let cwd = tmp.path().to_string_lossy().to_string();

    let make_record = |complexity: &str, estimated: u64, actual: u64| -> String {
        serde_json::to_string(&serde_json::json!({
            "task_id": format!("T-{}", complexity),
            "session_id": "feat/test",
            "complexity": complexity,
            "estimated_budget": estimated,
            "tracked_usage": actual,
            "tool_calls_count": 5,
            "turns_count": 10,
            "content_tokens_sent": 2000,
            "content_tokens_received": 2000,
            "cache_scenario": "cold",
            "timestamp": "2026-04-20T00:00:00Z"
        }))
        .expect("serialize record")
    };

    // Record three entries: low, medium, high
    for (complexity, est, act) in &[("low", 10000u64, 8000u64), ("medium", 20000, 18000), ("high", 40000, 35000)] {
        let record_str = make_record(complexity, *est, *act);
        let (_, stderr, success) = run_cli_cwd(&cwd, &["stats", "record", &record_str]);
        assert!(success, "stats record ({complexity}) failed; stderr: {stderr}");
    }

    // --last 1 should return filtered_records=1
    let (last_out, last_err, last_ok) = run_cli_cwd(&cwd, &["stats", "summary", "--last", "1"]);
    assert!(last_ok, "stats summary --last 1 failed; stderr: {last_err}");
    let last_v = parse_json(&last_out);
    assert_eq!(
        last_v["total_records"], 3,
        "total_records should still reflect all 3 stored records"
    );
    assert_eq!(
        last_v["filtered_records"], 1,
        "filtered_records should be 1 with --last 1; got: {last_v}"
    );

    // --complexity low should return only the low entry
    let (cx_out, cx_err, cx_ok) =
        run_cli_cwd(&cwd, &["stats", "summary", "--complexity", "low"]);
    assert!(cx_ok, "stats summary --complexity low failed; stderr: {cx_err}");
    let cx_v = parse_json(&cx_out);
    assert_eq!(
        cx_v["filtered_records"], 1,
        "filtered_records should be 1 for --complexity low; got: {cx_v}"
    );
    assert_eq!(
        cx_v["by_complexity"]["low"]["count"], 1,
        "by_complexity.low.count should be 1; got: {cx_v}"
    );
}

/// [REQ-05] A task with budget_tokens=90000 should trigger an "exceeds 80k"
/// warning when the plan is validated.
#[test]
fn test_validate_plan_80k_limit() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();

    let src_file = dir.join("main.rs");
    fs::write(&src_file, "fn main() {}").expect("write main.rs");

    let plan = serde_json::json!({
        "name": "oversized plan",
        "workspace_dir": dir_str,
        "budget_tokens": 90000,
        "tasks": [
            {
                "id": "T-01",
                "name": "Big task",
                "complexity": "high",
                "budget_tokens": 90000,
                "namespace": null,
                "files": [src_file.to_string_lossy()],
                "depends_on": [],
                "context_pointers": [],
                "covers": ["REQ-05"],
                "acceptance": "echo ok"
            }
        ]
    });

    let plan_path = dir.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&plan).expect("serialize plan"))
        .expect("write plan.json");

    // validate plan exits 0 even when there are warnings
    let (stdout, _stderr, _success) =
        run_cli(&["validate", "plan", &plan_path.to_string_lossy()]);
    let v = parse_json(&stdout);

    let warnings = v["warnings"]
        .as_array()
        .expect("'warnings' must be an array");

    let has_80k_warning = warnings.iter().any(|w| {
        w.as_str()
            .map(|s| s.contains("exceeds 80k"))
            .unwrap_or(false)
    });
    assert!(
        has_80k_warning,
        "expected a warning containing 'exceeds 80k'; got warnings: {warnings:?}"
    );
}

/// [REQ-11] Cold task (no depends_on) should have a higher
/// `system_prompt_effective` than warm task (has depends_on).
#[test]
fn test_budget_estimate_cold_vs_warm() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();

    let src_file = dir.join("main.rs");
    fs::write(&src_file, "fn main() {}").expect("write main.rs");
    let src_str = src_file.to_string_lossy();

    let plan = serde_json::json!({
        "name": "cold vs warm plan",
        "workspace_dir": dir_str,
        "budget_tokens": 50000,
        "tasks": [
            {
                "id": "T-cold",
                "name": "Cold task",
                "complexity": "medium",
                "budget_tokens": 25000,
                "namespace": null,
                "files": [&*src_str],
                "depends_on": [],
                "context_pointers": [],
                "covers": ["REQ-11"],
                "acceptance": "echo ok"
            },
            {
                "id": "T-warm",
                "name": "Warm task",
                "complexity": "medium",
                "budget_tokens": 25000,
                "namespace": null,
                "files": [&*src_str],
                "depends_on": ["T-cold"],
                "context_pointers": [],
                "covers": ["REQ-11"],
                "acceptance": "echo ok"
            }
        ]
    });

    let plan_path = dir.join("plan.json");
    fs::write(&plan_path, serde_json::to_string_pretty(&plan).expect("serialize plan"))
        .expect("write plan.json");
    let plan_str = plan_path.to_string_lossy();

    let (stdout_cold, stderr_cold, ok_cold) =
        run_cli(&["budget", "estimate", &plan_str, "T-cold"]);
    assert!(ok_cold, "budget estimate T-cold failed; stderr: {stderr_cold}");
    let v_cold = parse_json(&stdout_cold);

    let (stdout_warm, stderr_warm, ok_warm) =
        run_cli(&["budget", "estimate", &plan_str, "T-warm"]);
    assert!(ok_warm, "budget estimate T-warm failed; stderr: {stderr_warm}");
    let v_warm = parse_json(&stdout_warm);

    let cold_spe = v_cold["breakdown"]["system_prompt_effective"]
        .as_u64()
        .expect("T-cold breakdown.system_prompt_effective must be a u64");
    let warm_spe = v_warm["breakdown"]["system_prompt_effective"]
        .as_u64()
        .expect("T-warm breakdown.system_prompt_effective must be a u64");

    assert!(
        cold_spe > warm_spe,
        "cold system_prompt_effective ({cold_spe}) should be higher than warm ({warm_spe})"
    );

    // Cross-check cache_scenario labels
    assert_eq!(
        v_cold["breakdown"]["cache_scenario"], "cold",
        "T-cold should report cache_scenario=cold"
    );
    assert_eq!(
        v_warm["breakdown"]["cache_scenario"], "warm",
        "T-warm should report cache_scenario=warm"
    );
}
