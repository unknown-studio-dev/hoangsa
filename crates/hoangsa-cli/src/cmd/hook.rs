use crate::helpers::{out, read_json};
use serde_json::json;
use std::fs;
use std::path::Path;

/// `hook stop-check [sessions_dir]`
///
/// Deterministic workflow-completion check for the Claude Code Stop hook.
/// Replaces the fragile prompt-type hook that couldn't distinguish
/// fix/research/audit sessions from menu sessions.
///
/// Logic:
///   - No session or no state.json → approve
///   - status="cooking" + plan.json has pending/running tasks → block
///   - Everything else → approve
pub fn cmd_stop_check(sessions_dir: Option<&str>, cwd: &str) {
    let dir = sessions_dir.map(|s| s.to_string()).unwrap_or_else(|| {
        Path::new(cwd)
            .join(".hoangsa")
            .join("sessions")
            .to_string_lossy()
            .to_string()
    });

    let latest = find_latest_session(&dir);
    let session_dir = match latest {
        Some(d) => d,
        None => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let state_path = Path::new(&session_dir).join("state.json");
    if !state_path.exists() {
        out(&json!({"decision": "approve"}));
        return;
    }

    let state = read_json(state_path.to_str().unwrap_or(""));
    if state.get("error").is_some() {
        // Can't read state → don't block
        out(&json!({"decision": "approve"}));
        return;
    }

    let status = state["status"].as_str().unwrap_or("");

    // Only block during active cooking with incomplete tasks
    if status == "cooking" {
        let plan_path = Path::new(&session_dir).join("plan.json");
        if plan_path.exists() {
            let plan = read_json(plan_path.to_str().unwrap_or(""));
            if plan.get("error").is_none() {
                let pending = count_incomplete_tasks(&plan);
                if pending > 0 {
                    out(&json!({
                        "decision": "approve",
                        "reason": format!(
                            "⚠️ HOANGSA: Workflow incomplete — {} task(s) still pending/running in session {}. You MUST complete all tasks before finishing. If you need user input, ask and then continue working.",
                            pending,
                            state["session_id"].as_str().unwrap_or("unknown")
                        )
                    }));
                    return;
                }
            }
        }
    }

    out(&json!({"decision": "approve"}));
}

/// `hook lesson-guard`
///
/// PreToolUse hook for Edit/Write. Reads stdin JSON, extracts file_path,
/// calls `thoth recall` to find relevant lessons/facts, surfaces them.
/// If a recalled lesson contains "NEVER" + a path fragment that matches
/// the file being edited → block. Otherwise → approve with context shown.
pub fn cmd_lesson_guard(cwd: &str) {
    use std::io::Read;
    use std::process::Command;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let parsed: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(json!({}));

    let file_path = parsed
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|fp| fp.as_str())
        .unwrap_or("");

    if file_path.is_empty() {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Build a query from the file path — extract meaningful path segments
    let query = build_recall_query(file_path);
    if query.is_empty() {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Find thoth binary
    let thoth_root = Path::new(cwd).join(".hoangsa-memory");
    if !thoth_root.exists() {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Call thoth CLI to recall lessons relevant to this file path
    let thoth_bin = find_thoth_bin();
    let thoth_bin = match thoth_bin {
        Some(b) => b,
        None => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let result = Command::new(&thoth_bin)
        .args(["--root", &thoth_root.to_string_lossy()])
        .args(["query", &query, "--top-k", "8", "--json"])
        .output();

    let output_bytes = match result {
        Ok(o) => o.stdout,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let recall: serde_json::Value = match serde_json::from_slice(&output_bytes) {
        Ok(v) => v,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let chunks = match recall.get("chunks").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    // Filter to only LESSONS.md and MEMORY.md chunks
    let lessons: Vec<&str> = chunks
        .iter()
        .filter(|c| {
            let path = c.get("path").and_then(|p| p.as_str()).unwrap_or("");
            path == "LESSONS.md" || path == "MEMORY.md"
        })
        .filter_map(|c| c.get("body").and_then(|b| b.as_str()))
        .collect();

    if lessons.is_empty() {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Check: does any lesson say "NEVER" + contain a path fragment matching file_path?
    let fp_lower = file_path.to_lowercase();
    let mut blocking_lesson: Option<&str> = None;

    for lesson in &lessons {
        let lesson_lower = lesson.to_lowercase();
        if !lesson_lower.contains("never") {
            continue;
        }
        // Find "NEVER" in the lesson, then extract path fragments from
        // the text between "NEVER" and the next "—" or sentence end.
        // This avoids matching paths in the "do this instead" advice part.
        if let Some(never_pos) = lesson_lower.find("never") {
            let after_never = &lesson[never_pos..];
            // Take text up to next "—" or "Always" or end
            let end_pos = after_never.find(" — ")
                .or_else(|| after_never.find(". Always"))
                .or_else(|| after_never.find(". The"))
                .unwrap_or(after_never.len());
            let never_clause = &after_never[..end_pos];

            for word in never_clause.split_whitespace() {
                let clean = word.trim_matches(|c: char| {
                    !c.is_alphanumeric() && c != '/' && c != '.' && c != '-' && c != '_'
                }).trim_matches('`');
                if clean.contains('/') && clean.len() > 2 && fp_lower.contains(&clean.to_lowercase()) {
                    blocking_lesson = Some(lesson);
                    break;
                }
            }
        }
        if blocking_lesson.is_some() {
            break;
        }
    }

    // Check if file is gitignored — adds context to the decision
    let is_gitignored = Command::new("git")
        .args(["check-ignore", "-q", file_path])
        .current_dir(cwd)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let all_lessons_text = lessons.join("\n---\n");

    if let Some(lesson) = blocking_lesson {
        // Hard-block when editing an installed-copy path that a NEVER lesson
        // warns about. Previously this only surfaced a warning and approved —
        // which let the agent override the lesson (happened 5+ times). The
        // block condition is deterministic: NEVER-lesson match + gitignored +
        // path sits under a known installed-copy prefix.
        let fp = file_path;
        let is_installed_copy_path = fp.contains("/.claude/hoangsa/")
            || fp.contains("/.claude/skills/")
            || fp.contains("/.claude/commands/")
            || fp.contains("/.claude/agents/");
        let should_block = is_gitignored && is_installed_copy_path;

        if should_block {
            out(&json!({
                "decision": "block",
                "reason": format!(
                    "BLOCKED: '{}' is a gitignored installed-copy path and matches a NEVER lesson.\n\nLesson:\n{}\n\nEdit the source under templates/ instead, then run bin/install to sync.\n\nIf this is intentional (rare), tell the user to override explicitly.",
                    file_path, lesson
                )
            }));
        } else {
            let gitignore_note = if is_gitignored {
                "\nNote: This file is in .gitignore — it may be an installed copy, not the source."
            } else {
                ""
            };
            out(&json!({
                "decision": "approve",
                "reason": format!(
                    "⚠️ LESSON GUARD for '{}':{}\n\nRelevant lesson:\n{}\n\n---\nAll recalled lessons:\n{}\n\nIf this edit is intentional, proceed. If not, find the correct source file.",
                    file_path, gitignore_note, lesson, all_lessons_text
                )
            }));
        }
    } else if !lessons.is_empty() {
        // No blocking lesson, but surface lessons as context
        out(&json!({
            "decision": "approve",
            "reason": format!(
                "Lessons for '{}':\n{}",
                file_path, all_lessons_text
            )
        }));
    } else {
        out(&json!({"decision": "approve"}));
    }
}

/// Build a recall query from a file path.
/// Keeps path structure intact so thoth can match lessons mentioning paths.
fn build_recall_query(path: &str) -> String {
    // Strip home dir prefix for cleaner query
    let clean = if let Ok(home) = std::env::var("HOME") {
        path.strip_prefix(&home).unwrap_or(path)
    } else {
        path
    };
    // Strip leading project dir — keep from first recognizable segment
    let clean = clean.trim_start_matches('/');
    // Keep path-like structure so ".claude/hoangsa" or "templates/" matches
    format!("NEVER edit {clean}")
}

/// Find a binary by searching PATH (cross-platform).
/// `stem` is the binary name without extension (e.g. "thoth").
fn find_bin_in_path(stem: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    let names: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ""]
    } else {
        &[""]
    };
    for dir in path_var.split(sep) {
        for suffix in names {
            let name = format!("{stem}{suffix}");
            let candidate = Path::new(dir).join(&name);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn find_thoth_bin() -> Option<String> {
    find_bin_in_path("hoangsa-memory")
}

/// Count tasks with status other than "completed", "done", "skipped".
fn count_incomplete_tasks(plan: &serde_json::Value) -> usize {
    let tasks = match plan["tasks"].as_array() {
        Some(t) => t,
        None => return 0,
    };

    tasks
        .iter()
        .filter(|t| {
            let s = t["status"].as_str().unwrap_or("pending");
            !matches!(s, "completed" | "done" | "skipped" | "failed")
        })
        .count()
}

// ── Unified Enforcement Hook ─────────────────────────────────────────────────

/// `hook enforce`
///
/// Single PreToolUse entry point for ALL enforcement:
/// 1. Pattern-based rules from rules.json (same as rule-gate)
/// 2. Stateful rule: require thoth_impact before Edit (first-touch files only)
/// 3. Stateful rule: require detect_changes before git commit
///
/// Critical (block) rules fail-CLOSED. Quality (warn) rules fail-OPEN.
pub fn cmd_enforce(cwd: &str) {
    use crate::cmd::rule::{
        evaluate_rule_conditions, read_rules_config_pub, Enforcement, RuleAction,
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

    // ── Layer 1: Pattern-based rules from rules.json ──
    let config = match read_rules_config_pub(cwd) {
        Ok(Some(c)) => c,
        Ok(None) => {
            // No rules.json — still run stateful checks
            if let Some(result) = stateful_check(cwd, tool_name, &tool_input) {
                print_decision(&result);
                return;
            }
            out(&json!({"decision": "approve"}));
            return;
        }
        Err(_) => {
            // Parse error — fail-OPEN for quality, but stateful checks still run
            if let Some(result) = stateful_check(cwd, tool_name, &tool_input) {
                print_decision(&result);
                return;
            }
            out(&json!({"decision": "approve"}));
            return;
        }
    };

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
    if let Some(result) = stateful_check(cwd, tool_name, &tool_input) {
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

fn print_decision(result: &EnforceResult) {
    if result.decision == "block" {
        out(&json!({"decision": "block", "reason": result.reason}));
    } else if let Some(w) = &result.warning {
        out(&json!({"decision": "approve", "reason": w}));
    } else {
        out(&json!({"decision": "approve"}));
    }
}

/// Stateful enforcement checks based on event log.
/// Returns None if no stateful rule applies to this tool call.
fn stateful_check(cwd: &str, tool_name: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    match tool_name {
        "Edit" | "Write" => {
            if stateful_rule_enabled(cwd, "require-thoth-impact") {
                stateful_check_edit(cwd, tool_input)
            } else {
                None
            }
        }
        "Bash" => {
            if stateful_rule_enabled(cwd, "require-detect-changes") {
                stateful_check_bash(cwd, tool_input)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Look up a stateful rule by its `stateful` field value; default-enabled if no
/// entry is present (backwards compatibility with installs predating the field).
fn stateful_rule_enabled(cwd: &str, stateful_id: &str) -> bool {
    use crate::cmd::rule::read_rules_config_pub;
    let config = match read_rules_config_pub(cwd) {
        Ok(Some(c)) => c,
        _ => return true,
    };
    for rule in &config.rules {
        if rule.stateful.as_deref() == Some(stateful_id) {
            return rule.enabled;
        }
    }
    true
}

/// Rule #9: Require thoth_impact for first-touch files before Edit.
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
                entry.get("rule").and_then(|r| r.as_str()) == Some("require-thoth-impact")
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
            "⛔ STATEFUL: require-thoth-impact\n\n\
             No thoth_impact found for '{path}'\n\
             Run thoth_impact on this file before editing.\n\n\
             If this is a false positive, use:\n\
             hoangsa-cli enforce override --rule require-thoth-impact --target {path} --reason \"...\"",
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
             No thoth_detect_changes found before commit.\n\
             Run thoth_detect_changes to verify scope before committing.\n\n\
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
             Consider re-running thoth_detect_changes before commit."
        ))
    }
}

/// `hook intent-guard`
///
/// Standalone PreToolUse-style subcommand that runs ONLY the correlation
/// checks (file-level for Edit/Write, diff-scope for Bash git-commit).
/// Does NOT read rules.json — it trusts the caller to have decided whether
/// the guard should fire. Useful for composing lint-style commit hooks or
/// writing unit tests that exercise correlation in isolation.
///
/// Decisions: approve | block | approve+reason (warn). Reads tool payload
/// from stdin in the same shape Claude Code's PreToolUse hooks receive.
pub fn cmd_intent_guard(cwd: &str) {
    use std::io::Read as _;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let parsed: serde_json::Value = serde_json::from_str(&input).unwrap_or(json!({}));
    let tool_name = parsed.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let tool_input = parsed.get("tool_input").cloned().unwrap_or(json!({}));
    let events = fs::read_to_string(enforcement_events_path(cwd)).unwrap_or_default();

    let outcome = match tool_name {
        "Edit" | "Write" => {
            match tool_input.get("file_path").and_then(|v| v.as_str()) {
                Some(fp) if is_source_file(fp) => intent_guard_edit(&events, fp),
                _ => IntentOutcome::Approve,
            }
        }
        "Bash" => {
            match tool_input.get("command").and_then(|v| v.as_str()) {
                Some(cmd) if is_git_commit(cmd) => {
                    let files = get_staged_files(cwd);
                    intent_guard_bash_commit(&events, &files)
                }
                _ => IntentOutcome::Approve,
            }
        }
        _ => IntentOutcome::Approve,
    };

    match outcome {
        IntentOutcome::Approve => out(&json!({"decision": "approve"})),
        IntentOutcome::Block(reason) => out(&json!({"decision": "block", "reason": reason})),
        IntentOutcome::Warn(reason) => out(&json!({"decision": "approve", "reason": reason})),
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

fn is_source_file(path: &str) -> bool {
    let source_extensions = [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java",
        ".c", ".cpp", ".h", ".hpp", ".rb", ".swift", ".kt",
    ];
    source_extensions.iter().any(|ext| path.ends_with(ext))
}

// ── PostToolUse State Recording ──────────────────────────────────────────────

/// `hook post-enforce`
///
/// PostToolUse hook that records enforcement events after thoth tool calls.
/// Records: impact (with file resolution), detect_changes (with files), recall (with query).
/// Always outputs `{"decision":"approve"}` — never blocks.
pub fn cmd_post_enforce(cwd: &str) {
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let parsed: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let tool_name = parsed.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let tool_input = parsed.get("tool_input").cloned().unwrap_or(json!({}));

    let event = match tool_name {
        "mcp__thoth__thoth_impact" => build_impact_event(cwd, &tool_input),
        "mcp__thoth__thoth_detect_changes" => build_detect_changes_event(&tool_input, &parsed),
        "mcp__thoth__thoth_recall" => build_recall_event(&tool_input),
        "Edit" | "Write" | "MultiEdit" => build_drift_event(cwd, &tool_input),
        _ => None,
    };

    if let Some(event) = event {
        append_event(cwd, &event);
    }

    out(&json!({"decision": "approve"}));
}

/// Rule #14 (experimental, v1): grep-based post-edit drift detection.
/// Extracts top-level symbol names from the diff (old_string vs new_string),
/// compares against impact-checked symbols for this file from the event log,
/// and emits a `drift_warn` event when the edited set isn't covered.
/// WARN-only — never blocks. False-positive rate tracked via `enforce report`.
fn build_drift_event(cwd: &str, tool_input: &serde_json::Value) -> Option<serde_json::Value> {
    let file_path = tool_input.get("file_path").and_then(|v| v.as_str())?;
    if !is_source_file(file_path) {
        return None;
    }
    let old_string = tool_input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
    let new_string = tool_input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
    let content = tool_input.get("content").and_then(|v| v.as_str()).unwrap_or("");

    // Collect symbols from the edit region (old ∪ new for Edit; content for Write).
    let mut edited: Vec<String> = Vec::new();
    for text in [old_string, new_string, content] {
        if text.is_empty() { continue; }
        extract_symbols(cwd, text, &mut edited);
    }
    edited.sort();
    edited.dedup();
    if edited.is_empty() {
        return None;
    }

    // Collect impact-checked symbols for this file from the event log.
    let events_path = enforcement_events_path(cwd);
    let events = fs::read_to_string(&events_path).unwrap_or_default();
    let mut impacted: Vec<String> = Vec::new();
    for line in events.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if entry.get("event").and_then(|e| e.as_str()) != Some("impact") {
            continue;
        }
        let matches_file = entry
            .get("file")
            .and_then(|f| f.as_str())
            .map(|f| f == file_path || file_path.ends_with(f) || f.ends_with(file_path))
            .unwrap_or(false);
        if !matches_file {
            continue;
        }
        if let Some(sym) = entry.get("symbol").and_then(|s| s.as_str()) {
            impacted.push(sym.to_string());
        }
    }

    // If no impact was recorded for this file, require-thoth-impact already
    // surfaced that gap at PreToolUse — don't double-warn here.
    if impacted.is_empty() {
        return None;
    }

    let uncovered: Vec<String> = edited
        .iter()
        .filter(|e| !impacted.iter().any(|i| i.contains(e.as_str()) || e.contains(i.as_str())))
        .cloned()
        .collect();

    if uncovered.is_empty() {
        None
    } else {
        Some(json!({
            "event": "drift_warn",
            "file": file_path,
            "edited_symbols": edited,
            "impact_symbols": impacted,
            "uncovered": uncovered,
        }))
    }
}

/// Default symbol-detection regexes, used when
/// `.hoangsa/config.json → enforcement.symbol_patterns` is absent.
/// Each pattern MUST have exactly one capture group for the symbol name.
/// Kept broad on purpose: false positives degrade only the drift-warn metric,
/// not correctness (drift is WARN-only per Decision #10 in the brainstorm).
const DEFAULT_SYMBOL_PATTERNS: &[&str] = &[
    r"(?m)\b(?:pub\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\b(?:pub\s+)?(?:struct|enum|trait|impl)\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bdef\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bfunction\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bfunc\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bclass\s+([A-Za-z_][A-Za-z0-9_]*)",
];

/// Read symbol-detection regexes from `.hoangsa/config.json` under
/// `enforcement.symbol_patterns` (array of regex strings). Falls back to
/// `DEFAULT_SYMBOL_PATTERNS` when absent or malformed.
fn read_symbol_patterns(cwd: &str) -> Vec<String> {
    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    if !config_path.exists() {
        return DEFAULT_SYMBOL_PATTERNS.iter().map(|s| s.to_string()).collect();
    }
    let config = read_json(config_path.to_str().unwrap_or(""));
    let configured = config
        .get("enforcement")
        .and_then(|e| e.get("symbol_patterns"))
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect::<Vec<_>>()
        });
    match configured {
        Some(patterns) if !patterns.is_empty() => patterns,
        _ => DEFAULT_SYMBOL_PATTERNS.iter().map(|s| s.to_string()).collect(),
    }
}

/// Extract plausible top-level symbol names from a source snippet.
/// Regexes come from `.hoangsa/config.json → enforcement.symbol_patterns`
/// (array of regex), falling back to `DEFAULT_SYMBOL_PATTERNS`. Best-effort;
/// grep-level, not AST. Each pattern must expose the symbol name in capture 1.
fn extract_symbols(cwd: &str, text: &str, out: &mut Vec<String>) {
    for pat in read_symbol_patterns(cwd) {
        if let Ok(re) = regex::Regex::new(&pat) {
            for cap in re.captures_iter(text) {
                if let Some(m) = cap.get(1) {
                    out.push(m.as_str().to_string());
                }
            }
        }
    }
}

fn build_impact_event(cwd: &str, tool_input: &serde_json::Value) -> Option<serde_json::Value> {
    let fqn = tool_input.get("fqn").and_then(|v| v.as_str())
        .or_else(|| tool_input.get("target").and_then(|v| v.as_str()))
        .unwrap_or("");
    if fqn.is_empty() {
        return None;
    }

    // FQN that already looks like a path → use as-is.
    let file = if fqn.contains('/') || (fqn.contains('.') && !fqn.contains("::")) {
        fqn.to_string()
    } else {
        resolve_symbol_to_file(cwd, fqn).unwrap_or_default()
    };

    Some(json!({
        "event": "impact",
        "symbol": fqn,
        "file": file,
    }))
}

fn build_detect_changes_event(tool_input: &serde_json::Value, full_payload: &serde_json::Value) -> Option<serde_json::Value> {
    // Try to extract files from tool_result (the actual output of detect_changes)
    let mut files: Vec<String> = Vec::new();

    // If diff was passed as input, extract file paths from it
    if let Some(diff) = tool_input.get("diff").and_then(|v| v.as_str()) {
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                files.push(path.to_string());
            } else if let Some(path) = line.strip_prefix("--- a/") {
                if path != "/dev/null" {
                    files.push(path.to_string());
                }
            }
        }
    }

    // Also check tool_result for file mentions
    if files.is_empty() {
        if let Some(result) = full_payload.get("tool_result").and_then(|v| v.as_str()) {
            // Parse result looking for file paths
            for line in result.lines() {
                let trimmed = line.trim();
                if trimmed.contains('/') && (trimmed.ends_with(".rs") || trimmed.ends_with(".ts") || trimmed.ends_with(".py")) {
                    // Rough extraction of paths
                    for word in trimmed.split_whitespace() {
                        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.' && c != '-' && c != '_');
                        if clean.contains('/') && clean.len() > 3 {
                            files.push(clean.to_string());
                        }
                    }
                }
            }
        }
    }

    files.sort();
    files.dedup();

    Some(json!({
        "event": "detect_changes",
        "files": files,
    }))
}

fn build_recall_event(tool_input: &serde_json::Value) -> Option<serde_json::Value> {
    let query = tool_input.get("query").and_then(|v| v.as_str()).unwrap_or("");
    if query.is_empty() {
        return None;
    }
    Some(json!({
        "event": "recall",
        "query": query,
    }))
}

/// Resolve a symbol (FQN or bare name) to a source file path.
/// 1. Ask the Thoth CLI for the symbol's canonical location (uses the code graph).
/// 2. On miss or when thoth is unavailable, fall back to a config-driven grep
///    built from `enforcement.symbol_patterns` (same source as extract_symbols).
/// Both paths scan from `cwd` — no more hardcoded `cli/src/` / `src/`.
fn resolve_symbol_to_file(cwd: &str, symbol: &str) -> Option<String> {
    use std::process::Command;

    // Strip module prefix: "rule::cmd_rule_add" → "cmd_rule_add".
    let bare = symbol.rsplit("::").next().unwrap_or(symbol);

    // Preferred: Thoth index lookup.
    if let Some(thoth_bin) = find_thoth_bin() {
        let thoth_root = Path::new(cwd).join(".hoangsa-memory");
        if thoth_root.exists() {
            if let Ok(out) = Command::new(&thoth_bin)
                .args(["--root", &thoth_root.to_string_lossy()])
                .args(["context", bare, "--json"])
                .current_dir(cwd)
                .output()
            {
                if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(path) = v.get("symbol").and_then(|s| s.get("path")).and_then(|p| p.as_str()) {
                        return Some(path.to_string());
                    }
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        return Some(path.to_string());
                    }
                }
            }
        }
    }

    // Fallback: in-process regex walk using the configured symbol patterns.
    // Portable across platforms (BSD grep lacks PCRE). Only runs when thoth
    // can't resolve — bounded by depth + source-extension filter.
    let patterns = read_symbol_patterns(cwd);
    let escaped = regex::escape(bare);
    let compiled: Vec<regex::Regex> = patterns
        .iter()
        .map(|pat| pat.replacen("([A-Za-z_][A-Za-z0-9_]*)", &escaped, 1))
        .filter(|p| p.contains(&escaped))
        .filter_map(|p| regex::Regex::new(&p).ok())
        .collect();
    if compiled.is_empty() {
        return None;
    }
    find_symbol_in_tree(cwd, Path::new(cwd), &compiled, 0)
}

/// Recursive DFS over source files looking for any pattern match.
/// Skips vendor/build dirs and binary extensions. Returns the first match.
fn find_symbol_in_tree(
    cwd: &str,
    dir: &Path,
    patterns: &[regex::Regex],
    depth: u32,
) -> Option<String> {
    if depth > 8 {
        return None;
    }
    const SKIP_DIRS: &[&str] = &[
        ".git", "node_modules", "target", "dist", "build", ".hoangsa",
        ".hoangsa-memory", ".claude", "__pycache__", ".venv", "venv", ".next",
    ];
    const SOURCE_EXTS: &[&str] = &[
        "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "c", "cpp",
        "h", "hpp", "rb", "swift", "kt", "scala", "cs", "php", "lua", "ex",
    ];

    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') && depth == 0 {
            continue;
        }
        let ft = match entry.file_type() { Ok(t) => t, Err(_) => continue };
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if let Some(found) = find_symbol_in_tree(cwd, &path, patterns, depth + 1) {
                return Some(found);
            }
        } else if ft.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !SOURCE_EXTS.contains(&ext) {
                continue;
            }
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for re in patterns {
                if re.is_match(&content) {
                    return Some(
                        path.strip_prefix(cwd)
                            .ok()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string_lossy().to_string()),
                    );
                }
            }
        }
    }
    None
}

fn append_event(cwd: &str, event: &serde_json::Value) {
    let events_path = enforcement_events_path(cwd);
    if let Some(parent) = events_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut enriched = event.clone();
    if enriched.get("ts").is_none() {
        let ts = chrono_now();
        enriched.as_object_mut().map(|o| o.insert("ts".to_string(), json!(ts)));
    }

    let mut line = serde_json::to_string(&enriched).unwrap_or_default();
    line.push('\n');

    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .and_then(|mut f| {
            use std::io::Write as _;
            f.write_all(line.as_bytes())
        });
}

// ── Enforce Override + Report ────────────────────────────────────────────────

/// `enforce override --rule <id> --target <path> --reason <text>`
///
/// Records a scoped override event. The enforce hook checks for these
/// before blocking, allowing bypass of false positives.
pub fn cmd_enforce_override(cwd: &str, args: &[&str]) {
    let rule = flag_value(args, "--rule").unwrap_or("");
    let target = flag_value(args, "--target").unwrap_or("");
    let reason = flag_value(args, "--reason").unwrap_or("");

    if rule.is_empty() || target.is_empty() {
        out(&json!({"success": false, "error": "Required: --rule <id> --target <path> --reason <text>"}));
        return;
    }
    if reason.is_empty() {
        out(&json!({"success": false, "error": "--reason is required (explains why override is safe)"}));
        return;
    }

    let event = json!({
        "event": "override",
        "rule": rule,
        "target": target,
        "reason": reason,
    });

    append_event(cwd, &event);
    out(&json!({"success": true, "rule": rule, "target": target, "reason": reason}));
}

/// `enforce report`
///
/// Aggregates enforcement events into a human-readable summary.
pub fn cmd_enforce_report(cwd: &str) {
    let events_path = enforcement_events_path(cwd);
    let content = fs::read_to_string(&events_path).unwrap_or_default();

    if content.is_empty() {
        out(&json!({"report": "No enforcement events recorded this session."}));
        return;
    }

    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut warns: Vec<(String, String)> = Vec::new();
    let mut overrides: Vec<(String, String, String)> = Vec::new();
    let mut drifts: Vec<(String, Vec<String>)> = Vec::new();
    let mut impacts = 0u32;
    let mut detect_changes = 0u32;
    let mut recalls = 0u32;

    for line in content.lines() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            let event_type = entry.get("event").and_then(|e| e.as_str()).unwrap_or("");
            match event_type {
                "block" => {
                    let rule = entry.get("rule").and_then(|r| r.as_str()).unwrap_or("?").to_string();
                    let target = entry.get("target").and_then(|t| t.as_str()).unwrap_or("?").to_string();
                    blocks.push((rule, target));
                }
                "warn" => {
                    let rule = entry.get("rule").and_then(|r| r.as_str()).unwrap_or("?").to_string();
                    let target = entry.get("target").and_then(|t| t.as_str()).unwrap_or("?").to_string();
                    warns.push((rule, target));
                }
                "override" => {
                    let rule = entry.get("rule").and_then(|r| r.as_str()).unwrap_or("?").to_string();
                    let target = entry.get("target").and_then(|t| t.as_str()).unwrap_or("?").to_string();
                    let reason = entry.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string();
                    overrides.push((rule, target, reason));
                }
                "drift_warn" => {
                    let file = entry.get("file").and_then(|f| f.as_str()).unwrap_or("?").to_string();
                    let uncovered: Vec<String> = entry
                        .get("uncovered")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
                        .unwrap_or_default();
                    drifts.push((file, uncovered));
                }
                "impact" => impacts += 1,
                "detect_changes" => detect_changes += 1,
                "recall" => recalls += 1,
                _ => {}
            }
        }
    }

    let total_events = content.lines().count();
    let fp_risk = if blocks.is_empty() {
        0.0
    } else {
        overrides.len() as f64 / blocks.len() as f64
    };

    let report = json!({
        "total_events": total_events,
        "blocks": blocks.len(),
        "warns": warns.len(),
        "overrides": overrides.len(),
        "drifts": drifts.len(),
        "impacts": impacts,
        "detect_changes": detect_changes,
        "recalls": recalls,
        "fp_risk": format!("{:.2}", fp_risk),
        "top_blocks": blocks,
        "top_warns": warns,
        "override_details": overrides,
        "drift_details": drifts,
    });

    out(&report);
}

