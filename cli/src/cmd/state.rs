use crate::helpers::{out, read_json};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::macros::format_description;

struct ConfigPrefs {
    language: String,
    auto_taste: Value,
    auto_plate: Value,
    auto_serve: Value,
}

fn read_config_prefs(cwd: &str) -> ConfigPrefs {
    let config_file = Path::new(cwd).join(".hoangsa").join("config.json");
    let prefs = fs::read_to_string(&config_file)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("preferences").cloned());

    let pref = |key: &str| -> Value {
        prefs
            .as_ref()
            .and_then(|p| p.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    };

    let language = prefs
        .as_ref()
        .and_then(|p| p.get("lang"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| "en".to_string());

    ConfigPrefs {
        language,
        auto_taste: pref("auto_taste"),
        auto_plate: pref("auto_plate"),
        auto_serve: pref("auto_serve"),
    }
}

fn now_iso() -> String {
    let now = OffsetDateTime::now_utc();
    now.format(format_description!(
        "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
    ))
    .unwrap_or_default()
}

/// Resolve a sessionDir argument to an absolute path.
/// Accepts either:
///   - An absolute path (returned as-is)
///   - A session ID like "chore/professional-readme" (resolved via cwd/.hoangsa/sessions/)
///   - A relative path (resolved via cwd/.hoangsa/sessions/ if it contains state.json there,
///     otherwise used as-is relative to cwd)
fn resolve_session_dir(raw: &str, cwd: &str) -> PathBuf {
    let p = Path::new(raw);
    // Already absolute → use as-is
    if p.is_absolute() {
        return p.to_path_buf();
    }
    // Try resolving as session ID under cwd/.hoangsa/sessions/
    let via_sessions = Path::new(cwd)
        .join(".hoangsa")
        .join("sessions")
        .join(raw);
    if via_sessions.join("state.json").exists() || via_sessions.exists() {
        return via_sessions;
    }
    // Fallback: relative to cwd
    Path::new(cwd).join(raw)
}

/// `state init <sessionDir>`
pub fn cmd_init(session_dir: Option<&str>, cwd: &str) {
    let session_dir_resolved;
    let session_dir = match session_dir {
        Some(d) => {
            session_dir_resolved = resolve_session_dir(d, cwd);
            session_dir_resolved.to_str().unwrap_or(d)
        }
        None => {
            out(&json!({ "error": "sessionDir is required" }));
            return;
        }
    };

    let state_file = Path::new(session_dir).join("state.json");
    if state_file.exists() {
        out(&json!({
            "error": "state.json already exists",
            "path": state_file.to_string_lossy(),
        }));
        return;
    }

    // Extract session_id as "<type>/<name>" from the last two path components,
    // or fall back to just the last component for legacy sessions.
    let session_path = Path::new(session_dir);
    let session_name = session_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    let type_prefix = session_path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|t| t.to_str())
        .filter(|t| !["sessions", ".hoangsa"].contains(t));
    let session_id = type_prefix
        .map(|t| format!("{t}/{session_name}"))
        .unwrap_or_else(|| session_name.to_string());
    let task_type = type_prefix.unwrap_or("feat").to_string();
    let prefs = read_config_prefs(cwd);
    let now = now_iso();

    let state = json!({
        "session_id": session_id,
        "status": "design",
        "task_type": task_type,
        "language": prefs.language,
        "preferences": {
            "auto_taste": prefs.auto_taste,
            "auto_plate": prefs.auto_plate,
            "auto_serve": prefs.auto_serve,
        },
        "created_at": now,
        "updated_at": now,
    });

    if let Err(e) = fs::create_dir_all(session_dir) {
        out(&json!({ "success": false, "error": e.to_string() }));
        return;
    }
    match fs::write(&state_file, serde_json::to_string_pretty(&state).unwrap()) {
        Ok(_) => out(&json!({
            "success": true,
            "path": state_file.to_string_lossy(),
            "state": state,
        })),
        Err(e) => out(&json!({ "success": false, "error": e.to_string() })),
    }
}

/// `state get <sessionDir>`
pub fn cmd_get(session_dir: Option<&str>, cwd: &str) {
    let session_dir_resolved;
    let session_dir = match session_dir {
        Some(d) => {
            session_dir_resolved = resolve_session_dir(d, cwd);
            session_dir_resolved.to_str().unwrap_or(d)
        }
        None => {
            out(&json!({ "error": "sessionDir is required" }));
            return;
        }
    };

    let state_file = Path::new(session_dir).join("state.json");
    if !state_file.exists() {
        out(&json!({ "error": format!("state.json not found at {}. Run `state init` first.", state_file.display()) }));
        return;
    }
    let state = read_json(state_file.to_str().unwrap_or(""));
    if state.get("error").is_some() {
        out(&json!({ "error": state["error"] }));
        return;
    }
    out(&state);
}

/// `state update <sessionDir> <jsonPatch>`
pub fn cmd_update(session_dir: Option<&str>, json_patch: Option<&str>, cwd: &str) {
    let session_dir_resolved;
    let session_dir = match session_dir {
        Some(d) => {
            session_dir_resolved = resolve_session_dir(d, cwd);
            session_dir_resolved.to_str().unwrap_or(d)
        }
        None => {
            out(&json!({ "error": "sessionDir is required" }));
            return;
        }
    };
    let json_patch = match json_patch {
        Some(p) => p,
        None => {
            out(&json!({ "error": "jsonPatch is required" }));
            return;
        }
    };

    let state_file = Path::new(session_dir).join("state.json");
    if !state_file.exists() {
        out(&json!({ "error": format!("state.json not found at {}. Run `state init` first.", state_file.display()) }));
        return;
    }
    let state = read_json(state_file.to_str().unwrap_or(""));
    if state.get("error").is_some() {
        out(&json!({ "error": state["error"] }));
        return;
    }

    let patch: Value = match serde_json::from_str(json_patch) {
        Ok(v) => v,
        Err(e) => {
            out(&json!({ "error": format!("Invalid JSON patch: {}", e) }));
            return;
        }
    };

    // Shallow merge: state + patch + updated_at
    let mut updated = state.as_object().cloned().unwrap_or_default();
    if let Some(patch_obj) = patch.as_object() {
        for (k, v) in patch_obj {
            updated.insert(k.clone(), v.clone());
        }
    }
    updated.insert("updated_at".to_string(), json!(now_iso()));

    // Deep merge preferences if patch includes it
    if let (Some(patch_prefs), Some(state_prefs)) = (
        patch.get("preferences").and_then(|v| v.as_object()),
        state.get("preferences").and_then(|v| v.as_object()),
    ) {
        let mut merged_prefs: Map<String, Value> = state_prefs.clone();
        for (k, v) in patch_prefs {
            merged_prefs.insert(k.clone(), v.clone());
        }
        updated.insert("preferences".to_string(), Value::Object(merged_prefs));
    }

    let updated_val = Value::Object(updated);
    match fs::write(
        &state_file,
        serde_json::to_string_pretty(&updated_val).unwrap(),
    ) {
        Ok(_) => out(&json!({ "success": true, "state": updated_val })),
        Err(e) => out(&json!({ "success": false, "error": e.to_string() })),
    }
}
