//! E2E — the `rule` subcommands must exit non-zero when the underlying command
//! fails, and must keep their success output byte-for-byte.
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
    assert_eq!(empty.stderr, "", "rule list <empty project> must not write to stderr");
    assert_eq!(
        empty.stdout,
        "{\n  \"count\": 0,\n  \"disabled\": 0,\n  \"enabled\": 0,\n  \"rules\": []\n}\n",
        "rule list <empty project> stdout changed"
    );

    let added = run_cli(&["rule", "add", dir, VALID_RULE]);
    assert_eq!(added.code, Some(0), "rule add <valid rule> must exit 0");
    assert_eq!(added.stderr, "", "rule add <valid rule> must not write to stderr");
    assert_eq!(
        added.stdout,
        "{\n  \"id\": \"r1\",\n  \"rules_count\": 1,\n  \"success\": true\n}\n",
        "rule add <valid rule> stdout changed"
    );

    let listed = run_cli(&["rule", "list", dir]);
    assert_eq!(listed.code, Some(0), "rule list <valid dir> must exit 0");
    assert_eq!(listed.stderr, "", "rule list <valid dir> must not write to stderr");
    assert_eq!(
        listed.stdout,
        concat!(
            "{\n",
            "  \"count\": 1,\n",
            "  \"disabled\": 0,\n",
            "  \"enabled\": 1,\n",
            "  \"rules\": [\n",
            "    {\n",
            "      \"action\": \"warn\",\n",
            "      \"conditions\": [],\n",
            "      \"enabled\": true,\n",
            "      \"enforcement\": \"prompt\",\n",
            "      \"id\": \"r1\",\n",
            "      \"matcher\": \"*\",\n",
            "      \"message\": \"m\",\n",
            "      \"name\": \"R1\"\n",
            "    }\n",
            "  ]\n",
            "}\n",
        ),
        "rule list <valid dir> stdout changed"
    );
}
