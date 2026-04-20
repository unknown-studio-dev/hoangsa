use crate::helpers::{out, read_json};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// Resolve config.json path from project dir.
fn config_path(project_dir: &str) -> std::path::PathBuf {
    Path::new(project_dir).join(".hoangsa").join("config.json")
}

/// Ensure config.json exists with defaults, return parsed value.
fn ensure_config(project_dir: &str) -> Option<Value> {
    let config_file = config_path(project_dir);
    if !config_file.exists() {
        // Use config get to create defaults
        let config_dir = Path::new(project_dir).join(".hoangsa");
        fs::create_dir_all(&config_dir).ok()?;
        let defaults = json!({
            "profile": "balanced",
            "preferences": {
                "lang": null,
                "spec_lang": null,
                "tech_stack": [],
                "interaction_level": null,
                "auto_taste": null,
                "auto_plate": null,
                "auto_serve": null,
                "research_scope": null,
                "research_mode": null,
                "review_style": null,
                "simplify_pass": false,
                "quality_gate": false,
                "test_runs": 1,
                "context_mode": "selective",
                "thoth_strict": false,
            },
            "task_manager": {
                "provider": null,
                "mcp_server": null,
                "verified": false,
                "verified_at": null,
                "project_id": null,
                "default_list": null,
            },
        });
        fs::write(
            &config_file,
            serde_json::to_string_pretty(&defaults).unwrap(),
        )
        .ok()?;
        return Some(defaults);
    }

    let config = read_json(config_file.to_str().unwrap_or(""));
    if config.get("error").is_some() {
        return None;
    }
    Some(config)
}

/// Known preference keys.
const KNOWN_KEYS: &[&str] = &[
    "lang",
    "spec_lang",
    "tech_stack",
    "interaction_level",
    "auto_taste",
    "auto_plate",
    "auto_serve",
    "research_scope",
    "research_mode",
    "review_style",
    "auto_compact",
    "auto_compact_interval",
    "auto_compact_cooldown_secs",
    "simplify_pass",
    "quality_gate",
    "test_runs",
    "context_mode",
    "thoth_strict",
    "profile",
];

/// `pref get <projectDir> <key>` — read a preference from config.json
pub fn cmd_get(project_dir: Option<&str>, key: Option<&str>) {
    let project_dir = match project_dir {
        Some(d) => d,
        None => {
            out(&json!({ "error": "projectDir is required" }));
            return;
        }
    };
    let key = match key {
        Some(k) => k,
        None => {
            // No key → return all preferences
            let config = match ensure_config(project_dir) {
                Some(c) => c,
                None => {
                    out(&json!({ "error": "Cannot read config.json" }));
                    return;
                }
            };
            let prefs = config.get("preferences").cloned().unwrap_or(json!({}));
            out(&prefs);
            return;
        }
    };

    if !KNOWN_KEYS.contains(&key) {
        out(
            &json!({ "error": format!("Unknown preference key: {}. Known keys: {}", key, KNOWN_KEYS.join(", ")) }),
        );
        return;
    }

    let config = match ensure_config(project_dir) {
        Some(c) => c,
        None => {
            out(&json!({ "error": "Cannot read config.json" }));
            return;
        }
    };

    let value = config
        .get("preferences")
        .and_then(|p| p.get(key))
        .cloned()
        .unwrap_or(Value::Null);

    out(&json!({ "key": key, "value": value }));
}

