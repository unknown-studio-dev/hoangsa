use crate::helpers::out;
use serde_json::json;
use std::fs;

use super::{apply_patch_file_paths, enforcement_events_path, is_source_file};

// ── Unified Enforcement Hook ─────────────────────────────────────────────────

/// `hook enforce`
///
/// Single PreToolUse entry point for ALL enforcement:
/// 1. Pattern-based rules from rules.json (same as rule-gate)
/// 2. Stateful rule: require memory_impact before Edit (first-touch files only)
/// 3. Stateful rule: require detect_changes before git commit
///
/// Critical (block) rules fail-CLOSED. Quality (warn) rules fail-OPEN.
pub fn cmd_enforce(cwd: &str) {
    use crate::cmd::rule::{
        evaluate_rule_conditions, read_effective_rules_config, Enforcement, RuleAction,
    };
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let parsed: serde_json::Value = serde_json::from_str(&input).unwrap_or(json!({}));
    let tool_name = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_input = parsed.get("tool_input").cloned().unwrap_or(json!({}));

    // ── Layer 1: Pattern-based rules (global ~/.hoangsa/rules.json overlaid by
    // project .hoangsa/rules.json; project overrides global by id) ──
    // A missing OR malformed file at either layer contributes no rules — the
    // loop below is then a no-op and control still flows to the Layer 2
    // stateful checks. Degrading each layer independently means a corrupt
    // global file can never silently disable a valid project BLOCK rule.
    let config = read_effective_rules_config(cwd);

    let mut warnings: Vec<String> = Vec::new();

    for rule in &config.rules {
        if !rule.enabled {
            continue;
        }
        // Skip rules that aren't hook-enforced or prompt-enforced
        // (preflight rules are checked elsewhere by CLI)
        if rule.enforcement == Enforcement::Preflight {
            continue;
        }
        // Stateful rules are dispatched by stateful_check below, not pattern-matched.
        if rule.stateful.is_some() {
            continue;
        }

        let matcher_matches = rule.matcher.split('|').any(|m| m.trim() == tool_name);
        if !matcher_matches {
            continue;
        }

        if !evaluate_rule_conditions(rule, &tool_input) {
            continue;
        }

        match rule.action {
            RuleAction::Block => {
                let reason = format!(
                    "⛔ RULE VIOLATION: {}\n\nRule: {}\nAction: BLOCK\n\n{}",
                    rule.id, rule.name, rule.message
                );
                out(&json!({"decision": "block", "reason": reason}));
                return;
            }
            RuleAction::Warn => {
                warnings.push(format!("⚠️ {}: {}", rule.id, rule.message));
            }
        }
    }

    // ── Layer 2: Stateful checks (require impact/detect_changes) ──
    if let Some(result) = stateful_check(cwd, tool_name, &tool_input, &config) {
        match result.decision.as_str() {
            "block" => {
                // Append any pattern warnings to the reason
                let mut reason = result.reason;
                if !warnings.is_empty() {
                    reason = format!("{}\n\n---\nAdditional warnings:\n{}", reason, warnings.join("\n"));
                }
                out(&json!({"decision": "block", "reason": reason}));
                return;
            }
            _ => {
                // Stateful check passed but may have added warnings
                if let Some(w) = result.warning {
                    warnings.push(w);
                }
            }
        }
    }

    // ── Output final decision ──
    if warnings.is_empty() {
        out(&json!({"decision": "approve"}));
    } else {
        let reason = warnings.join("\n\n");
        out(&json!({"decision": "approve", "reason": reason}));
    }
}

struct EnforceResult {
    decision: String,
    reason: String,
    warning: Option<String>,
}

/// Stateful enforcement checks based on event log.
/// Returns None if no stateful rule applies to this tool call.
/// `config` is the effective rule set `cmd_enforce` already loaded —
/// this hook runs on every tool call, so re-reading rules.json here
/// would double the per-call file I/O.
fn stateful_check(
    cwd: &str,
    tool_name: &str,
    tool_input: &serde_json::Value,
    config: &crate::cmd::rule::RulesConfig,
) -> Option<EnforceResult> {
    match tool_name {
        "Edit" | "Write" => {
            if stateful_rule_enabled(config, "require-memory-impact") {
                stateful_check_edit(cwd, tool_input)
            } else {
                None
            }
        }
        // Codex serializes file edits as `apply_patch` (patch text in
        // tool_input.command) instead of Edit/Write with a file_path.
        "apply_patch" => {
            if stateful_rule_enabled(config, "require-memory-impact") {
                stateful_check_apply_patch(cwd, tool_input)
            } else {
                None
            }
        }
        "Bash" => {
            if stateful_rule_enabled(config, "require-detect-changes")
                && let Some(r) = stateful_check_bash(cwd, tool_input)
            {
                return Some(r);
            }
            if stateful_rule_enabled(config, "no-git-add-ignored")
                && let Some(r) = check_gitignore_add(cwd, tool_input)
            {
                return Some(r);
            }
            None
        }
        _ => None,
    }
}

