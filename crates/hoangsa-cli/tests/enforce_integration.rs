//! End-to-end tests for `hoangsa-cli hook enforce`.
//!
//! `cmd_enforce` composes two layers that have only ever been unit-tested in
//! isolation: the pattern-rule loop over the effective (global-overlaid-by-
//! project) rule set, and the stateful checks. These tests drive the BUILT
//! BINARY — a temp project on disk, a crafted PreToolUse payload on stdin,
//! and assertions on the JSON that comes back on stdout.
//!
//! The global rules layer is redirected with `HOANGSA_HOME` (read by
//! `global_rules_path` in `cmd/rule.rs`) so no test ever reads or writes the
//! developer's real `~/.hoangsa`.

use serde_json::Value;
use std::fs;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};

// ─── Harness ──────────────────────────────────────────────────────────────────

/// A temp project plus an isolated hoangsa home, so both rule layers are
/// under test control.
struct Fixture {
    project: PathBuf,
    home: PathBuf,
    // Owns the temp tree; dropping it removes both directories.
    _tmp: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().expect("create tempdir");
    let project = tmp.path().join("project");
    let home = tmp.path().join("hoangsa-home");
    fs::create_dir_all(project.join(".hoangsa").join("state")).expect("create .hoangsa/state");
    // `find_project_root` keys on this marker, so `--cwd`-less resolution
    // lands on the fixture rather than walking up to a real project.
    fs::write(project.join(".hoangsa").join("config.json"), "{}").expect("write config.json");
    fs::create_dir_all(&home).expect("create hoangsa home");
    Fixture { project, home, _tmp: tmp }
}

impl Fixture {
    fn write_project_rules(&self, json: &str) {
        fs::write(self.project.join(".hoangsa").join("rules.json"), json)
            .expect("write project rules.json");
    }

    fn write_global_rules(&self, json: &str) {
        fs::write(self.home.join("rules.json"), json).expect("write global rules.json");
    }

    /// Spawn `hoangsa-cli hook enforce` with cwd set to the fixture project,
    /// pipe `stdin_data` in, close stdin, and collect the result.
    /// Returns (stdout, stderr, exited_successfully).
    fn run_enforce(&self, stdin_data: &str) -> (String, String, bool) {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hoangsa-cli"))
            .args(["hook", "enforce"])
            .current_dir(&self.project)
            .env("HOANGSA_HOME", &self.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn hoangsa-cli");

        {
            let mut s = child.stdin.take().expect("stdin pipe");
            s.write_all(stdin_data.as_bytes()).expect("write stdin");
            // Dropped here — the hook blocks on read_to_string until EOF.
        }

        let output = child.wait_with_output().expect("wait for output");
        (
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
            output.status.success(),
        )
    }
}

fn parse_json(stdout: &str) -> Value {
    serde_json::from_str(stdout)
        .unwrap_or_else(|e| panic!("stdout must be valid JSON ({e}); got: {stdout}"))
}

fn decision(stdout: &str) -> String {
    parse_json(stdout)
        .get("decision")
        .and_then(|d| d.as_str())
        .unwrap_or_else(|| panic!("response must carry a `decision`; got: {stdout}"))
        .to_string()
}

fn reason(stdout: &str) -> String {
    parse_json(stdout)
        .get("reason")
        .and_then(|r| r.as_str())
        .unwrap_or_else(|| panic!("response must carry a `reason`; got: {stdout}"))
        .to_string()
}

// ─── Rule fixtures ────────────────────────────────────────────────────────────

/// A WARN rule that fires on any Bash command containing `git`, followed by a
/// BLOCK rule that fires only on `stash`. Ordering matters: the warn is
/// accumulated first, so a later block proves the short-circuit.
const WARN_THEN_BLOCK: &str = r#"{
  "version": "1.0",
  "rules": [
    {
      "id": "warn-any-git",
      "name": "Warn on git",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "contains", "value": "git" }],
      "action": "warn",
      "message": "WARN_TEXT_prefer_specific_files"
    },
    {
      "id": "block-git-stash",
      "name": "Block git stash",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "contains", "value": "stash" }],
      "action": "block",
      "message": "BLOCK_TEXT_never_stash"
    }
  ]
}"#;

// ─── EC-02: malformed input to the gate → fail closed ─────────────────────────

#[test]
fn enforce_malformed_stdin_blocks() {
    let fx = fixture();
    // Valid rules present: nothing here should approve the call either way —
    // the point is that an unreadable payload is denied before rules matter.
    fx.write_project_rules(WARN_THEN_BLOCK);

    let (stdout, stderr, ok) = fx.run_enforce("not json");

    assert!(ok, "enforce must exit 0 even when it blocks; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "block",
        "unparseable stdin must fail CLOSED; got: {stdout}"
    );
    let reason = reason(&stdout);
    assert!(
        reason.contains("could not parse the hook payload"),
        "reason must name the unparseable payload; got: {reason}"
    );
}

