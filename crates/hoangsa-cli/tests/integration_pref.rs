use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn cli() -> Command {
    Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
}

fn run_cli(args: &[&str]) -> (String, String, bool) {
    let mut cmd = cli();
    cmd.args(args);
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

/// Create a minimal .hoangsa/config.json in `dir` and return its path.
fn init_config(dir: &Path) -> PathBuf {
    let hoangsa_dir = dir.join(".hoangsa");
    fs::create_dir_all(&hoangsa_dir).expect("create .hoangsa dir");
    let config_path = hoangsa_dir.join("config.json");
    // Write a bare config — pref set will populate preferences on first use
    let minimal = serde_json::json!({
        "profile": "balanced",
        "preferences": {
            "lang": null,
            "spec_lang": null,
            "tech_stack": [],
            "interaction_level": null,
            "auto_taste": null,
            "auto_plate": null,
            "auto_serve": null,
            "research_scope": null,
            "research_mode": null,
            "review_style": null,
            "simplify_pass": false,
            "quality_gate": false,
            "test_runs": 1,
            "context_mode": "selective",
            "memory_strict": false
        },
        "task_manager": {
            "provider": null,
            "mcp_server": null,
            "verified": false,
            "verified_at": null,
            "project_id": null,
            "default_list": null
        }
    });
    fs::write(
        &config_path,
        serde_json::to_string_pretty(&minimal).expect("serialize config"),
    )
    .expect("write config.json");
    config_path
}

/// Write a plan.json into `session_dir` that references files inside
/// `workspace_dir`.  The task uses line-range file specs when `use_line_range`
/// is true.
fn write_plan_with_context(
    session_dir: &Path,
    workspace_dir: &Path,
    use_line_range: bool,
) -> PathBuf {
    // Create a real source file with enough content that a line-range matters.
    let src_file = workspace_dir.join("lib.rs");
    let src_content = (1..=20)
        .map(|i| format!("// line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&src_file, &src_content).expect("write lib.rs");

    let file_spec = if use_line_range {
        format!("{}:3-7", src_file.to_string_lossy())
    } else {
        src_file.to_string_lossy().to_string()
    };

    let plan = serde_json::json!({
        "name": "pref integration plan",
        "workspace_dir": workspace_dir.to_string_lossy(),
        "budget_tokens": 10000,
        "tasks": [
            {
                "id": "T-01",
                "name": "Context test task",
                "complexity": "low",
                "budget_tokens": 10000,
                "namespace": null,
                "files": [file_spec],
                "depends_on": [],
                "context_pointers": [],
                "covers": ["REQ-07"],
                "acceptance": "echo ok"
            }
        ]
    });

    let plan_path = session_dir.join("plan.json");
    fs::write(
        &plan_path,
        serde_json::to_string_pretty(&plan).expect("serialize plan"),
    )
    .expect("write plan.json");
    plan_path
}

// ─── Profile Roundtrip Tests ──────────────────────────────────────────────────

/// [REQ-01] Setting profile=balanced must write the 6 balanced preset values
/// into config.json preferences.
#[test]
fn test_profile_roundtrip_balanced() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();
    init_config(dir);

    let (stdout, stderr, success) = run_cli(&["pref", "set", &dir_str, "profile", "balanced"]);
    assert!(success, "pref set profile balanced failed; stderr: {stderr}");

    let v = parse_json(&stdout);
    assert_eq!(v["success"], true, "expected success=true; got: {v}");
    assert_eq!(v["profile"], "balanced", "expected profile=balanced; got: {v}");

    // Verify each of the 6 keys via pref get
    let expected: &[(&str, Value)] = &[
        ("simplify_pass", Value::Bool(false)),
        ("quality_gate", Value::Bool(false)),
        ("test_runs", serde_json::json!(1)),
        ("research_mode", Value::String("inline".into())),
        ("context_mode", Value::String("selective".into())),
        ("memory_strict", Value::Bool(false)),
    ];

    for (key, expected_value) in expected {
        let (get_out, get_err, get_ok) = run_cli(&["pref", "get", &dir_str, key]);
        assert!(get_ok, "pref get {key} failed; stderr: {get_err}");
        let gv = parse_json(&get_out);
        assert_eq!(
            &gv["value"], expected_value,
            "balanced profile: key={key} expected={expected_value}; got: {gv}"
        );
    }
}

/// [REQ-01] Setting profile=full must write the 6 full preset values into
/// config.json preferences.
#[test]
fn test_profile_roundtrip_full() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();
    init_config(dir);

    let (stdout, stderr, success) = run_cli(&["pref", "set", &dir_str, "profile", "full"]);
    assert!(success, "pref set profile full failed; stderr: {stderr}");

    let v = parse_json(&stdout);
    assert_eq!(v["success"], true, "expected success=true; got: {v}");
    assert_eq!(v["profile"], "full", "expected profile=full; got: {v}");

    let expected: &[(&str, Value)] = &[
        ("simplify_pass", Value::Bool(true)),
        ("quality_gate", Value::Bool(true)),
        ("test_runs", serde_json::json!(3)),
        ("research_mode", Value::String("full".into())),
        ("context_mode", Value::String("full".into())),
        ("memory_strict", Value::Bool(true)),
    ];

    for (key, expected_value) in expected {
        let (get_out, get_err, get_ok) = run_cli(&["pref", "get", &dir_str, key]);
        assert!(get_ok, "pref get {key} failed; stderr: {get_err}");
        let gv = parse_json(&get_out);
        assert_eq!(
            &gv["value"], expected_value,
            "full profile: key={key} expected={expected_value}; got: {gv}"
        );
    }
}

// ─── Individual Override Tests ────────────────────────────────────────────────

/// [REQ-01] After setting profile=balanced an individual key override must
/// change only that key; the other 5 balanced keys must remain unchanged.
#[test]
fn test_individual_override_after_balanced_profile() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();
    init_config(dir);

    // First apply balanced profile
    let (_, err, ok) = run_cli(&["pref", "set", &dir_str, "profile", "balanced"]);
    assert!(ok, "pref set profile balanced failed; stderr: {err}");

    // Now override one key
    let (set_out, set_err, set_ok) =
        run_cli(&["pref", "set", &dir_str, "simplify_pass", "true"]);
    assert!(set_ok, "pref set simplify_pass true failed; stderr: {set_err}");
    let sv = parse_json(&set_out);
    assert_eq!(sv["success"], true, "expected success=true; got: {sv}");
    assert_eq!(sv["value"], true, "expected value=true; got: {sv}");

    // Overridden key must now be true
    let (get_out, get_err, get_ok) = run_cli(&["pref", "get", &dir_str, "simplify_pass"]);
    assert!(get_ok, "pref get simplify_pass failed; stderr: {get_err}");
    let gv = parse_json(&get_out);
    assert_eq!(gv["value"], true, "simplify_pass must be true after override; got: {gv}");

    // The other 5 balanced keys must remain at their balanced defaults
    let unchanged: &[(&str, Value)] = &[
        ("quality_gate", Value::Bool(false)),
        ("test_runs", serde_json::json!(1)),
        ("research_mode", Value::String("inline".into())),
        ("context_mode", Value::String("selective".into())),
        ("memory_strict", Value::Bool(false)),
    ];

    for (key, expected_value) in unchanged {
        let (gout, gerr, gok) = run_cli(&["pref", "get", &dir_str, key]);
        assert!(gok, "pref get {key} failed; stderr: {gerr}");
        let gv2 = parse_json(&gout);
        assert_eq!(
            &gv2["value"], expected_value,
            "after simplify_pass override, key={key} should still be balanced value {expected_value}; got: {gv2}"
        );
    }
}