/// Look up a stateful rule by its `stateful` field value in the effective
/// (global-overlaid-by-project) rule set.
///
/// Returns `false` unless the stateful rule is explicitly present AND enabled.
/// A project with no rules.json — or a rules.json that simply doesn't list this
/// stateful id — is treated as opted out: hoangsa never blocks or warns from a
/// stateful rule the user hasn't explicitly enabled. This is the deliberate
/// "nothing configured → nothing applied implicitly" contract; the old
/// back-compat default of enabling unlisted stateful rules is gone.
fn stateful_rule_enabled(config: &crate::cmd::rule::RulesConfig, stateful_id: &str) -> bool {
    for rule in &config.rules {
        if rule.stateful.as_deref() == Some(stateful_id) {
            return rule.enabled;
        }
    }
    false
}

/// Rule #9: Require memory_impact for first-touch files before Edit.
/// Softened: if the file already has events (prior impact or edit), skip.
/// Thin wrapper — does I/O, delegates correlation to `intent_guard_edit`.
fn stateful_check_edit(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let file_path = tool_input.get("file_path").and_then(|v| v.as_str())?;
    if !is_source_file(file_path) {
        return None;
    }
    let events = fs::read_to_string(enforcement_events_path(cwd)).unwrap_or_default();
    match intent_guard_edit(&events, file_path) {
        IntentOutcome::Approve => None,
        IntentOutcome::Block(reason) => Some(EnforceResult { decision: "block".to_string(), reason, warning: None }),
        IntentOutcome::Warn(w) => Some(EnforceResult { decision: "approve".to_string(), reason: String::new(), warning: Some(w) }),
    }
}

/// Rule #9 for Codex: apply the first-touch impact guard to every source
/// file an `apply_patch` envelope touches. Any uncovered file blocks;
/// Warn outcomes accumulate into a single warning.
/// Thin wrapper — does I/O, delegates correlation to `apply_patch_intent_outcome`.
fn stateful_check_apply_patch(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let patch = tool_input.get("command").and_then(|v| v.as_str())?;
    let events = fs::read_to_string(enforcement_events_path(cwd)).unwrap_or_default();
    match apply_patch_intent_outcome(&events, patch)? {
        IntentOutcome::Approve => None,
        IntentOutcome::Block(reason) => Some(EnforceResult { decision: "block".to_string(), reason, warning: None }),
        IntentOutcome::Warn(w) => Some(EnforceResult { decision: "approve".to_string(), reason: String::new(), warning: Some(w) }),
    }
}

/// Pure: run `intent_guard_edit` over every source file in an apply_patch
/// envelope. `None` when no source file needs a verdict; first Block wins;
/// Warns join into one.
pub(super) fn apply_patch_intent_outcome(events: &str, patch: &str) -> Option<IntentOutcome> {
    let mut warnings: Vec<String> = Vec::new();
    for file in apply_patch_file_paths(patch) {
        if !is_source_file(&file) {
            continue;
        }
        match intent_guard_edit(events, &file) {
            IntentOutcome::Approve => {}
            block @ IntentOutcome::Block(_) => return Some(block),
            IntentOutcome::Warn(w) => warnings.push(w),
        }
    }
    if warnings.is_empty() {
        None
    } else {
        Some(IntentOutcome::Warn(warnings.join("\n")))
    }
}

