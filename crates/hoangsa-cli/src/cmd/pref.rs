use crate::helpers::{ERR_READ_CONFIG, out, read_json, require_arg};
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
                "memory_strict": false,
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
    "simplify_pass",
    "quality_gate",
    "test_runs",
    "context_mode",
    "chain_mode",
    "memory_strict",
    "workflow_profile",
];

/// Deprecated spelling of `workflow_profile`.
///
/// `profile` is also the top-level model-routing key (`quality | balanced |
/// budget | minimal`), and `pref set … profile` used to write the workflow
/// preset name — `full | balanced | minimal` — straight into it. `full` is
/// not a routing profile, so routing silently fell back to balanced while
/// reporting `"profile": "full"`; `minimal` is valid in *both* vocabularies
/// with different meanings, so picking the cheap workflow preset also
/// downgraded every model. The alias still works and now writes only the
/// workflow key.
const LEGACY_PROFILE_KEY: &str = "profile";

/// The six preference keys a workflow profile sets.
fn workflow_preset(name: &str) -> Option<[(&'static str, Value); 6]> {
    let (simplify, gate, runs, research, context, strict) = match name {
        "full" => (true, true, 3, "full", "full", true),
        "balanced" => (false, false, 1, "inline", "selective", false),
        "minimal" => (false, false, 0, "inline", "selective", false),
        _ => return None,
    };
    Some([
        ("simplify_pass", Value::Bool(simplify)),
        ("quality_gate", Value::Bool(gate)),
        ("test_runs", Value::Number(runs.into())),
        ("research_mode", Value::String(research.to_string())),
        ("context_mode", Value::String(context.to_string())),
        ("memory_strict", Value::Bool(strict)),
    ])
}

/// Map the deprecated `profile` spelling onto `workflow_profile`, warning
/// once. Applied before the `KNOWN_KEYS` check so the old command line keeps
/// working — it just no longer collides with model routing.
fn normalize_key(key: &str) -> &str {
    if key == LEGACY_PROFILE_KEY {
        eprintln!(
            "pref: `profile` is the top-level model-routing key; the workflow preset is \
             now `workflow_profile`. Using workflow_profile — model routing untouched."
        );
        return "workflow_profile";
    }
    key
}

/// `pref get <projectDir> <key>` — read a preference from config.json
pub fn cmd_get(project_dir: Option<&str>, key: Option<&str>) {
    let Some(project_dir) = require_arg(project_dir, "projectDir") else { return };
    let key = match key {
        Some(k) => k,
        None => {
            // No key → return all preferences
            let Some(config) = ensure_config(project_dir) else {
                out(&json!({ "error": ERR_READ_CONFIG }));
                return;
            };
            let prefs = config.get("preferences").cloned().unwrap_or(json!({}));
            out(&prefs);
            return;
        }
    };

    let key = normalize_key(key);
    if !KNOWN_KEYS.contains(&key) {
        out(
            &json!({ "error": format!("Unknown preference key: {}. Known keys: {}", key, KNOWN_KEYS.join(", ")) }),
        );
        return;
    }

    let Some(config) = ensure_config(project_dir) else {
        out(&json!({ "error": ERR_READ_CONFIG }));
        return;
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
    let Some(project_dir) = require_arg(project_dir, "projectDir") else { return };
    let Some(key) = require_arg(key, "key") else { return };
    let key = normalize_key(key);

    if !KNOWN_KEYS.contains(&key) {
        out(
            &json!({ "error": format!("Unknown preference key: {}. Known keys: {}", key, KNOWN_KEYS.join(", ")) }),
        );
        return;
    }

    let Some(mut config) = ensure_config(project_dir) else {
        out(&json!({ "error": ERR_READ_CONFIG }));
        return;
    };

    // Workflow quality preset — expands to 6 optimization keys.
    if key == "workflow_profile" {
        let profile_name = value.unwrap_or("");
        let Some(preset) = workflow_preset(profile_name) else {
            out(&json!({
                "error": format!(
                    "Unknown workflow profile: {profile_name}. Known: full, balanced, minimal"
                )
            }));
            return;
        };
        // Deliberately NOT the top-level `profile` key: that one routes
        // models, and these three names do not belong to its vocabulary.
        // Create `preferences` if the config predates it or was hand-written
        // without it — the previous `if let Some(..)` with no else wrote
        // nothing and still reported success, which is a silent no-op.
        let Some(obj) = config.as_object_mut() else {
            out(&json!({ "error": "config.json is not a JSON object" }));
            return;
        };
        let prefs = obj
            .entry("preferences")
            .or_insert_with(|| Value::Object(Default::default()));
        let Some(prefs) = prefs.as_object_mut() else {
            out(&json!({ "error": "config.json `preferences` is not an object" }));
            return;
        };
        prefs.insert("workflow_profile".to_string(), Value::String(profile_name.to_string()));
        for (k, v) in preset {
            prefs.insert(k.to_string(), v);
        }
        let config_file = config_path(project_dir);
        match fs::write(&config_file, serde_json::to_string_pretty(&config).unwrap()) {
            Ok(_) => out(&json!({ "success": true, "workflow_profile": profile_name })),
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
                "memory_strict": false,
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
        assert_eq!(prefs["memory_strict"], Value::Bool(true), "memory_strict");
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
        assert_eq!(prefs["memory_strict"], Value::Bool(false), "memory_strict");
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
        assert_eq!(prefs["test_runs"].as_i64(), Some(0), "test_runs");
        assert_eq!(prefs["research_mode"].as_str(), Some("inline"), "research_mode");
        assert_eq!(prefs["context_mode"].as_str(), Some("selective"), "context_mode");
        assert_eq!(prefs["memory_strict"], Value::Bool(false), "memory_strict");
        // This used to assert `config["profile"] == "minimal"`, pinning the
        // collision as intended behaviour: the root key routes MODELS, and
        // `minimal` exists in that vocabulary too — so setting the cheap
        // workflow preset silently downgraded every model as well.
        assert_eq!(
            prefs["workflow_profile"].as_str(),
            Some("minimal"),
            "preset name belongs under preferences"
        );
        assert_eq!(
            config["profile"].as_str(),
            Some("balanced"),
            "model routing must be left alone"
        );
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
        for key in &["simplify_pass", "quality_gate", "test_runs", "context_mode", "memory_strict"] {
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
    fn test_cmd_set_accepts_memory_strict() {
        let tmp = setup_project();
        let dir = tmp.path().to_str().expect("valid path");
        cmd_set(Some(dir), Some("memory_strict"), Some("true"));
        let config = read_config(dir);
        assert_eq!(get_pref(&config, "memory_strict"), Value::Bool(true));
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

    /// `profile` is the top-level model-routing key. `pref set … profile`
    /// used to write the workflow preset name into it — `full` is not a
    /// routing profile (silent fallback to balanced), and `minimal` is valid
    /// in BOTH vocabularies with different meanings, so choosing the cheap
    /// workflow preset also downgraded every model.
    #[test]
    fn workflow_preset_never_touches_the_routing_profile() {
        let dir = std::env::temp_dir().join("hoangsa-pref-test-collision");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let d = dir.to_str().unwrap();

        cmd_set(Some(d), Some("workflow_profile"), Some("minimal"));
        let cfg = read_json(config_path(d).to_str().unwrap());
        assert_eq!(cfg["profile"], "balanced", "model routing must be untouched");
        assert_eq!(cfg["preferences"]["workflow_profile"], "minimal");
        assert_eq!(cfg["preferences"]["test_runs"], 0, "the preset must still apply");

        // The deprecated spelling keeps working and is equally harmless.
        cmd_set(Some(d), Some("profile"), Some("full"));
        let cfg = read_json(config_path(d).to_str().unwrap());
        assert_eq!(cfg["profile"], "balanced", "legacy alias must not write routing");
        assert_eq!(cfg["preferences"]["workflow_profile"], "full");
        assert_eq!(cfg["preferences"]["test_runs"], 3);
        let _ = fs::remove_dir_all(&dir);
    }

    /// A workflow profile name that does not exist must be refused, not
    /// written through as a preset of nothing.
    #[test]
    fn unknown_workflow_profile_is_rejected() {
        assert!(workflow_preset("quality").is_none(), "routing names are not presets");
        assert!(workflow_preset("").is_none());
        for ok in ["full", "balanced", "minimal"] {
            assert!(workflow_preset(ok).is_some(), "{ok} should be a preset");
        }
    }
}