// ─── A BLOCK returns immediately, dropping accumulated warnings ───────────────

#[test]
fn enforce_block_short_circuits_warn() {
    let fx = fixture();
    fx.write_project_rules(WARN_THEN_BLOCK);

    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git stash push -m wip"}}"#;
    let (stdout, stderr, ok) = fx.run_enforce(payload);

    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(decision(&stdout), "block", "got: {stdout}");
    let reason = reason(&stdout);
    assert!(
        reason.contains("block-git-stash") && reason.contains("BLOCK_TEXT_never_stash"),
        "block reason must name the rule and carry its message; got: {reason}"
    );
    // The warn rule matched first and was accumulated, but `cmd_enforce`
    // returns on the block without ever emitting `warnings`.
    assert!(
        !stdout.contains("WARN_TEXT_prefer_specific_files"),
        "the earlier WARN rule's message must not survive the block; got: {stdout}"
    );
}

// ─── A WARN alone never blocks ────────────────────────────────────────────────

#[test]
fn enforce_warn_does_not_block() {
    let fx = fixture();
    fx.write_project_rules(WARN_THEN_BLOCK);

    // `git status` matches the warn rule only.
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git status --short"}}"#;
    let (stdout, stderr, ok) = fx.run_enforce(payload);

    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "approve",
        "a warn-only match must not block; got: {stdout}"
    );
    let reason = reason(&stdout);
    assert!(
        reason.contains("warn-any-git") && reason.contains("WARN_TEXT_prefer_specific_files"),
        "the warning text must reach the caller; got: {reason}"
    );
    assert!(
        !stdout.contains(r#""decision": "block""#),
        "output must not carry a block decision; got: {stdout}"
    );
}

// ─── Project layer overrides global layer by rule id ──────────────────────────

#[test]
fn enforce_project_rules_override_global() {
    let fx = fixture();
    // Global says BLOCK for id `shared-id` …
    fx.write_global_rules(
        r#"{
  "version": "1.0",
  "rules": [
    {
      "id": "shared-id",
      "name": "Global block on rm -rf",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "contains", "value": "rm -rf" }],
      "action": "block",
      "message": "GLOBAL_BLOCK_TEXT"
    }
  ]
}"#,
    );
    // … the project downgrades the SAME id to a warn.
    fx.write_project_rules(
        r#"{
  "version": "1.0",
  "rules": [
    {
      "id": "shared-id",
      "name": "Project downgrade to warn",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "contains", "value": "rm -rf" }],
      "action": "warn",
      "message": "PROJECT_WARN_TEXT"
    }
  ]
}"#,
    );

    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"rm -rf target/debug"}}"#;
    let (stdout, stderr, ok) = fx.run_enforce(payload);

    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "approve",
        "the project override wins by id, so the call must not be blocked; got: {stdout}"
    );
    assert!(
        reason(&stdout).contains("PROJECT_WARN_TEXT"),
        "the project version of the rule must be the one that fired; got: {stdout}"
    );
    assert!(
        !stdout.contains("GLOBAL_BLOCK_TEXT"),
        "the overridden global rule must not fire; got: {stdout}"
    );
}

// ─── Each layer degrades independently ────────────────────────────────────────

#[test]
fn enforce_malformed_rules_file_degrades_per_layer() {
    const GLOBAL_BLOCK: &str = r#"{
  "version": "1.0",
  "rules": [
    {
      "id": "global-block-stash",
      "name": "Global block on git stash",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "contains", "value": "stash" }],
      "action": "block",
      "message": "GLOBAL_BLOCK_FIRES"
    }
  ]
}"#;
    const PROJECT_BLOCK: &str = r#"{
  "version": "1.0",
  "rules": [
    {
      "id": "project-block-stash",
      "name": "Project block on git stash",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "contains", "value": "stash" }],
      "action": "block",
      "message": "PROJECT_BLOCK_FIRES"
    }
  ]
}"#;
    const CORRUPT: &str = r#"{ "version": "1.0", "rules": [ {"id": "truncated"#;

    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git stash"}}"#;

    // (a) Corrupt PROJECT layer — the valid global BLOCK rule still fires.
    let fx = fixture();
    fx.write_global_rules(GLOBAL_BLOCK);
    fx.write_project_rules(CORRUPT);
    let (stdout, stderr, ok) = fx.run_enforce(payload);
    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "block",
        "a corrupt project file must not disable a valid global rule; got: {stdout}"
    );
    assert!(
        reason(&stdout).contains("global-block-stash"),
        "the surviving global rule must be the one that fired; got: {stdout}"
    );

    // (b) Corrupt GLOBAL layer — the valid project BLOCK rule still fires.
    let fx = fixture();
    fx.write_global_rules(CORRUPT);
    fx.write_project_rules(PROJECT_BLOCK);
    let (stdout, stderr, ok) = fx.run_enforce(payload);
    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "block",
        "a corrupt global file must not disable a valid project rule; got: {stdout}"
    );
    assert!(
        reason(&stdout).contains("project-block-stash"),
        "the surviving project rule must be the one that fired; got: {stdout}"
    );

    // (c) The deliberate cost of that design, stated plainly: a corrupt layer
    // contributes ZERO rules, so the BLOCK rules it held stop being enforced
    // and the same call is approved. Enforcement degrades silently on stdout —
    // a corrupt rules file disables its own layer, it does not fail closed.
    let fx = fixture();
    fx.write_project_rules(CORRUPT);
    let (stdout, stderr, ok) = fx.run_enforce(payload);
    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "approve",
        "with every layer corrupt or absent there are no rules left to enforce; got: {stdout}"
    );
}

