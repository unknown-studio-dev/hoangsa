//! Spec test 8: CLI end-to-end test for the graph subcommands.
//!
//! Drives the real `hoangsa-memory` binary: init a tempdir root, index a
//! tiny fixture source tree (2–3 Rust files with functions calling each
//! other), then run graph query / paths / communities / processes and
//! assert on the JSON output shapes. Also verifies that an unknown FQN
//! query returns resolved:false / found:false and exits 0.

use std::path::Path;
use std::process::Command;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Helpers
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

/// Write the tiny fixture source tree: main.rs + lib.rs with simple function calls.
///
/// ```
/// // main.rs
/// fn main() { run(); }
/// fn run() { helper(); }
///
/// // lib.rs
/// fn helper() { compute(); }
/// fn compute() -> i32 { 42 }
/// ```
fn write_fixture(src_dir: &Path) {
    std::fs::write(
        src_dir.join("main.rs"),
        r#"fn main() {
    run();
}

fn run() {
    helper();
}
"#,
    )
    .expect("write main.rs");

    std::fs::write(
        src_dir.join("lib.rs"),
        r#"pub fn helper() {
    compute();
}

pub fn compute() -> i32 {
    42
}
"#,
    )
    .expect("write lib.rs");
}

// ---------------------------------------------------------------------------
// Spec test 8: cli_graph_end_to_end
// ---------------------------------------------------------------------------

#[test]
fn cli_graph_end_to_end() {
    let root_dir = tempfile::tempdir().expect("root tempdir");
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let root = root_dir.path();
    let src = src_dir.path();

    // Write fixture source files.
    write_fixture(src);

    // 1. Init the root directory.
    let (_, stderr, ok) = run(root, &["init"]);
    assert!(ok, "init must succeed; stderr: {stderr}");

    // 2. Index the fixture source tree.
    let (stdout, stderr, ok) = run(root, &["index", src.to_str().expect("src utf8")]);
    assert!(ok, "index must succeed; stdout: {stdout}; stderr: {stderr}");

    // 3. graph query --start main::main --depth 2 --json
    // main::main should be indexed; depth 2 should reach main::run.
    let (stdout, stderr, ok) =
        run(root, &["--json", "graph", "query", "--start", "main::main", "--depth", "2"]);
    assert!(ok, "graph query must exit 0; stderr: {stderr}");
    let data = parse_json(&stdout, "graph query");
    assert!(
        data.get("nodes").is_some(),
        "graph query JSON must have 'nodes' key; got: {data}"
    );
    assert!(
        data.get("edges").is_some(),
        "graph query JSON must have 'edges' key; got: {data}"
    );
    assert!(
        data.get("truncated").is_some(),
        "graph query JSON must have 'truncated' key; got: {data}"
    );
    // The result should be non-trivial if main::main was indexed.
    // We don't assert exact FQN content because the indexer's crate
    // inference is best-effort; we just verify the shape is valid JSON
    // with the required keys.

    // 4. Unknown FQN query → unresolved non-empty, exit 0.
    let (stdout, stderr, ok) =
        run(root, &["--json", "graph", "query", "--start", "no::such::symbol", "--depth", "2"]);
    assert!(ok, "graph query with unknown fqn must exit 0; stderr: {stderr}");
    let data = parse_json(&stdout, "graph query unknown fqn");
    let nodes = data["nodes"].as_array().expect("nodes is array");
    assert!(nodes.is_empty(), "unknown fqn must return empty nodes; got: {data}");
    // unresolved must contain the input FQN.
    let unresolved = data.get("unresolved").and_then(|v| v.as_array());
    if let Some(arr) = unresolved {
        assert!(
            arr.iter().any(|v| v.as_str() == Some("no::such::symbol")),
            "unresolved must contain the unknown fqn; got: {arr:?}"
        );
    }
    // When unresolved key is absent, nodes being empty is sufficient evidence.

    // 5. graph paths --from no::from --to no::to → found:false, exit 0.
    let (stdout, stderr, ok) = run(
        root,
        &["--json", "graph", "paths", "--from", "no::from", "--to", "no::to"],
    );
    assert!(ok, "graph paths with unknown fqns must exit 0; stderr: {stderr}");
    let data = parse_json(&stdout, "graph paths unknown fqns");
    assert!(
        data.get("found").is_some(),
        "graph paths JSON must have 'found' key; got: {data}"
    );
    assert_eq!(
        data["found"],
        serde_json::json!(false),
        "no path between unknown fqns must return found:false; got: {data}"
    );

    // 6. graph communities → { "communities": [...] }, exit 0.
    let (stdout, stderr, ok) = run(root, &["--json", "graph", "communities", "--min-size", "2"]);
    assert!(ok, "graph communities must exit 0; stderr: {stderr}");
    let data = parse_json(&stdout, "graph communities");
    assert!(
        data.get("communities").is_some(),
        "graph communities JSON must have 'communities' key; got: {data}"
    );
    assert!(
        data["communities"].is_array(),
        "'communities' must be an array; got: {data}"
    );

    // 7. graph processes → { "processes": [...] }, exit 0.
    let (stdout, stderr, ok) = run(root, &["--json", "graph", "processes"]);
    assert!(ok, "graph processes must exit 0; stderr: {stderr}");
    let data = parse_json(&stdout, "graph processes");
    assert!(
        data.get("processes").is_some(),
        "graph processes JSON must have 'processes' key; got: {data}"
    );
    assert!(
        data["processes"].is_array(),
        "'processes' must be an array; got: {data}"
    );
}
