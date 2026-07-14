use crate::helpers::{ERR_READ_CONFIG, out, read_json, require_arg};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;

fn default_config() -> Value {
    json!({
        "profile": "balanced",
        "model_overrides": {},
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
            "auto_compact": true,
            "auto_compact_interval": 500,
            "auto_compact_cooldown_secs": 86400,
            "simplify_pass": false,
            "quality_gate": false,
            "test_runs": 1,
            "context_mode": "selective",
            "memory_strict": false,
        },
        "codebase": {
            "monorepo": false,
            "packages": [],
            "frameworks": [],
            "testing": {
                "frameworks": [],
                "config_files": [],
                "file_pattern": null,
            },
            "ci": null,
            "git_convention": null,
            "linters": [],
            "entry_points": [],
            "active_addons": [],
        },
        "task_manager": {
            "provider": null,
            "mcp_server": null,
            "verified": false,
            "verified_at": null,
            "project_id": null,
            "default_list": null,
        },
    })
}

/// `config get <projectDir>` — reads or creates default config.
pub fn cmd_get(project_dir: Option<&str>) {
    let Some(project_dir) = require_arg(project_dir, "projectDir") else { return };

    let config_dir = Path::new(project_dir).join(".hoangsa");
    let config_file = config_dir.join("config.json");

    if !config_file.exists() {
        let defaults = default_config();
        if let Err(e) = fs::create_dir_all(&config_dir) {
            out(&json!({ "error": format!("Cannot create config.json: {}", e) }));
            return;
        }
        if let Err(e) = fs::write(
            &config_file,
            serde_json::to_string_pretty(&defaults).unwrap(),
        ) {
            out(&json!({ "error": format!("Cannot create config.json: {}", e) }));
            return;
        }
        out(&defaults);
        return;
    }

    let config = read_json(config_file.to_str().unwrap_or(""));
    if config.get("error").is_some() {
        out(&json!({ "error": config["error"] }));
        return;
    }
    out(&config);
}

/// Ensure config file exists, creating defaults if missing. Returns the config.
fn ensure_config(project_dir: &str) -> Option<Value> {
    let config_dir = Path::new(project_dir).join(".hoangsa");
    let config_file = config_dir.join("config.json");

    if !config_file.exists() {
        let defaults = default_config();
        fs::create_dir_all(&config_dir).ok()?;
        fs::write(
            &config_file,
            serde_json::to_string_pretty(&defaults).unwrap(),
        )
        .ok()?;
    }

    let config = read_json(config_file.to_str().unwrap_or(""));
    if config.get("error").is_some() {
        return None;
    }
    Some(config)
}

/// `config set <projectDir> <jsonPatch>`
pub fn cmd_set(project_dir: Option<&str>, json_patch: Option<&str>) {
    let Some(project_dir) = require_arg(project_dir, "projectDir") else { return };
    let json_patch = match json_patch {
        Some(p) => p,
        None => {
            out(&json!({ "error": "jsonPatch is required" }));
            return;
        }
    };

    // Ensure config exists (creates defaults silently — no extra JSON output)
    let Some(config) = ensure_config(project_dir) else {
        out(&json!({ "error": ERR_READ_CONFIG }));
        return;
    };

    let patch: Value = match serde_json::from_str(json_patch) {
        Ok(v) => v,
        Err(e) => {
            out(&json!({ "error": format!("Invalid JSON patch: {}", e) }));
            return;
        }
    };

    // Shallow merge
    let mut updated = config.as_object().cloned().unwrap_or_default();
    if let Some(patch_obj) = patch.as_object() {
        for (k, v) in patch_obj {
            updated.insert(k.clone(), v.clone());
        }
    }

    // Deep merge task_manager if patch includes it
    if let (Some(patch_tm), Some(config_tm)) = (
        patch.get("task_manager").and_then(|v| v.as_object()),
        config.get("task_manager").and_then(|v| v.as_object()),
    ) {
        let mut merged: Map<String, Value> = config_tm.clone();
        for (k, v) in patch_tm {
            merged.insert(k.clone(), v.clone());
        }
        updated.insert("task_manager".to_string(), Value::Object(merged));
    }

    // Deep merge nested objects: preferences, model_overrides, codebase
    for key in &["preferences", "model_overrides", "codebase"] {
        if let (Some(patch_obj), Some(config_obj)) = (
            patch.get(*key).and_then(|v| v.as_object()),
            config.get(*key).and_then(|v| v.as_object()),
        ) {
            let mut merged: Map<String, Value> = config_obj.clone();
            for (k, v) in patch_obj {
                merged.insert(k.clone(), v.clone());
            }
            updated.insert(key.to_string(), Value::Object(merged));
        }
    }

    let updated_val = Value::Object(updated);
    let config_file = Path::new(project_dir).join(".hoangsa").join("config.json");
    match fs::write(
        &config_file,
        serde_json::to_string_pretty(&updated_val).unwrap(),
    ) {
        Ok(_) => out(&json!({ "success": true, "config": updated_val })),
        Err(e) => out(&json!({ "success": false, "error": e.to_string() })),
    }
}