// ── Enforcement State: append-only JSONL event log ──────────────────────────

fn enforcement_events_path(cwd: &str) -> std::path::PathBuf {
    Path::new(cwd)
        .join(".hoangsa")
        .join("state")
        .join("enforcement.events")
}

/// `hook state-record`
///
/// Appends a single enforcement event (JSONL line) to `.hoangsa/state/enforcement.events`.
/// Reads event JSON from stdin. Adds `ts` field if missing.
/// Always outputs `{"decision":"approve"}` — never blocks.
pub fn cmd_state_record(cwd: &str) {
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let mut event: serde_json::Value = match serde_json::from_str(input.trim()) {
        Ok(v) => v,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    if event.get("ts").is_none() {
        let ts = chrono_now();
        event.as_object_mut().map(|o| o.insert("ts".to_string(), json!(ts)));
    }

    let events_path = enforcement_events_path(cwd);
    if let Some(parent) = events_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut line = serde_json::to_string(&event).unwrap_or_default();
    line.push('\n');

    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .and_then(|mut f| {
            use std::io::Write as _;
            f.write_all(line.as_bytes())
        });

    out(&json!({"decision": "approve"}));
}

/// `hook state-check --event <type> [--file <path>] [--symbol <name>]`
///
/// Checks if a matching event exists in the enforcement log.
/// Outputs JSON: `{"found": true/false, "event": ...}` or `{"found": false}`.
pub fn cmd_state_check(cwd: &str, args: &[&str]) {
    let event_type = flag_value(args, "--event").unwrap_or("");
    let file_filter = flag_value(args, "--file");
    let symbol_filter = flag_value(args, "--symbol");

    if event_type.is_empty() {
        out(&json!({"found": false, "error": "missing --event flag"}));
        return;
    }

    let events_path = enforcement_events_path(cwd);
    let content = match fs::read_to_string(&events_path) {
        Ok(c) => c,
        Err(_) => {
            out(&json!({"found": false}));
            return;
        }
    };

    for line in content.lines().rev() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if entry.get("event").and_then(|e| e.as_str()) != Some(event_type) {
            continue;
        }

        if let Some(file) = file_filter {
            let entry_file = entry.get("file").and_then(|f| f.as_str()).unwrap_or("");
            if entry_file != file {
                continue;
            }
        }

        if let Some(symbol) = symbol_filter {
            let entry_sym = entry.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            if entry_sym != symbol {
                continue;
            }
        }

        out(&json!({"found": true, "event": entry}));
        return;
    }

    out(&json!({"found": false}));
}

