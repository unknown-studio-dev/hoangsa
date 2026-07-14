//! E2E integration tests for PDG taint analysis.
//!
//! Drives the real `hoangsa-memory` binary: init a tempdir root, write fixture
//! source files, run `index --pdg`, then `graph taint` and assert on parsed JSON.
//!
//! Harness mirrors `integration_graph.rs` exactly.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers (mirror of integration_graph.rs)
// ---------------------------------------------------------------------------

fn bin() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_hoangsa-memory"))
}

/// Run `hoangsa-memory --root <root> <args...>` and return (stdout, stderr, success).
fn run(root: &Path, args: &[&str]) -> (String, String, bool) {
    let output = Command::new(bin())
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .expect("failed to run hoangsa-memory binary");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Parse stdout as JSON, panicking with a descriptive message on failure.
fn parse_json(stdout: &str, context: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("{context}: stdout is not valid JSON (err={e}): {stdout:?}"))
}

// ---------------------------------------------------------------------------
// Test 1: intraprocedural — Rust env::var -> Command::new
// ---------------------------------------------------------------------------

#[test]
fn e2e_taint_rust_env_to_command() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let root = root_dir.path();
    let src = src_dir.path();

    std::fs::write(
        src.join("handler.rs"),
        "pub fn handler() {\n    let cmd = std::env::var(\"CMD\").unwrap();\n    let _ = std::process::Command::new(cmd).spawn();\n}\n",
    )
    .expect("write handler.rs");

    let (_, stderr, ok) = run(root, &["init"]);
    assert!(ok, "init failed; stderr: {stderr}");

    let (stdout, stderr, ok) = run(root, &["index", src.to_str().expect("src utf8"), "--pdg"]);
    assert!(ok, "index --pdg failed; stdout: {stdout}; stderr: {stderr}");

    let (stdout, stderr, ok) =
        run(root, &["--json", "graph", "taint", "--source", "env::var", "--sink", "command::new"]);
    assert!(ok, "graph taint must exit 0; stderr: {stderr}");

    let data = parse_json(&stdout, "e2e_taint_rust_env_to_command");
    let findings = data["findings"].as_array().expect("findings is array");
    assert!(
        findings.len() >= 1,
        "expected >=1 finding for env::var->command::new; got: {data}"
    );
    let path = findings[0]["path"].as_array().expect("path is array");
    assert!(
        !path.is_empty(),
        "finding path must be non-empty; got: {data}"
    );
}

// ---------------------------------------------------------------------------
// Test 2: intraprocedural — Python input() -> subprocess
// ---------------------------------------------------------------------------

#[test]
fn e2e_taint_python_input_to_subprocess() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let root = root_dir.path();
    let src = src_dir.path();

    std::fs::write(
        src.join("handler.py"),
        "def handler():\n    user = input()\n    import subprocess\n    subprocess.run(user)\n",
    )
    .expect("write handler.py");

    let (_, stderr, ok) = run(root, &["init"]);
    assert!(ok, "init failed; stderr: {stderr}");

    let (stdout, stderr, ok) = run(root, &["index", src.to_str().expect("src utf8"), "--pdg"]);
    assert!(ok, "index --pdg failed; stdout: {stdout}; stderr: {stderr}");

    let (stdout, stderr, ok) =
        run(root, &["--json", "graph", "taint", "--source", "input", "--sink", "subprocess"]);
    assert!(ok, "graph taint must exit 0; stderr: {stderr}");

    let data = parse_json(&stdout, "e2e_taint_python_input_to_subprocess");
    let findings = data["findings"].as_array().expect("findings is array");
    assert!(
        findings.len() >= 1,
        "expected >=1 finding for input()->subprocess; got: {data}"
    );
    let path = findings[0]["path"].as_array().expect("path is array");
    assert!(
        !path.is_empty(),
        "finding path must be non-empty; got: {data}"
    );
}

// ---------------------------------------------------------------------------
// Test 3: interprocedural — Rust env::var -> db_query via call-arg bridge
// ---------------------------------------------------------------------------

#[test]
fn e2e_taint_interprocedural_bridge() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let root = root_dir.path();
    let src = src_dir.path();

    std::fs::write(
        src.join("inter.rs"),
        "pub fn entry() {\n    let user = std::env::var(\"Q\").unwrap();\n    db_query(user);\n}\npub fn db_query(sql: String) {\n    let _ = sql;\n}\n",
    )
    .expect("write inter.rs");

    let (_, stderr, ok) = run(root, &["init"]);
    assert!(ok, "init failed; stderr: {stderr}");

    let (stdout, stderr, ok) = run(root, &["index", src.to_str().expect("src utf8"), "--pdg"]);
    assert!(ok, "index --pdg failed; stdout: {stdout}; stderr: {stderr}");

    let (stdout, stderr, ok) =
        run(root, &["--json", "graph", "taint", "--source", "env::var", "--sink", "query"]);
    assert!(ok, "graph taint must exit 0; stderr: {stderr}");

    let data = parse_json(&stdout, "e2e_taint_interprocedural_bridge");
    let findings = data["findings"].as_array().expect("findings is array");

    // Find the finding whose sink fqn contains "db_query".
    let interproc = findings.iter().find(|f| {
        f["sink"]["fqn"].as_str().is_some_and(|s| s.contains("db_query"))
    });
    assert!(
        interproc.is_some(),
        "expected a finding with sink fqn containing 'db_query'; got: {data}"
    );
    let path = interproc.unwrap()["path"].as_array().expect("path is array");
    assert!(
        path.len() >= 2,
        "interprocedural finding must have path len >= 2 (bridge hop); got path: {path:?}"
    );
}

