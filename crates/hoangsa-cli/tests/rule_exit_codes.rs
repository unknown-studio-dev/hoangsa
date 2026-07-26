//! E2E — the `rule` subcommands must exit non-zero when the underlying command
//! fails, and must keep their success output intact (compared as JSON
//! documents; see `assert_json_eq` for why byte-for-byte is not available).
//!
//! Every case drives the real `hoangsa-cli` binary and asserts on the process
//! exit code plus stdout/stderr, so a regression that swallows an error back
//! into exit 0 (`let _ = …`) fails here.

use std::process::Command;
use tempfile::TempDir;

/// A rule that round-trips through `Rule`'s serde contract:
/// `enforcement` ∈ {hook, preflight, prompt}, `action` ∈ {block, warn}.
const VALID_RULE: &str = r#"{"id":"r1","name":"R1","enabled":true,"enforcement":"prompt","matcher":"*","conditions":[],"action":"warn","message":"m"}"#;

struct Run {
    code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_cli(args: &[&str]) -> Run {
    let output = Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
        .args(args)
        .output()
        .expect("failed to run hoangsa-cli");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn assert_failed(run: &Run, label: &str, expected_stderr_fragment: &str) {
    assert_eq!(
        run.code,
        Some(1),
        "{label} must exit 1\n  stdout: {}\n  stderr: {}",
        run.stdout,
        run.stderr
    );
    assert!(
        run.stderr.contains(expected_stderr_fragment),
        "{label} stderr must mention {expected_stderr_fragment:?}; got: {:?}",
        run.stderr
    );
}

#[test]
fn cli_rule_subcommands_exit_nonzero_on_failure() {
    let tmp = TempDir::new().expect("create temp project dir");
    let dir = tmp.path().to_str().expect("temp dir path is utf-8");

    // `rule list` against a directory that does not exist.
    let listed = run_cli(&["rule", "list", "/nonexistent-dir-xyz"]);
    assert_eq!(
        listed.code,
        Some(1),
        "rule list <nonexistent dir> must exit 1\n  stdout: {}\n  stderr: {}",
        listed.stdout,
        listed.stderr
    );
    assert!(
        !listed.stderr.trim().is_empty(),
        "rule list <nonexistent dir> must write a diagnostic to stderr; got: {:?}",
        listed.stderr
    );

    // `rule add` with a body that is not JSON at all — the parse failure must
    // reach the caller. serde_json reports every parse error with a
    // `line N column M` suffix.
    let not_json = run_cli(&["rule", "add", dir, "not-valid-json"]);
    assert_failed(&not_json, "rule add <invalid JSON>", "line 1 column");

    // EC-13 — `rule add` with a body missing the required `id` field.
    let no_id = run_cli(&["rule", "add", dir, "{}"]);
    assert_failed(&no_id, "rule add '{}'", "missing field `id`");

    // EC-14 — the first add succeeds, the second one collides on `id`.
    let first = run_cli(&["rule", "add", dir, VALID_RULE]);
    assert_eq!(
        first.code,
        Some(0),
        "first rule add must exit 0\n  stdout: {}\n  stderr: {}",
        first.stdout,
        first.stderr
    );
    let duplicate = run_cli(&["rule", "add", dir, VALID_RULE]);
    assert_failed(&duplicate, "rule add <duplicate id>", "already exists");

    // rules.json now exists, so these three hit the "unknown id" path rather
    // than the "no rules file" one.
    let removed = run_cli(&["rule", "remove", dir, "unknown-id"]);
    assert_failed(&removed, "rule remove <unknown id>", "not found");

    let enabled = run_cli(&["rule", "enable", dir, "unknown-id"]);
    assert_failed(&enabled, "rule enable <unknown id>", "not found");

    let disabled = run_cli(&["rule", "disable", dir, "unknown-id"]);
    assert_failed(&disabled, "rule disable <unknown id>", "not found");
}

#[test]
fn cli_rule_success_paths_unchanged() {
    let tmp = TempDir::new().expect("create temp project dir");
    let dir = tmp.path().to_str().expect("temp dir path is utf-8");

    // `rule list` on a project with no rules file yet — an empty rule set is a
    // success, not a failure.
    let empty = run_cli(&["rule", "list", dir]);
    assert_eq!(empty.code, Some(0), "rule list <empty project> must exit 0");
    assert_eq!(
        empty.stderr, "",
        "rule list <empty project> must not write to stderr"
    );
    assert_json_eq(
        &empty.stdout,
        serde_json::json!({"count": 0, "disabled": 0, "enabled": 0, "rules": []}),
        "rule list <empty project> stdout changed",
    );

    let added = run_cli(&["rule", "add", dir, VALID_RULE]);
    assert_eq!(added.code, Some(0), "rule add <valid rule> must exit 0");
    assert_eq!(
        added.stderr, "",
        "rule add <valid rule> must not write to stderr"
    );
    assert_json_eq(
        &added.stdout,
        serde_json::json!({"id": "r1", "rules_count": 1, "success": true}),
        "rule add <valid rule> stdout changed",
    );

    let listed = run_cli(&["rule", "list", dir]);
    assert_eq!(listed.code, Some(0), "rule list <valid dir> must exit 0");
    assert_eq!(
        listed.stderr, "",
        "rule list <valid dir> must not write to stderr"
    );
    assert_json_eq(
        &listed.stdout,
        serde_json::json!({
            "count": 1,
            "disabled": 0,
            "enabled": 1,
            "rules": [{
                "action": "warn",
                "conditions": [],
                "enabled": true,
                "enforcement": "prompt",
                "id": "r1",
                "matcher": "*",
                "message": "m",
                "name": "R1"
            }]
        }),
        "rule list <valid dir> stdout changed",
    );
}

/// Compare stdout as a JSON document rather than as bytes.
///
/// Key order is not ours to pin: `hoangsa-proxy` enables serde_json's
/// `preserve_order`, and cargo unifies features across a workspace build, so
/// the very same binary emits insertion-ordered keys under
/// `cargo test --workspace` and alphabetical ones under
/// `cargo test -p hoangsa-cli`. Asserting on the byte string made this suite
/// pass alone and fail in CI. Every field is still pinned — only the ordering
/// the serializer chose is not.
fn assert_json_eq(stdout: &str, expected: serde_json::Value, msg: &str) {
    let actual: serde_json::Value = serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("{msg}: stdout is not valid JSON ({e}): {stdout:?}"));
    assert_eq!(actual, expected, "{msg}");
}
