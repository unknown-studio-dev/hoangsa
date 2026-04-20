use crate::helpers::{out, read_json};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

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
    let thoth_root = Path::new(cwd).join(".thoth");
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

/// `hook compact-check`
///
/// PostToolUse hook. Increments a mutation counter and, when the counter
/// crosses `auto_compact_interval` (default 500) with at least
/// `auto_compact_cooldown_secs` (default 86400) since the last run,
/// spawns `thoth compact` in a detached background process.
///
/// Silent by design — it always emits `{"decision":"approve"}` so it never
/// blocks tool calls. Failures log to `.hoangsa/state/compact-check.log`.
pub fn cmd_compact_check(cwd: &str) {
    use std::io::Read;
    use std::process::{Command, Stdio};

    // Always approve — this hook must never block.
    // We still read stdin to drain the pipe cleanly.
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    // Check preferences (opt-out + tuning).
    let (enabled, interval, cooldown) = read_compact_prefs(cwd);
    if !enabled {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Only count mutation tool calls.
    let parsed: Value = serde_json::from_str(&input).unwrap_or(json!({}));
    let tool = parsed
        .get("tool_name")
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if !matches!(tool, "Bash" | "Write" | "Edit" | "NotebookEdit") {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Require a .thoth/ dir — nothing to compact otherwise.
    if !Path::new(cwd).join(".thoth").exists() {
        out(&json!({"decision": "approve"}));
        return;
    }

    let state_dir = Path::new(cwd).join(".hoangsa").join("state");
    if fs::create_dir_all(&state_dir).is_err() {
        out(&json!({"decision": "approve"}));
        return;
    }
    let counter_path = state_dir.join("compact-counter.json");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let current = read_json(counter_path.to_str().unwrap_or(""));
    let prev_count = current
        .get("count")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let last_ts = current
        .get("last_ts")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let new_count = prev_count + 1;
    let should_run = new_count >= interval && now.saturating_sub(last_ts) >= cooldown;

    let (count_to_write, ts_to_write) = if should_run {
        (0u64, now)
    } else {
        (new_count, last_ts)
    };

    let _ = fs::write(
        &counter_path,
        serde_json::to_string_pretty(&json!({
            "count": count_to_write,
            "last_ts": ts_to_write,
        }))
        .unwrap_or_default(),
    );

    if should_run {
        if let Some(thoth_bin) = find_thoth_bin() {
            let thoth_root = Path::new(cwd).join(".thoth");
            let log_path = state_dir.join("compact-check.log");
            let log_file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .ok();
            let stdout_redirect = log_file
                .as_ref()
                .and_then(|f| f.try_clone().ok())
                .map(Stdio::from)
                .unwrap_or_else(Stdio::null);
            let stderr_redirect = log_file
                .map(Stdio::from)
                .unwrap_or_else(Stdio::null);
            let _ = Command::new(&thoth_bin)
                .args(["--root", &thoth_root.to_string_lossy()])
                .arg("compact")
                .stdin(Stdio::null())
                .stdout(stdout_redirect)
                .stderr(stderr_redirect)
                .spawn();
        }
    }

    out(&json!({"decision": "approve"}));
}

/// Read compact prefs from `.hoangsa/config.json` with defaults.
/// Returns `(enabled, interval, cooldown_secs)`.
fn read_compact_prefs(cwd: &str) -> (bool, u64, u64) {
    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    let default = (true, 500u64, 86_400u64);
    if !config_path.exists() {
        return default;
    }
    let config = read_json(config_path.to_str().unwrap_or(""));
    if config.get("error").is_some() {
        return default;
    }
    let prefs = match config.get("preferences") {
        Some(p) => p,
        None => return default,
    };
    let enabled = prefs
        .get("auto_compact")
        .map(coerce_bool)
        .unwrap_or(None)
        .unwrap_or(default.0);
    let interval = prefs
        .get("auto_compact_interval")
        .and_then(coerce_u64)
        .filter(|&n| n > 0)
        .unwrap_or(default.1);
    let cooldown = prefs
        .get("auto_compact_cooldown_secs")
        .and_then(coerce_u64)
        .unwrap_or(default.2);
    (enabled, interval, cooldown)
}

/// Coerce a JSON value to u64, accepting both numbers and numeric strings
/// (since `hoangsa-cli pref set` stores everything as strings).
fn coerce_u64(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.parse().ok())
}

fn coerce_bool(v: &Value) -> Option<bool> {
    v.as_bool().or_else(|| match v.as_str()? {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    })
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

/// `hook thoth-gate-proxy`
///
/// Conditional wrapper for thoth-gate. Checks thoth_strict preference:
/// - false → out({"decision":"approve"}) (bypass gate)
/// - true → spawn thoth-gate binary, pipe stdin, capture stdout
pub fn cmd_thoth_gate_proxy(cwd: &str) {
    use std::io::{Read, Write};
    use std::process::{Command, Stdio};

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    // Default true: existing setups without the key keep the strict behaviour.
    let thoth_strict = {
        let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
        if config_path.exists() {
            let config = read_json(config_path.to_str().unwrap_or(""));
            config
                .get("preferences")
                .and_then(|p| p.get("thoth_strict"))
                .and_then(coerce_bool)
                .unwrap_or(true)
        } else {
            true
        }
    };

    if !thoth_strict {
        out(&json!({"decision": "approve"}));
        return;
    }

    if let Some(gate_bin) = find_thoth_gate_bin() {
        if let Ok(mut child) = Command::new(&gate_bin)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
        {
            if let Some(mut stdin_handle) = child.stdin.take() {
                stdin_handle.write_all(input.as_bytes()).ok();
            }
            if let Ok(o) = child.wait_with_output() {
                if !o.stdout.is_empty() {
                    print!("{}", String::from_utf8_lossy(&o.stdout));
                    return;
                }
            }
        }
    }

    eprintln!("hoangsa: thoth-gate-proxy: thoth-gate binary not found, approving");
    out(&json!({"decision": "approve"}));
}

/// Find a binary by searching PATH (cross-platform).
/// `stem` is the binary name without extension (e.g. "thoth", "thoth-gate").
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

fn find_thoth_gate_bin() -> Option<String> {
    find_bin_in_path("thoth-gate")
}

fn find_thoth_bin() -> Option<String> {
    find_bin_in_path("thoth")
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
        "Edit" | "Write" => stateful_check_edit(cwd, tool_input),
        "Bash" => stateful_check_bash(cwd, tool_input),
        _ => None,
    }
}

/// Rule #9: Require thoth_impact for first-touch files before Edit.
/// Softened: if the file already has events (prior impact or edit), skip.
fn stateful_check_edit(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let file_path = tool_input.get("file_path").and_then(|v| v.as_str())?;

    // Skip non-source files (configs, markdown, etc.)
    if !is_source_file(file_path) {
        return None;
    }

    let events_path = enforcement_events_path(cwd);
    let content = fs::read_to_string(&events_path).unwrap_or_default();

    if content.is_empty() {
        // No events at all — this is first-touch, require impact
        return Some(EnforceResult {
            decision: "block".to_string(),
            reason: format!(
                "⛔ STATEFUL: require-thoth-impact\n\n\
                 No thoth_impact found for '{}'\n\
                 Run thoth_impact on this file before editing.\n\n\
                 If this is a false positive, use:\n\
                 hoangsa-cli enforce override --rule require-thoth-impact --target {} --reason \"...\"",
                file_path, file_path
            ),
            warning: None,
        });
    }

    // Check if this file has any prior events (impact, edit, or override for this rule)
    let file_has_events = content.lines().any(|line| {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            let event_type = entry.get("event").and_then(|e| e.as_str()).unwrap_or("");
            match event_type {
                "impact" => {
                    entry.get("file").and_then(|f| f.as_str()) == Some(file_path)
                }
                "override" => {
                    entry.get("rule").and_then(|r| r.as_str()) == Some("require-thoth-impact")
                        && entry.get("target").and_then(|t| t.as_str()) == Some(file_path)
                }
                _ => false,
            }
        } else {
            false
        }
    });

    if file_has_events {
        None // Already checked — allow iterative edits
    } else {
        Some(EnforceResult {
            decision: "block".to_string(),
            reason: format!(
                "⛔ STATEFUL: require-thoth-impact\n\n\
                 No thoth_impact found for '{}'\n\
                 Run thoth_impact on this file before editing.\n\n\
                 If this is a false positive, use:\n\
                 hoangsa-cli enforce override --rule require-thoth-impact --target {} --reason \"...\"",
                file_path, file_path
            ),
            warning: None,
        })
    }
}