/// Rule #10: Require detect_changes before git commit.
/// Thin wrapper — does I/O, delegates correlation to `intent_guard_bash_commit`.
fn stateful_check_bash(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let command = tool_input.get("command").and_then(|v| v.as_str())?;
    if !is_git_commit(command) {
        return None;
    }
    let events = fs::read_to_string(enforcement_events_path(cwd)).unwrap_or_default();
    let diff_files = get_staged_files(cwd);
    match intent_guard_bash_commit(&events, &diff_files) {
        IntentOutcome::Approve => None,
        IntentOutcome::Block(reason) => Some(EnforceResult { decision: "block".to_string(), reason, warning: None }),
        IntentOutcome::Warn(w) => Some(EnforceResult { decision: "approve".to_string(), reason: String::new(), warning: Some(w) }),
    }
}

/// Rule `no-git-add-ignored`: Block `git add` of gitignored files early so
/// the agent gets a specific error instead of a cryptic "git commit failed"
/// after a silent `git add` exit 1.
///
/// Thin wrapper — does I/O (`git check-ignore`), delegates parsing to
/// `parse_git_add_files` and the block message to `gitignore_block_reason`.
fn check_gitignore_add(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let command = tool_input.get("command").and_then(|v| v.as_str())?;
    let files = parse_git_add_files(command)?;
    let reason = gitignore_block_reason(&files, |f| is_path_gitignored(cwd, f))?;
    Some(EnforceResult { decision: "block".to_string(), reason, warning: None })
}

/// Pure: extract non-flag file args from a `git add ...` command.
///
/// Returns `None` when the command should not be checked by this rule:
/// - Not a `git add` command (prefix mismatch).
/// - Has `-f` / `--force` (covered by `no-git-add-force`).
/// - Has `-A` / `--all` / `.` (covered by `warn-git-add-all`; enumerating
///   ignored files in the working tree is out of scope for v1).
/// - No file args.
///
/// Uses whitespace tokenisation — file paths with spaces (rare for source
/// files) fall through this rule and hit the original git-add failure.
pub(super) fn parse_git_add_files(command: &str) -> Option<Vec<String>> {
    let trimmed = command.trim_start();
    let rest = trimmed.strip_prefix("git")?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let rest = rest.trim_start().strip_prefix("add")?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let tokens: Vec<&str> = rest.split_whitespace().collect();

    let mut files: Vec<String> = Vec::new();
    for tok in tokens {
        if matches!(tok, "-f" | "--force" | "-A" | "--all" | ".") {
            return None;
        }
        if tok.starts_with('-') {
            continue;
        }
        files.push(tok.to_string());
    }
    if files.is_empty() {
        None
    } else {
        Some(files)
    }
}

/// Pure: given a list of file args and an `is_ignored` predicate, return the
/// block reason if any file is ignored, else `None`.
pub(super) fn gitignore_block_reason<F: Fn(&str) -> bool>(files: &[String], is_ignored: F) -> Option<String> {
    let ignored: Vec<&String> = files.iter().filter(|f| is_ignored(f)).collect();
    if ignored.is_empty() {
        return None;
    }
    let list = ignored.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    Some(format!(
        "⛔ RULE VIOLATION: no-git-add-ignored\n\nRule: Block git add of gitignored files\nAction: BLOCK\n\ngit add contains gitignored files: {list}. Remove them from the command or update .gitignore."
    ))
}

/// I/O: run `git check-ignore -q <path>` in `cwd`. Exit 0 = ignored.
/// Any error (git missing, outside repo, etc.) → `false` (graceful degrade).
fn is_path_gitignored(cwd: &str, path: &str) -> bool {
    use std::process::{Command, Stdio};
    Command::new("git")
        .args(["check-ignore", "-q", path])
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Outcome of a pure intent-guard check. Kept separate from `EnforceResult`
/// so the guard logic is trivially unit-testable without mocking I/O.
#[derive(Debug, Clone, PartialEq)]
pub enum IntentOutcome {
    Approve,
    Block(String),
    Warn(String),
}

/// Pure: is there a prior `impact` or `override` event covering `file_path`?
/// Tolerant of rel↔abs path differences (via `paths_refer_to_same_file`).
/// Takes the full events-log text as input so it's trivially unit-testable
/// — no filesystem reads, no `cwd` threading, no env dependency.
pub fn intent_guard_edit(events: &str, file_path: &str) -> IntentOutcome {
    let covered = events.lines().any(|line| {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return false,
        };
        match entry.get("event").and_then(|e| e.as_str()).unwrap_or("") {
            "impact" => entry
                .get("file")
                .and_then(|f| f.as_str())
                .map(|f| !f.is_empty() && paths_refer_to_same_file(f, file_path))
                .unwrap_or(false),
            "override" => {
                entry.get("rule").and_then(|r| r.as_str()) == Some("require-memory-impact")
                    && entry
                        .get("target")
                        .and_then(|t| t.as_str())
                        .map(|t| paths_refer_to_same_file(t, file_path))
                        .unwrap_or(false)
            }
            _ => false,
        }
    });

    if covered {
        IntentOutcome::Approve
    } else {
        IntentOutcome::Block(format!(
            "⛔ STATEFUL: require-memory-impact\n\n\
             No memory_impact found for '{path}'\n\
             Run memory_impact on this file before editing.\n\n\
             If this is a false positive, use:\n\
             hoangsa-cli enforce override --rule require-memory-impact --target {path} --reason \"...\"",
            path = file_path
        ))
    }
}