/// [REQ-02] `pref set` must store integers as JSON numbers, not strings.
/// Reading back test_runs=3 must return the number 3, not the string "3".
#[test]
fn test_integer_coercion_roundtrip() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();
    init_config(dir);

    let (set_out, set_err, set_ok) = run_cli(&["pref", "set", &dir_str, "test_runs", "3"]);
    assert!(set_ok, "pref set test_runs 3 failed; stderr: {set_err}");
    let sv = parse_json(&set_out);
    assert_eq!(sv["success"], true, "expected success=true; got: {sv}");

    let (get_out, get_err, get_ok) = run_cli(&["pref", "get", &dir_str, "test_runs"]);
    assert!(get_ok, "pref get test_runs failed; stderr: {get_err}");
    let gv = parse_json(&get_out);

    // Must be numeric 3, not string "3"
    assert!(
        gv["value"].is_number(),
        "test_runs value must be a JSON number, not a string; got: {}",
        gv["value"]
    );
    assert_eq!(
        gv["value"].as_i64(),
        Some(3),
        "test_runs must equal 3 as an integer; got: {gv}"
    );
}

/// [REQ-02] Verify `pref get` without a key returns all preferences as an
/// object, confirming stored integers are numbers in the full preferences dump.
#[test]
fn test_pref_get_all_after_set() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();
    init_config(dir);

    run_cli(&["pref", "set", &dir_str, "test_runs", "5"]);
    run_cli(&["pref", "set", &dir_str, "simplify_pass", "true"]);

    let (get_out, get_err, get_ok) = run_cli(&["pref", "get", &dir_str]);
    assert!(get_ok, "pref get (all) failed; stderr: {get_err}");
    let gv = parse_json(&get_out);

    assert!(gv.is_object(), "pref get (all) must return a JSON object; got: {gv}");
    assert!(
        gv["test_runs"].is_number(),
        "test_runs in full dump must be a JSON number; got: {}",
        gv["test_runs"]
    );
    assert_eq!(
        gv["test_runs"].as_i64(),
        Some(5),
        "test_runs must be 5; got: {gv}"
    );
    assert_eq!(
        gv["simplify_pass"], true,
        "simplify_pass must be true; got: {gv}"
    );
}

