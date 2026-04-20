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