/// Pure: compare the most-recent `detect_changes` event's file list against
/// the actual staged-diff file list. Returns Block if no detect_changes at
/// all, Warn if scope drift, Approve if covered. An override event for
/// `require-detect-changes` short-circuits to Approve.
pub fn intent_guard_bash_commit(events: &str, staged_files: &[String]) -> IntentOutcome {
    let mut detected: Vec<String> = Vec::new();
    let mut has_override = false;
    for line in events.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match entry.get("event").and_then(|e| e.as_str()).unwrap_or("") {
            "detect_changes" => {
                if let Some(files) = entry.get("files").and_then(|f| f.as_array()) {
                    for f in files {
                        if let Some(s) = f.as_str() {
                            detected.push(s.to_string());
                        }
                    }
                }
            }
            "override" if entry.get("rule").and_then(|r| r.as_str()) == Some("require-detect-changes") => {
                has_override = true;
            }
            _ => {}
        }
    }

    if has_override {
        return IntentOutcome::Approve;
    }

    if detected.is_empty() {
        return IntentOutcome::Block(
            "⛔ STATEFUL: require-detect-changes\n\n\
             No memory_detect_changes found before commit.\n\
             Run memory_detect_changes to verify scope before committing.\n\n\
             If this is a false positive, use:\n\
             hoangsa-cli enforce override --rule require-detect-changes --target commit --reason \"...\""
                .to_string(),
        );
    }

    // No staged files → nothing to correlate against (e.g. amend, empty commit).
    if staged_files.is_empty() {
        return IntentOutcome::Approve;
    }

    let uncovered: Vec<&String> = staged_files
        .iter()
        .filter(|f| {
            !detected
                .iter()
                .any(|d| f.ends_with(d.as_str()) || d.ends_with(f.as_str()))
        })
        .collect();

    if uncovered.is_empty() {
        IntentOutcome::Approve
    } else {
        let list = uncovered
            .iter()
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        IntentOutcome::Warn(format!(
            "⚠️ INTENT GUARD: Files in staged diff not covered by detect_changes: [{list}]\n\
             Consider re-running memory_detect_changes before commit."
        ))
    }
}

fn get_staged_files(cwd: &str) -> Vec<String> {
    use std::process::Command;
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(cwd)
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn is_git_commit(command: &str) -> bool {
    let re = regex::Regex::new(r"git\s+commit").unwrap_or_else(|_| regex::Regex::new("$^").expect("infallible"));
    re.is_match(command)
}

/// Relaxed path equality: treat an absolute path and a repo-relative path as
/// referring to the same file when one ends with the other. Used to bridge
/// impact events (often relative) against Edit file_path (usually absolute).
fn paths_refer_to_same_file(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Normalize leading "./"
    let an = a.trim_start_matches("./");
    let bn = b.trim_start_matches("./");
    if an == bn {
        return true;
    }
    // Tolerate abs↔rel: one must end with the other preceded by a path separator
    // to avoid "foo/bar.rs" matching "other/foo/bar.rs" incorrectly — wait, that's
    // actually the intended match here (same basename+subpath), so a bare ends_with
    // is correct. Require at least one path separator to avoid "a.rs" matching
    // "banana.rs".
    let ends_match = |long: &str, short: &str| -> bool {
        short.contains('/') && long.ends_with(short) && {
            let boundary = long.len() - short.len();
            boundary == 0 || long.as_bytes().get(boundary - 1) == Some(&b'/')
        }
    };
    ends_match(an, bn) || ends_match(bn, an)
}