/// Rule #10: Require detect_changes before git commit.
/// Intent guard: if detect_changes exists, compare its files against `git diff --cached`.
fn stateful_check_bash(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let command = tool_input.get("command").and_then(|v| v.as_str())?;

    if !is_git_commit(command) {
        return None;
    }

    let events_path = enforcement_events_path(cwd);
    let content = fs::read_to_string(&events_path).unwrap_or_default();

    // Collect detect_changes files and check for override
    let mut detected_files: Vec<String> = Vec::new();
    let mut has_override = false;

    for line in content.lines() {
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            let event_type = entry.get("event").and_then(|e| e.as_str()).unwrap_or("");
            if event_type == "detect_changes" {
                if let Some(files) = entry.get("files").and_then(|f| f.as_array()) {
                    for f in files {
                        if let Some(s) = f.as_str() {
                            detected_files.push(s.to_string());
                        }
                    }
                }
            } else if event_type == "override"
                && entry.get("rule").and_then(|r| r.as_str()) == Some("require-detect-changes")
            {
                has_override = true;
            }
        }
    }

    if has_override {
        return None;
    }

    if detected_files.is_empty() {
        return Some(EnforceResult {
            decision: "block".to_string(),
            reason: "⛔ STATEFUL: require-detect-changes\n\n\
                     No thoth_detect_changes found before commit.\n\
                     Run thoth_detect_changes to verify scope before committing.\n\n\
                     If this is a false positive, use:\n\
                     hoangsa-cli enforce override --rule require-detect-changes --target commit --reason \"...\""
                .to_string(),
            warning: None,
        });
    }

    // Intent guard: compare detected files against actual staged diff
    let diff_files = get_staged_files(cwd);
    if diff_files.is_empty() {
        return None; // Can't determine diff — allow
    }

    let uncovered: Vec<&String> = diff_files
        .iter()
        .filter(|f| !detected_files.iter().any(|d| f.ends_with(d.as_str()) || d.ends_with(f.as_str())))
        .collect();

    if uncovered.is_empty() {
        None
    } else {
        let uncovered_list = uncovered.iter().map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
        Some(EnforceResult {
            decision: "approve".to_string(),
            reason: String::new(),
            warning: Some(format!(
                "⚠️ INTENT GUARD: Files in staged diff not covered by detect_changes: [{}]\n\
                 Consider re-running thoth_detect_changes before commit.",
                uncovered_list
            )),
        })
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
        "mcp__thoth__thoth_impact" => build_impact_event(&tool_input),
        "mcp__thoth__thoth_detect_changes" => build_detect_changes_event(&tool_input, &parsed),
        "mcp__thoth__thoth_recall" => build_recall_event(&tool_input),
        _ => None,
    };

    if let Some(event) = event {
        append_event(cwd, &event);
    }

    out(&json!({"decision": "approve"}));
}

