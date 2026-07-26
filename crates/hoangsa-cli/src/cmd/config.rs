use crate::helpers::{ERR_READ_CONFIG, out, read_json, require_arg};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::Path;

fn default_config() -> Value {
    json!({
        "profile": "balanced",
        "harness": "claude",
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

/// Persist the active harness into `<project_dir>/.hoangsa/config.json`,
/// leaving every other key alone.
///
/// `install --harness codex` used to configure the Codex tree without
/// recording the choice anywhere `resolve-model` could read it, so model
/// routing kept resolving Claude tiers for a Codex session. Returns
/// `Ok(true)` when the file was written, `Ok(false)` when it already said
/// the same thing.
pub fn set_harness(project_dir: &Path, harness: &str) -> Result<bool, String> {
    let dir = project_dir.to_str().ok_or("project path is not UTF-8")?;
    let config = ensure_config(dir).ok_or(ERR_READ_CONFIG)?;
    if config.get("harness").and_then(|v| v.as_str()) == Some(harness) {
        return Ok(false);
    }

    let mut updated = config.as_object().cloned().unwrap_or_default();
    updated.insert("harness".to_string(), json!(harness));
    let config_file = project_dir.join(".hoangsa").join("config.json");
    fs::write(
        &config_file,
        serde_json::to_string_pretty(&Value::Object(updated)).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(true)
}

/// Move a 0.5.0 workflow-preset name out of the model-routing `profile` key.
///
/// Until 0.6.0 the documented command `pref set . profile <full|balanced|
/// minimal>` wrote a *workflow preset* name into the *model routing* key.
/// `full` is not a routing profile (routing quietly fell back to balanced),
/// and `minimal` is valid in both vocabularies with different meanings — so an
/// upgraded config silently routes every role to haiku.
///
/// `full` is unambiguous. `minimal` is fingerprinted by `test_runs == 0`,
/// which only the workflow preset ever writes; a deliberate `minimal` routing
/// choice was impossible before 0.6.0 anyway — the wizard never offered it.
/// Returns the preset name when something was migrated.
pub fn migrate_legacy_profile(project_dir: &Path) -> Option<String> {
    let config_file = project_dir.join(".hoangsa").join("config.json");
    if !config_file.exists() {
        return None;
    }
    let mut config = read_json(config_file.to_str()?);
    let profile = config.get("profile").and_then(|v| v.as_str())?.to_string();
    if config
        .get("preferences")
        .and_then(|p| p.get("workflow_profile"))
        .is_some()
    {
        return None; // already migrated
    }
    let test_runs = config
        .get("preferences")
        .and_then(|p| p.get("test_runs"))
        .and_then(|v| v.as_i64());
    let looks_like_preset = profile == "full" || (profile == "minimal" && test_runs == Some(0));
    if !looks_like_preset {
        return None;
    }

    let obj = config.as_object_mut()?;
    obj.insert("profile".to_string(), json!("balanced"));
    let prefs = obj
        .entry("preferences")
        .or_insert_with(|| Value::Object(Default::default()));
    prefs
        .as_object_mut()?
        .insert("workflow_profile".to_string(), json!(profile.clone()));

    fs::write(&config_file, serde_json::to_string_pretty(&config).ok()?).ok()?;
    Some(profile)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("hoangsa-config-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("tmp dir");
        dir
    }

    #[test]
    fn set_harness_creates_config_and_is_idempotent() {
        let dir = tmp("harness-new");
        assert_eq!(set_harness(&dir, "codex"), Ok(true), "first write");
        let cfg = read_json(dir.join(".hoangsa/config.json").to_str().unwrap());
        assert_eq!(cfg["harness"], "codex");
        assert_eq!(cfg["profile"], "balanced", "defaults still seeded");

        assert_eq!(set_harness(&dir, "codex"), Ok(false), "no rewrite when unchanged");
        assert_eq!(set_harness(&dir, "claude"), Ok(true), "change is written");
        let cfg = read_json(dir.join(".hoangsa/config.json").to_str().unwrap());
        assert_eq!(cfg["harness"], "claude");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Recording the harness must not be a way to lose the user's routing
    /// config — install calls this on an already-configured project.
    #[test]
    fn set_harness_preserves_other_keys() {
        let dir = tmp("harness-preserve");
        fs::create_dir_all(dir.join(".hoangsa")).unwrap();
        fs::write(
            dir.join(".hoangsa/config.json"),
            r#"{"profile":"quality","model_overrides":{"designer":"fable"},"preferences":{"lang":"vi"}}"#,
        )
        .unwrap();

        assert_eq!(set_harness(&dir, "codex"), Ok(true));
        let cfg = read_json(dir.join(".hoangsa/config.json").to_str().unwrap());
        assert_eq!(cfg["harness"], "codex");
        assert_eq!(cfg["profile"], "quality");
        assert_eq!(cfg["model_overrides"]["designer"], "fable");
        assert_eq!(cfg["preferences"]["lang"], "vi");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn default_config_declares_the_harness_key() {
        assert_eq!(default_config()["harness"], "claude");
    }

    /// A 0.5.0 config carries the workflow preset name in the model-routing
    /// key. `full` is unambiguous; `minimal` is fingerprinted by
    /// `test_runs == 0`, which only the preset writes.
    #[test]
    fn legacy_profile_migrates_only_when_it_is_a_preset() {
        let cases = [
            // (profile, test_runs, should_migrate)
            ("full", 1, true),
            ("minimal", 0, true),
            ("minimal", 1, false),   // a deliberate minimal ROUTING choice
            ("balanced", 0, false),  // same name in both, harmless either way
            ("quality", 0, false),
            ("budget", 0, false),
        ];
        for (profile, test_runs, should) in cases {
            let dir = tmp(&format!("migrate-{profile}-{test_runs}"));
            fs::create_dir_all(dir.join(".hoangsa")).unwrap();
            fs::write(
                dir.join(".hoangsa/config.json"),
                format!(r#"{{"profile":"{profile}","preferences":{{"test_runs":{test_runs}}}}}"#),
            )
            .unwrap();

            let migrated = migrate_legacy_profile(&dir);
            let cfg = read_json(dir.join(".hoangsa/config.json").to_str().unwrap());
            if should {
                assert_eq!(migrated.as_deref(), Some(profile), "{profile}/{test_runs}");
                assert_eq!(cfg["profile"], "balanced", "routing must be restored");
                assert_eq!(cfg["preferences"]["workflow_profile"], profile);
                // Idempotent: a second pass must not touch an already-migrated file.
                assert_eq!(migrate_legacy_profile(&dir), None, "second pass");
            } else {
                assert_eq!(migrated, None, "{profile}/{test_runs} must be left alone");
                assert_eq!(cfg["profile"], profile);
            }
            let _ = fs::remove_dir_all(&dir);
        }
    }
}