/// `hook state-clear`
///
/// Removes the enforcement events file (used at session start).
pub fn cmd_state_clear(cwd: &str) {
    let events_path = enforcement_events_path(cwd);
    let _ = fs::remove_file(&events_path);
    out(&json!({"success": true}));
}

fn flag_value<'a>(args: &'a [&'a str], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|&a| a == flag)
        .and_then(|i| args.get(i + 1))
        .copied()
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}Z", secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── intent_guard_edit ────────────────────────────────────────────────────

    #[test]
    fn test_intent_guard_edit_empty_log_blocks() {
        let result = intent_guard_edit("", "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)), "empty events must block");
    }

    #[test]
    fn test_intent_guard_edit_matching_impact_approves() {
        let events = r#"{"event":"impact","file":"cli/src/cmd/foo.rs","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/Users/me/proj/cli/src/cmd/foo.rs");
        assert_eq!(result, IntentOutcome::Approve, "abs↔rel path match should approve");
    }

    #[test]
    fn test_intent_guard_edit_rejects_empty_file_field() {
        // An impact event where file resolution failed (empty string) must NOT
        // satisfy the guard — otherwise every unresolved event would unlock every file.
        let events = r#"{"event":"impact","file":"","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_override_approves() {
        let events = r#"{"event":"override","rule":"require-thoth-impact","target":"/abs/path/foo.rs","reason":"test"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_edit_override_for_different_rule_blocks() {
        let events = r#"{"event":"override","rule":"some-other-rule","target":"/abs/path/foo.rs","reason":"test"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_different_file_blocks() {
        let events = r#"{"event":"impact","file":"cli/src/cmd/foo.rs","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/Users/me/proj/cli/src/cmd/bar.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_malformed_lines_skipped() {
        // Malformed JSON lines must not crash or satisfy the guard.
        let events = "garbage line\n{invalid json\n\n";
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    // ── intent_guard_bash_commit ─────────────────────────────────────────────

    #[test]
    fn test_intent_guard_bash_no_detect_changes_blocks() {
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit("", &files);
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_bash_override_approves() {
        let events = r#"{"event":"override","rule":"require-detect-changes","target":"commit","reason":"..."}
"#;
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit(events, &files);
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_bash_detect_changes_covers_diff() {
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit(events, &files);
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_bash_diff_grew_warns() {
        // detect_changes covered foo.rs but bar.rs snuck into the staged diff.
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let files = vec![
            "cli/src/cmd/foo.rs".to_string(),
            "cli/src/cmd/bar.rs".to_string(),
        ];
        let result = intent_guard_bash_commit(events, &files);
        match result {
            IntentOutcome::Warn(msg) => assert!(msg.contains("bar.rs")),
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn test_intent_guard_bash_empty_staged_files_approves() {
        // No staged files (e.g. `git commit --amend` no-op) → nothing to correlate.
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let result = intent_guard_bash_commit(events, &[]);
        assert_eq!(result, IntentOutcome::Approve);
    }

    // ── build_recall_query ───────────────────────────────────────────────────

    #[test]
    fn test_build_recall_query_relative_path() {
        let q = build_recall_query("src/cmd/pref.rs");
        assert_eq!(q, "NEVER edit src/cmd/pref.rs");
    }

    #[test]
    fn test_build_recall_query_empty_path() {
        let q = build_recall_query("");
        // empty path → empty after strip → "NEVER edit "
        assert!(q.starts_with("NEVER edit"));
    }

    #[test]
    fn test_build_recall_query_absolute_non_home_path() {
        // path that is definitely not under HOME: /tmp/file.rs
        let q = build_recall_query("/tmp/file.rs");
        assert!(q.contains("tmp/file.rs"), "expected path segment in query, got: {q}");
        assert!(q.starts_with("NEVER edit"));
    }

    // ── count_incomplete_tasks ───────────────────────────────────────────────

    #[test]
    fn test_count_incomplete_tasks_all_pending() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "pending" },
                { "id": "T-02", "status": "running" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 2);
    }

    #[test]
    fn test_count_incomplete_tasks_all_done() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "completed" },
                { "id": "T-02", "status": "done" },
                { "id": "T-03", "status": "skipped" },
                { "id": "T-04", "status": "failed" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    #[test]
    fn test_count_incomplete_tasks_mixed() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "completed" },
                { "id": "T-02", "status": "pending" },
                { "id": "T-03", "status": "running" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 2);
    }

    #[test]
    fn test_count_incomplete_tasks_missing_status() {
        // Missing status field defaults to "pending" (incomplete)
        let plan = json!({
            "tasks": [
                { "id": "T-01" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 1);
    }

    #[test]
    fn test_count_incomplete_tasks_no_tasks_key() {
        let plan = json!({});
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    #[test]
    fn test_count_incomplete_tasks_empty_tasks() {
        let plan = json!({ "tasks": [] });
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    // ── enforcement state ───────────────────────────────────────────────────

    #[test]
    fn test_enforcement_events_path() {
        let p = enforcement_events_path("/tmp/project");
        assert_eq!(
            p.to_string_lossy(),
            "/tmp/project/.hoangsa/state/enforcement.events"
        );
    }

    #[test]
    fn test_flag_value_found() {
        let args = vec!["--event", "impact", "--file", "foo.rs"];
        assert_eq!(flag_value(&args, "--event"), Some("impact"));
        assert_eq!(flag_value(&args, "--file"), Some("foo.rs"));
    }

    #[test]
    fn test_flag_value_not_found() {
        let args = vec!["--event", "impact"];
        assert_eq!(flag_value(&args, "--file"), None);
    }

    #[test]
    fn test_flag_value_at_end() {
        let args = vec!["--event"];
        assert_eq!(flag_value(&args, "--event"), None);
    }

    #[test]
    fn test_chrono_now_format() {
        let ts = chrono_now();
        assert!(ts.ends_with('Z'));
        let num_part = &ts[..ts.len() - 1];
        assert!(num_part.parse::<u64>().is_ok());
    }
}

/// Find the most recently modified session directory.
fn find_latest_session(sessions_root: &str) -> Option<String> {
    let root = Path::new(sessions_root);
    let type_dirs = fs::read_dir(root).ok()?;

    let known_types = ["feat", "fix", "refactor", "perf", "test", "docs", "chore"];
    let mut best: Option<(std::time::SystemTime, String)> = None;

    for type_entry in type_dirs.filter_map(|e| e.ok()) {
        let ft = type_entry.file_type().ok()?;
        if !ft.is_dir() {
            continue;
        }
        let type_name = type_entry.file_name().into_string().ok()?;
        if !known_types.contains(&type_name.as_str()) {
            continue;
        }

        let name_dirs = match fs::read_dir(type_entry.path()) {
            Ok(d) => d,
            Err(_) => continue,
        };

        for name_entry in name_dirs.filter_map(|e| e.ok()) {
            if !name_entry
                .file_type()
                .map(|ft| ft.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let mtime = name_entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);

            if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                best = Some((mtime, name_entry.path().to_string_lossy().to_string()));
            }
        }
    }

    best.map(|(_, path)| path)
}