fn build_impact_event(tool_input: &serde_json::Value) -> Option<serde_json::Value> {
    let fqn = tool_input.get("fqn").and_then(|v| v.as_str())
        .or_else(|| tool_input.get("target").and_then(|v| v.as_str()))
        .unwrap_or("");
    if fqn.is_empty() {
        return None;
    }

    // Try to resolve FQN to file path — use the FQN itself if it looks like a path
    let file = if fqn.contains('/') || fqn.contains('.') && !fqn.contains("::") {
        fqn.to_string()
    } else {
        // For Rust FQNs like "cmd_rule_sync", try to find the file via grep
        resolve_symbol_to_file(fqn).unwrap_or_default()
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

fn resolve_symbol_to_file(symbol: &str) -> Option<String> {
    use std::process::Command;
    // Quick grep for the symbol in source files
    let output = Command::new("grep")
        .args(["-rl", &format!("fn {symbol}"), "cli/src/", "src/"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|l| l.to_string())
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
        "impacts": impacts,
        "detect_changes": detect_changes,
        "recalls": recalls,
        "fp_risk": format!("{:.2}", fp_risk),
        "top_blocks": blocks,
        "top_warns": warns,
        "override_details": overrides,
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

    // ── coerce_bool ──────────────────────────────────────────────────────────

    #[test]
    fn test_coerce_bool_true_literal() {
        assert_eq!(coerce_bool(&json!(true)), Some(true));
    }

    #[test]
    fn test_coerce_bool_false_literal() {
        assert_eq!(coerce_bool(&json!(false)), Some(false));
    }

    #[test]
    fn test_coerce_bool_string_true() {
        assert_eq!(coerce_bool(&json!("true")), Some(true));
    }

    #[test]
    fn test_coerce_bool_string_false() {
        assert_eq!(coerce_bool(&json!("false")), Some(false));
    }

    #[test]
    fn test_coerce_bool_string_yes() {
        assert_eq!(coerce_bool(&json!("yes")), Some(true));
    }

    #[test]
    fn test_coerce_bool_string_no() {
        assert_eq!(coerce_bool(&json!("no")), Some(false));
    }

    #[test]
    fn test_coerce_bool_string_1() {
        assert_eq!(coerce_bool(&json!("1")), Some(true));
    }

    #[test]
    fn test_coerce_bool_string_0() {
        assert_eq!(coerce_bool(&json!("0")), Some(false));
    }

    #[test]
    fn test_coerce_bool_invalid_string() {
        assert_eq!(coerce_bool(&json!("maybe")), None);
    }

    #[test]
    fn test_coerce_bool_null() {
        assert_eq!(coerce_bool(&json!(null)), None);
    }

    #[test]
    fn test_coerce_bool_number() {
        // Numbers are not booleans and not strings — returns None
        assert_eq!(coerce_bool(&json!(1)), None);
    }

    // ── coerce_u64 ──────────────────────────────────────────────────────────

    #[test]
    fn test_coerce_u64_number() {
        assert_eq!(coerce_u64(&json!(42u64)), Some(42));
    }

    #[test]
    fn test_coerce_u64_zero() {
        assert_eq!(coerce_u64(&json!(0u64)), Some(0));
    }

    #[test]
    fn test_coerce_u64_numeric_string() {
        assert_eq!(coerce_u64(&json!("500")), Some(500));
    }

    #[test]
    fn test_coerce_u64_non_numeric_string() {
        assert_eq!(coerce_u64(&json!("abc")), None);
    }

    #[test]
    fn test_coerce_u64_null() {
        assert_eq!(coerce_u64(&json!(null)), None);
    }

    #[test]
    fn test_coerce_u64_negative_string() {
        // "-1" doesn't parse as u64
        assert_eq!(coerce_u64(&json!("-1")), None);
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