/// [REQ-01] Requesting an unknown profile must return an error JSON, not panic.
#[test]
fn test_unknown_profile_returns_error() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    let dir_str = dir.to_string_lossy();
    init_config(dir);

    let (stdout, _stderr, _success) =
        run_cli(&["pref", "set", &dir_str, "profile", "turbo_mode"]);
    let v = parse_json(&stdout);
    assert!(
        v.get("error").is_some(),
        "unknown profile must return an error; got: {v}"
    );
}

// ─── Context Selective Tests ──────────────────────────────────────────────────

/// [REQ-07] When context_mode=selective and a file spec has a line range,
/// `context get` must return file_segments whose lines contain only the
/// requested range, not the full file.
#[test]
fn test_context_selective_line_range() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let workspace_dir = tmp.path();
    let session_dir = workspace_dir; // plan.json lives in workspace for simplicity
    let workspace_str = workspace_dir.to_string_lossy();

    // Set context_mode=selective in workspace config
    init_config(workspace_dir);
    let (_, set_err, set_ok) =
        run_cli(&["pref", "set", &workspace_str, "context_mode", "selective"]);
    assert!(set_ok, "pref set context_mode selective failed; stderr: {set_err}");

    // Write plan.json with a line-range file spec (lines 3-7)
    write_plan_with_context(session_dir, workspace_dir, true);

    let (stdout, stderr, success) =
        run_cli(&["context", "get", &workspace_str, "T-01"]);
    assert!(success, "context get failed; stderr: {stderr}");

    let v = parse_json(&stdout);
    assert!(
        v.get("error").is_none(),
        "context get must not return an error; got: {v}"
    );

    let segments = v["file_segments"]
        .as_array()
        .expect("file_segments must be an array");
    assert!(
        !segments.is_empty(),
        "file_segments must not be empty; got: {v}"
    );

    let seg = &segments[0];
    let start_line = seg["start_line"].as_u64().expect("start_line must be a number");
    let end_line = seg["end_line"].as_u64().expect("end_line must be a number");

    assert_eq!(start_line, 3, "selective mode: start_line must be 3; got: {seg}");
    assert!(
        end_line <= 7,
        "selective mode: end_line must be <= 7; got end_line={end_line}"
    );

    // The extracted lines must not contain line numbers outside the range.
    let lines_text = seg["lines"].as_str().expect("lines must be a string");
    for line in lines_text.lines() {
        // Each line is "// line N" — verify N is in [3, 7]
        if let Some(n_str) = line.strip_prefix("// line ") {
            if let Ok(n) = n_str.trim().parse::<usize>() {
                assert!(
                    n >= 3 && n <= 7,
                    "selective mode: line {n} is outside the requested range 3-7"
                );
            }
        }
    }
}

/// [REQ-07] When context_mode=full, `context get` returns the entire file
/// even when the file spec contains a line range — the range is ignored in
/// full mode.
#[test]
fn test_context_full_mode_ignores_line_range() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let workspace_dir = tmp.path();
    let session_dir = workspace_dir;
    let workspace_str = workspace_dir.to_string_lossy();

    init_config(workspace_dir);
    let (_, set_err, set_ok) =
        run_cli(&["pref", "set", &workspace_str, "context_mode", "full"]);
    assert!(set_ok, "pref set context_mode full failed; stderr: {set_err}");

    // Plan uses a line-range spec but context_mode is full — all lines expected
    write_plan_with_context(session_dir, workspace_dir, true);

    let (stdout, stderr, success) =
        run_cli(&["context", "get", &workspace_str, "T-01"]);
    assert!(success, "context get failed; stderr: {stderr}");

    let v = parse_json(&stdout);
    assert!(v.get("error").is_none(), "context get must not return an error; got: {v}");

    let segments = v["file_segments"]
        .as_array()
        .expect("file_segments must be an array");
    assert!(!segments.is_empty(), "file_segments must not be empty; got: {v}");

    let seg = &segments[0];
    let start_line = seg["start_line"].as_u64().expect("start_line must be a number");
    let end_line = seg["end_line"].as_u64().expect("end_line must be a number");

    // Full mode always starts at 1 and includes all lines (20 lines in our fixture)
    assert_eq!(start_line, 1, "full mode: start_line must be 1; got: {seg}");
    assert!(
        end_line >= 10,
        "full mode: end_line must cover most/all of the 20-line file; got end_line={end_line}"
    );
}

/// [REQ-07] `context get` on a missing task must return an error JSON, not panic.
#[test]
fn test_context_get_missing_task_returns_error() {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let workspace_dir = tmp.path();
    let session_dir = workspace_dir;
    let workspace_str = workspace_dir.to_string_lossy();

    init_config(workspace_dir);
    write_plan_with_context(session_dir, workspace_dir, false);

    let (stdout, _stderr, _success) =
        run_cli(&["context", "get", &workspace_str, "T-NONEXISTENT"]);
    let v = parse_json(&stdout);
    assert!(
        v.get("error").is_some(),
        "missing task must return an error; got: {v}"
    );
}
