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
                        "decision": "block",
                        "reason": format!(
                            "Workflow incomplete: {} task(s) still pending/running in session {}. Complete all tasks before stopping.",
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

            if best.as_ref().map_or(true, |(t, _)| mtime > *t) {
                best = Some((mtime, name_entry.path().to_string_lossy().to_string()));
            }
        }
    }

    best.map(|(_, path)| path)
}