// ─── EC-06: empty rule set — Layer 1 is a no-op, Layer 2 still runs ───────────

#[test]
fn enforce_empty_rules_still_runs_layer2() {
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git stash"}}"#;

    // (a) The literal EC-06 input. Note `RulesConfig::version` has no serde
    // default, so `{"rules": []}` does not deserialize and reaches the empty
    // rule set via the lenient-degrade path rather than as a parsed-but-empty
    // config. Same observable contract either way: zero rules, no block.
    let fx = fixture();
    fx.write_project_rules(r#"{"rules": []}"#);
    let (stdout, stderr, ok) = fx.run_enforce(payload);
    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(decision(&stdout), "approve", "got: {stdout}");

    // (b) A well-formed, genuinely empty rule set: Layer 1 matches nothing and
    // the process still emits a well-formed JSON decision.
    let fx = fixture();
    fx.write_project_rules(r#"{"version": "1.0", "rules": []}"#);
    let (stdout, stderr, ok) = fx.run_enforce(payload);
    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(decision(&stdout), "approve", "got: {stdout}");
    assert!(
        parse_json(&stdout).get("reason").is_none(),
        "an approve with no warnings carries no reason; got: {stdout}"
    );

    // (c) Layer 2 is still consulted when Layer 1 has no pattern rules at all.
    // The only rule present is stateful, which the Layer 1 loop skips outright;
    // reaching a block therefore proves control flowed past Layer 1.
    let fx = fixture();
    fx.write_project_rules(
        r#"{
  "version": "1.0",
  "rules": [
    {
      "id": "require-memory-impact",
      "name": "Require memory_impact before first edit",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Edit|Write",
      "conditions": [],
      "action": "block",
      "message": "Run memory_impact on this file before editing.",
      "stateful": "require-memory-impact"
    }
  ]
}"#,
    );
    let edit_payload = r#"{"tool_name":"Edit","tool_input":{"file_path":"src/lib.rs"}}"#;
    let (stdout, stderr, ok) = fx.run_enforce(edit_payload);
    assert!(ok, "enforce must exit 0; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "block",
        "the stateful layer must still run with zero pattern rules; got: {stdout}"
    );
    assert!(
        reason(&stdout).contains("STATEFUL: require-memory-impact"),
        "the block must come from Layer 2; got: {stdout}"
    );
}

// ─── EC-07: an uncompilable regex disables only its own condition ─────────────

#[test]
fn enforce_uncompilable_regex_degrades_one_condition() {
    let fx = fixture();
    fx.write_project_rules(
        r#"{
  "version": "1.0",
  "rules": [
    {
      "id": "bad-regex-block",
      "name": "Block on an uncompilable pattern",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "regex", "value": "(" }],
      "action": "block",
      "message": "BAD_REGEX_BLOCK_TEXT"
    },
    {
      "id": "good-warn",
      "name": "Warn on git",
      "enabled": true,
      "enforcement": "hook",
      "matcher": "Bash",
      "conditions": [{ "field": "command", "op": "contains", "value": "git" }],
      "action": "warn",
      "message": "GOOD_WARN_TEXT"
    }
  ]
}"#,
    );

    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#;
    let (stdout, stderr, ok) = fx.run_enforce(payload);

    assert!(ok, "an uncompilable pattern must not crash the hook; stderr: {stderr}");
    assert_eq!(
        decision(&stdout),
        "approve",
        "a condition whose regex did not compile must never match; got: {stdout}"
    );
    assert!(
        !stdout.contains("BAD_REGEX_BLOCK_TEXT"),
        "the rule with the broken pattern must not fire; got: {stdout}"
    );
    assert!(
        reason(&stdout).contains("GOOD_WARN_TEXT"),
        "the other rule must still evaluate; got: {stdout}"
    );
    assert!(
        stderr.contains("bad-regex-block") && stderr.contains("invalid regex"),
        "the failure must be reported on stderr, naming the rule; got: {stderr}"
    );
}