// ---------------------------------------------------------------------------
// Test 4: negative — isolated variable has no data path to sink
// ---------------------------------------------------------------------------

#[test]
fn e2e_taint_negative_no_false_flow() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let root = root_dir.path();
    let src = src_dir.path();

    // Same as fixture 1 but with an extra isolated variable that never flows
    // to the spawn sink.
    std::fs::write(
        src.join("handler.rs"),
        "pub fn handler() {\n    let cmd = std::env::var(\"CMD\").unwrap();\n    let _ = std::process::Command::new(cmd).spawn();\n    let note = \"hello\".to_string();\n    let _ = note.len();\n}\n",
    )
    .expect("write handler.rs");

    let (_, stderr, ok) = run(root, &["init"]);
    assert!(ok, "init failed; stderr: {stderr}");

    let (stdout, stderr, ok) = run(root, &["index", src.to_str().expect("src utf8"), "--pdg"]);
    assert!(ok, "index --pdg failed; stdout: {stdout}; stderr: {stderr}");

    // "note" has no data path to "spawn" — the isolated string never reaches it.
    let (stdout, stderr, ok) =
        run(root, &["--json", "graph", "taint", "--source", "note", "--sink", "spawn"]);
    assert!(ok, "graph taint must exit 0; stderr: {stderr}");

    let data = parse_json(&stdout, "e2e_taint_negative_no_false_flow");
    let findings = data["findings"].as_array().expect("findings is array");
    assert!(
        findings.is_empty(),
        "isolated 'note' variable must not reach spawn sink; got: {data}"
    );
}

// ---------------------------------------------------------------------------
// Test 5: default index (no --pdg) produces zero stmt nodes
// ---------------------------------------------------------------------------

#[test]
fn e2e_default_index_no_stmt_nodes() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let root = root_dir.path();
    let src = src_dir.path();

    std::fs::write(
        src.join("handler.rs"),
        "pub fn handler() {\n    let cmd = std::env::var(\"CMD\").unwrap();\n    let _ = std::process::Command::new(cmd).spawn();\n}\n",
    )
    .expect("write handler.rs");

    let (_, stderr, ok) = run(root, &["init"]);
    assert!(ok, "init failed; stderr: {stderr}");

    // Index WITHOUT --pdg: no stmt nodes written.
    let (stdout, stderr, ok) = run(root, &["index", src.to_str().expect("src utf8")]);
    assert!(ok, "index (no --pdg) failed; stdout: {stdout}; stderr: {stderr}");

    let (stdout, stderr, ok) =
        run(root, &["--json", "graph", "taint", "--source", "env::var", "--sink", "command::new"]);
    assert!(ok, "graph taint must exit 0; stderr: {stderr}");

    let data = parse_json(&stdout, "e2e_default_index_no_stmt_nodes");
    let findings = data["findings"].as_array().expect("findings is array");
    let source_matches = data["source_matches"].as_u64().expect("source_matches is number");
    assert!(
        findings.is_empty(),
        "no stmt nodes without --pdg → zero findings; got: {data}"
    );
    assert_eq!(
        source_matches, 0,
        "no stmt nodes without --pdg → source_matches must be 0; got: {data}"
    );
}

// ---------------------------------------------------------------------------
// Test 6: unknown pattern → empty report, exit 0
// ---------------------------------------------------------------------------

#[test]
fn e2e_taint_unknown_pattern_empty() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let root = root_dir.path();
    let src = src_dir.path();

    std::fs::write(
        src.join("handler.rs"),
        "pub fn handler() {\n    let cmd = std::env::var(\"CMD\").unwrap();\n    let _ = std::process::Command::new(cmd).spawn();\n}\n",
    )
    .expect("write handler.rs");

    let (_, stderr, ok) = run(root, &["init"]);
    assert!(ok, "init failed; stderr: {stderr}");

    let (stdout, stderr, ok) = run(root, &["index", src.to_str().expect("src utf8"), "--pdg"]);
    assert!(ok, "index --pdg failed; stdout: {stdout}; stderr: {stderr}");

    let (stdout, stderr, ok) = run(
        root,
        &["--json", "graph", "taint", "--source", "zzz_nothing_matches", "--sink", "also_nothing"],
    );
    assert!(ok, "graph taint with unknown patterns must exit 0; stderr: {stderr}");

    let data = parse_json(&stdout, "e2e_taint_unknown_pattern_empty");
    let findings = data["findings"].as_array().expect("findings is array");
    let source_matches = data["source_matches"].as_u64().expect("source_matches is number");
    assert!(
        findings.is_empty(),
        "unknown patterns must yield empty findings; got: {data}"
    );
    assert_eq!(
        source_matches, 0,
        "unknown pattern must yield source_matches=0; got: {data}"
    );
}
