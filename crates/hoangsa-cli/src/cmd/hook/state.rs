use crate::helpers::out;
use serde_json::json;
use std::fs;

use super::{
    chrono_now, enforcement_events_path, flag_value, graph_affordance_sentinel_path,
    reflect_sentinel_path,
};

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
/// Fires on every SessionStart (startup, resume, clear). Clears the
/// enforcement events file, and on `source == "clear"` snapshots the
/// statusline cost baseline so the displayed cost resets to $0.00.
pub fn cmd_state_clear(cwd: &str) {
    // Read the payload FIRST: the events file is per-project but its contents
    // are per-session, so a blanket delete wiped a sibling session's log and
    // blocked it on its next edit for work it had already done.
    let mut raw_early = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw_early);
    let payload_early: serde_json::Value =
        serde_json::from_str(&raw_early).unwrap_or_else(|_| json!({}));
    let my_session = payload_early
        .get("session_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string);

    let events_path = enforcement_events_path(cwd);
    match my_session.as_deref() {
        // Drop only our own events; untagged lines are legacy and belong to
        // nobody, so they go too.
        Some(sid) => {
            if let Ok(text) = fs::read_to_string(&events_path) {
                let kept: Vec<&str> = text
                    .lines()
                    .filter(|line| {
                        serde_json::from_str::<serde_json::Value>(line)
                            .ok()
                            .and_then(|v| {
                                v.get("session").and_then(|s| s.as_str()).map(str::to_string)
                            })
                            .is_some_and(|owner| owner != sid)
                    })
                    .collect();
                if kept.is_empty() {
                    let _ = fs::remove_file(&events_path);
                } else {
                    let mut body = kept.join("\n");
                    body.push('\n');
                    let _ = crate::helpers::atomic_write_string(&events_path, &body);
                }
            }
        }
        // No session id in the payload — fall back to the old behaviour.
        None => {
            let _ = fs::remove_file(&events_path);
        }
    }
    let _ = fs::remove_file(reflect_sentinel_path(cwd));
    let _ = fs::remove_file(graph_affordance_sentinel_path(cwd));

    // Reuse the payload read above — stdin is already drained, so re-reading
    // it here would always see an empty string and silently skip the reset.
    {
        let source = payload_early
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if source == "clear"
            && let Some(sid) = my_session.as_deref()
        {
            snapshot_statusline_baseline(sid);
        }
    }

    // Decision shape, not a bare `{"success": true}` — the codex wire
    // translator passes non-decision JSON through verbatim, and Codex's
    // deny_unknown_fields parser would mark the hook Failed on every
    // SessionStart.
    out(&json!({"decision": "approve", "success": true}));
}

/// On `/clear`, promote the last-seen cost into the baseline so the
/// statusline displays `max(0, total - baseline) = 0` until the new
/// conversation accrues cost. Rewrites the stored session_id from the
/// payload so the next statusline tick (which may carry a fresh sid
/// from CC) still treats the baseline as current.
fn snapshot_statusline_baseline(session_id: &str) {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
    let run_dir = home.join(".hoangsa").join("run");
    let path = crate::cmd::statusline::cost_state_path(&run_dir);
    let Some(mut state) = crate::cmd::statusline::read_cost_state(&path) else { return };
    state.baseline = state.last_seen;
    state.session_id = session_id.to_string();
    crate::cmd::statusline::write_cost_state(&path, &state);
}
