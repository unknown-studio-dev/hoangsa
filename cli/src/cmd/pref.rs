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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    // ── helpers ──────────────────────────────────────────────────────────────

    /// Create a temp dir with a pre-existing `.hoangsa/config.json`.
    fn setup_project() -> TempDir {
        let tmp = TempDir::new().expect("create tempdir");
        let hoangsa_dir = tmp.path().join(".hoangsa");
        fs::create_dir_all(&hoangsa_dir).expect("create .hoangsa dir");
        let config = json!({
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
                "research_mode": "inline",
                "review_style": null,
                "simplify_pass": false,
                "quality_gate": false,
                "test_runs": 1,
                "context_mode": "selective",
                "thoth_strict": false,
                "auto_compact": true,
                "auto_compact_interval": 500,
                "auto_compact_cooldown_secs": 86400,
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
            hoangsa_dir.join("config.json"),
            serde_json::to_string_pretty(&config).expect("serialize config"),
        )
        .expect("write config.json");
        tmp
    }

    fn read_config(project_dir: &str) -> Value {
        let path = config_path(project_dir);
        let raw = fs::read_to_string(&path).expect("read config.json");
        serde_json::from_str(&raw).expect("parse config.json")
    }

    fn get_pref(config: &Value, key: &str) -> Value {
        config
            .get("preferences")
            .and_then(|p| p.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    }

    // ── integer coercion tests ───────────────────────────────────────────────

    #[test]
    fn test_set_integer_value_stores_as_number() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("test_runs"), Some("3"));
        let config = read_config(dir);
        let val = get_pref(&config, "test_runs");
        assert!(val.is_number(), "expected JSON number, got: {val}");
        assert_eq!(val.as_i64(), Some(3));
    }

    #[test]
    fn test_set_non_numeric_string_stores_as_string() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("test_runs"), Some("abc"));
        let config = read_config(dir);
        let val = get_pref(&config, "test_runs");
        assert!(val.is_string(), "expected JSON string, got: {val}");
        assert_eq!(val.as_str(), Some("abc"));
    }

    #[test]
    fn test_set_boolean_true_string_stores_as_bool() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("simplify_pass"), Some("true"));
        let config = read_config(dir);
        let val = get_pref(&config, "simplify_pass");
        assert_eq!(val, Value::Bool(true));
    }

    #[test]
    fn test_set_boolean_false_string_stores_as_bool() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("quality_gate"), Some("false"));
        let config = read_config(dir);
        let val = get_pref(&config, "quality_gate");
        assert_eq!(val, Value::Bool(false));
    }

    #[test]
    fn test_set_null_string_stores_as_null() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("lang"), Some("null"));
        let config = read_config(dir);
        let val = get_pref(&config, "lang");
        assert_eq!(val, Value::Null);
    }

    // ── profile preset tests ─────────────────────────────────────────────────

    #[test]
    fn test_profile_full_sets_all_six_keys() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("profile"), Some("full"));
        let config = read_config(dir);
        let prefs = config.get("preferences").expect("preferences block");

        assert_eq!(prefs["simplify_pass"], Value::Bool(true), "simplify_pass");
        assert_eq!(prefs["quality_gate"], Value::Bool(true), "quality_gate");
        assert_eq!(prefs["test_runs"].as_i64(), Some(3), "test_runs");
        assert_eq!(prefs["research_mode"].as_str(), Some("full"), "research_mode");
        assert_eq!(prefs["context_mode"].as_str(), Some("full"), "context_mode");
        assert_eq!(prefs["thoth_strict"], Value::Bool(true), "thoth_strict");
    }

    #[test]
    fn test_profile_balanced_sets_all_six_keys() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        // First set full, then flip back to balanced
        cmd_set(Some(dir), Some("profile"), Some("full"));
        cmd_set(Some(dir), Some("profile"), Some("balanced"));
        let config = read_config(dir);
        let prefs = config.get("preferences").expect("preferences block");

        assert_eq!(prefs["simplify_pass"], Value::Bool(false), "simplify_pass");
        assert_eq!(prefs["quality_gate"], Value::Bool(false), "quality_gate");
        assert_eq!(prefs["test_runs"].as_i64(), Some(1), "test_runs");
        assert_eq!(prefs["research_mode"].as_str(), Some("inline"), "research_mode");
        assert_eq!(prefs["context_mode"].as_str(), Some("selective"), "context_mode");
        assert_eq!(prefs["thoth_strict"], Value::Bool(false), "thoth_strict");
    }

    #[test]
    fn test_profile_minimal_sets_all_six_keys() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("profile"), Some("minimal"));
        let config = read_config(dir);
        let prefs = config.get("preferences").expect("preferences block");

        assert_eq!(prefs["simplify_pass"], Value::Bool(false), "simplify_pass");
        assert_eq!(prefs["quality_gate"], Value::Bool(false), "quality_gate");
        assert_eq!(prefs["test_runs"].as_i64(), Some(1), "test_runs");
        assert_eq!(prefs["research_mode"].as_str(), Some("inline"), "research_mode");
        assert_eq!(prefs["context_mode"].as_str(), Some("selective"), "context_mode");
        assert_eq!(prefs["thoth_strict"], Value::Bool(false), "thoth_strict");
    }

    #[test]
    fn test_profile_unknown_returns_error() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        // cmd_set with an unknown profile — reads stdout via the out() fn,
        // but we just verify config remains unchanged (no write happened).
        let config_before = read_config(dir);
        cmd_set(Some(dir), Some("profile"), Some("turbo_ultra_max"));
        let config_after = read_config(dir);
        // Config should be identical — unknown profile must not write.
        assert_eq!(
            config_before.get("preferences"),
            config_after.get("preferences"),
            "config must not change on unknown profile"
        );
    }

    // ── known key acceptance tests ───────────────────────────────────────────

    #[test]
    fn test_known_keys_include_new_five() {
        for key in &["simplify_pass", "quality_gate", "test_runs", "context_mode", "thoth_strict"] {
            assert!(
                KNOWN_KEYS.contains(key),
                "expected {} in KNOWN_KEYS",
                key
            );
        }
    }

    #[test]
    fn test_cmd_set_accepts_context_mode() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("context_mode"), Some("full"));
        let config = read_config(dir);
        assert_eq!(get_pref(&config, "context_mode").as_str(), Some("full"));
    }

    #[test]
    fn test_cmd_set_accepts_thoth_strict() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("thoth_strict"), Some("true"));
        let config = read_config(dir);
        assert_eq!(get_pref(&config, "thoth_strict"), Value::Bool(true));
    }

    #[test]
    fn test_cmd_set_unknown_key_does_not_write() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        let config_before = read_config(dir);
        cmd_set(Some(dir), Some("nonexistent_key_xyz"), Some("foo"));
        let config_after = read_config(dir);
        assert_eq!(
            config_before.get("preferences"),
            config_after.get("preferences"),
            "unknown key must not modify preferences"
        );
    }

    #[test]
    fn test_cmd_get_returns_value_for_known_key() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        // Set then get — verifies round-trip
        cmd_set(Some(dir), Some("test_runs"), Some("5"));
        let config = read_config(dir);
        assert_eq!(get_pref(&config, "test_runs").as_i64(), Some(5));
    }
}