/// `pref set <projectDir> <key> <value>` — write a preference to config.json
pub fn cmd_set(project_dir: Option<&str>, key: Option<&str>, value: Option<&str>) {
    let project_dir = match project_dir {
        Some(d) => d,
        None => {
            out(&json!({ "error": "projectDir is required" }));
            return;
        }
    };
    let key = match key {
        Some(k) => k,
        None => {
            out(&json!({ "error": "key is required" }));
            return;
        }
    };

    if !KNOWN_KEYS.contains(&key) {
        out(
            &json!({ "error": format!("Unknown preference key: {}. Known keys: {}", key, KNOWN_KEYS.join(", ")) }),
        );
        return;
    }

    let mut config = match ensure_config(project_dir) {
        Some(c) => c,
        None => {
            out(&json!({ "error": "Cannot read config.json" }));
            return;
        }
    };

    // Handle profile preset — expands to 6 optimization keys
    if key == "profile" {
        let profile_name = value.unwrap_or("");
        let preset: Option<[(&str, Value); 6]> = match profile_name {
            "full" => Some([
                ("simplify_pass", Value::Bool(true)),
                ("quality_gate", Value::Bool(true)),
                ("test_runs", Value::Number(3.into())),
                ("research_mode", Value::String("full".to_string())),
                ("context_mode", Value::String("full".to_string())),
                ("thoth_strict", Value::Bool(true)),
            ]),
            "balanced" => Some([
                ("simplify_pass", Value::Bool(false)),
                ("quality_gate", Value::Bool(false)),
                ("test_runs", Value::Number(1.into())),
                ("research_mode", Value::String("inline".to_string())),
                ("context_mode", Value::String("selective".to_string())),
                ("thoth_strict", Value::Bool(false)),
            ]),
            "minimal" => Some([
                ("simplify_pass", Value::Bool(false)),
                ("quality_gate", Value::Bool(false)),
                ("test_runs", Value::Number(1.into())),
                ("research_mode", Value::String("inline".to_string())),
                ("context_mode", Value::String("selective".to_string())),
                ("thoth_strict", Value::Bool(false)),
            ]),
            _ => None,
        };
        let preset = match preset {
            Some(p) => p,
            None => {
                out(&json!({ "error": format!("Unknown profile: {}. Known profiles: full, balanced, minimal", profile_name) }));
                return;
            }
        };
        if let Some(prefs) = config
            .as_object_mut()
            .and_then(|o| o.get_mut("preferences"))
            .and_then(|v| v.as_object_mut())
        {
            for (k, v) in preset {
                prefs.insert(k.to_string(), v);
            }
        }
        let config_file = config_path(project_dir);
        match fs::write(&config_file, serde_json::to_string_pretty(&config).unwrap()) {
            Ok(_) => {
                out(&json!({ "success": true, "profile": profile_name }));
            }
            Err(e) => out(&json!({ "success": false, "error": e.to_string() })),
        }
        return;
    }

    // Parse value with type coercion
    let parsed: Value = match value {
        Some("true") => Value::Bool(true),
        Some("false") => Value::Bool(false),
        Some("null") => Value::Null,
        Some(v) => {
            // Try parsing as JSON first (for arrays like tech_stack)
            if v.starts_with('[') || v.starts_with('{') {
                serde_json::from_str(v).unwrap_or(Value::String(v.to_string()))
            } else if let Ok(n) = v.parse::<i64>() {
                Value::Number(n.into())
            } else {
                Value::String(v.to_string())
            }
        }
        None => Value::Null,
    };

    // Update preferences in config
    if let Some(prefs) = config
        .as_object_mut()
        .and_then(|o| o.get_mut("preferences"))
        .and_then(|v| v.as_object_mut())
    {
        prefs.insert(key.to_string(), parsed.clone());
    } else {
        // preferences block missing — create it
        if let Some(obj) = config.as_object_mut() {
            let mut prefs = serde_json::Map::new();
            prefs.insert(key.to_string(), parsed.clone());
            obj.insert("preferences".to_string(), Value::Object(prefs));
        }
    }

    let config_file = config_path(project_dir);
    match fs::write(&config_file, serde_json::to_string_pretty(&config).unwrap()) {
        Ok(_) => out(&json!({ "success": true, "key": key, "value": parsed })),
        Err(e) => out(&json!({ "success": false, "error": e.to_string() })),
    }
}
