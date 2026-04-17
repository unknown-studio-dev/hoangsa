use crate::helpers::{out, parse_frontmatter, read_json};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// Resolve HOANGSA_ROOT — find the installed addons directory.
/// Checks: env HOANGSA_ROOT → .claude/hoangsa from project dir → ~/.claude/hoangsa
fn resolve_hoangsa_root(project_dir: &str) -> Option<String> {
    if let Ok(root) = std::env::var("HOANGSA_ROOT") {
        let addons = Path::new(&root).join("workflows/worker-rules/addons");
        if addons.is_dir() {
            return Some(root);
        }
    }

    let local = Path::new(project_dir).join(".claude/hoangsa");
    if local.join("workflows/worker-rules/addons").is_dir() {
        return Some(local.to_string_lossy().to_string());
    }

    if let Ok(home) = std::env::var("HOME") {
        let global = Path::new(&home).join(".claude/hoangsa");
        if global.join("workflows/worker-rules/addons").is_dir() {
            return Some(global.to_string_lossy().to_string());
        }
    }

    None
}

/// Scan $HOANGSA_ROOT/workflows/worker-rules/addons/*.md, parse frontmatter.
/// Returns Vec of { name, frameworks, test_frameworks } objects.
fn scan_available_addons(hoangsa_root: &str) -> Vec<Value> {
    let addons_dir = Path::new(hoangsa_root).join("workflows/worker-rules/addons");
    let mut result = Vec::new();

    let entries = match fs::read_dir(&addons_dir) {
        Ok(e) => e,
        Err(_) => return result,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let fm = match parse_frontmatter(&content) {
            Some(f) => f,
            None => continue,
        };
        let name = match fm.get("name") {
            Some(n) => n.clone(),
            None => continue,
        };
        let frameworks: Value = fm
            .get("frameworks")
            .and_then(|f| serde_json::from_str(f).ok())
            .unwrap_or(json!([]));
        let test_frameworks: Value = fm
            .get("test_frameworks")
            .and_then(|f| serde_json::from_str(f).ok())
            .unwrap_or(json!([]));

        result.push(json!({
            "name": name,
            "frameworks": frameworks,
            "test_frameworks": test_frameworks,
        }));
    }

    result.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or("")
            .cmp(b["name"].as_str().unwrap_or(""))
    });
    result
}

/// Read active_addons from config.json.
fn get_active_addons(project_dir: &str) -> Vec<String> {
    let config_file = Path::new(project_dir).join(".hoangsa/config.json");
    let config = read_json(config_file.to_str().unwrap_or(""));
    config
        .get("codebase")
        .and_then(|c| c.get("active_addons"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// Write active_addons to config.json (preserving all other fields).
fn set_active_addons(project_dir: &str, addons: &[String]) -> bool {
    let config_file = Path::new(project_dir).join(".hoangsa/config.json");
    let mut config = read_json(config_file.to_str().unwrap_or(""));
    if config.get("error").is_some() {
        return false;
    }

    let addons_val: Vec<Value> = addons.iter().map(|s| Value::String(s.clone())).collect();

    if let Some(codebase) = config.get_mut("codebase").and_then(|c| c.as_object_mut()) {
        codebase.insert("active_addons".to_string(), Value::Array(addons_val));
    } else if let Some(obj) = config.as_object_mut() {
        obj.insert(
            "codebase".to_string(),
            json!({ "active_addons": addons }),
        );
    }

    fs::write(
        &config_file,
        serde_json::to_string_pretty(&config).unwrap(),
    )
    .is_ok()
}

/// Copy addon .md file from HOANGSA_ROOT to project-level .hoangsa/worker-rules/addons/.
fn copy_addon_file(hoangsa_root: &str, addon_name: &str, project_dir: &str) -> bool {
    let source = Path::new(hoangsa_root)
        .join("workflows/worker-rules/addons")
        .join(format!("{}.md", addon_name));
    if !source.exists() {
        return false;
    }
    let target_dir = Path::new(project_dir).join(".hoangsa/worker-rules/addons");
    if fs::create_dir_all(&target_dir).is_err() {
        return false;
    }
    let target = target_dir.join(format!("{}.md", addon_name));
    fs::copy(&source, &target).is_ok()
}

/// Remove addon .md file from project-level .hoangsa/worker-rules/addons/.
fn remove_addon_file(addon_name: &str, project_dir: &str) -> bool {
    let target = Path::new(project_dir)
        .join(".hoangsa/worker-rules/addons")
        .join(format!("{}.md", addon_name));
    if !target.exists() {
        return false;
    }
    fs::remove_file(&target).is_ok()
}

/// Regenerate .hoangsa/worker-rules.md with updated addon list.
fn sync_worker_rules(project_dir: &str, active_addons: &[Value]) -> bool {
    let project_name = Path::new(project_dir)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");

    let mut addon_lines = String::new();
    for addon in active_addons {
        let name = addon["name"].as_str().unwrap_or("?");
        let frameworks = addon["frameworks"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();
        addon_lines.push_str(&format!("- **{}** — matches: {}\n", name, frameworks));
    }

    let content = format!(
        "# Worker Rules — {project_name}\n\
         \n\
         Project-level worker rules. Extends the HOANGSA base worker-rules with addons matched to this project's stack.\n\
         \n\
         ## Detected addons\n\
         \n\
         The following addons will be auto-loaded at runtime based on this project's tech stack:\n\
         \n\
         {addon_lines}\
         _(addon matching: `frameworks` field in each addon's frontmatter vs `tech_stack` + detected frameworks in config.json)_\n\
         \n\
         ## Project overrides\n\
         \n\
         Add any project-specific rule overrides below. These take priority over base worker-rules and addons.\n"
    );

    let target = Path::new(project_dir).join(".hoangsa/worker-rules.md");
    fs::write(&target, content).is_ok()
}

/// `addon list <projectDir>` — show all available addons with active status.
pub fn cmd_list(project_dir: Option<&str>) {
    let project_dir = match project_dir {
        Some(d) => d,
        None => {
            out(&json!({ "error": "projectDir is required" }));
            return;
        }
    };

    let hoangsa_root = match resolve_hoangsa_root(project_dir) {
        Some(r) => r,
        None => {
            out(&json!({ "error": "Cannot find HOANGSA installation (no addons directory found)" }));
            return;
        }
    };

    let available = scan_available_addons(&hoangsa_root);
    let active = get_active_addons(project_dir);

    let available_with_status: Vec<Value> = available
        .iter()
        .map(|addon| {
            let name = addon["name"].as_str().unwrap_or("");
            let mut a = addon.clone();
            a.as_object_mut()
                .unwrap()
                .insert("active".to_string(), Value::Bool(active.contains(&name.to_string())));
            a
        })
        .collect();

    out(&json!({
        "available": available_with_status,
        "active_addons": active,
    }));
}

/// `addon add <projectDir> <json_array>` — enable addons by name.
pub fn cmd_add(project_dir: Option<&str>, addons_json: Option<&str>) {
    let project_dir = match project_dir {
        Some(d) => d,
        None => {
            out(&json!({ "error": "projectDir is required" }));
            return;
        }
    };
    let addons_json = match addons_json {
        Some(j) => j,
        None => {
            out(&json!({ "error": "addons JSON array is required, e.g. '[\"react\",\"vue\"]'" }));
            return;
        }
    };

    let requested: Vec<String> = match serde_json::from_str(addons_json) {
        Ok(v) => v,
        Err(e) => {
            out(&json!({ "error": format!("Invalid JSON array: {}", e) }));
            return;
        }
    };

    let hoangsa_root = match resolve_hoangsa_root(project_dir) {
        Some(r) => r,
        None => {
            out(&json!({ "error": "Cannot find HOANGSA installation" }));
            return;
        }
    };

    let available = scan_available_addons(&hoangsa_root);
    let available_names: Vec<String> = available
        .iter()
        .filter_map(|a| a["name"].as_str().map(String::from))
        .collect();

    // Validate all requested addons exist
    for name in &requested {
        if !available_names.contains(name) {
            out(&json!({ "error": format!("Addon not found: {}. Available: {}", name, available_names.join(", ")) }));
            return;
        }
    }

    let mut active = get_active_addons(project_dir);

    // Add requested addons (dedup)
    for name in &requested {
        if !active.contains(name) {
            active.push(name.clone());
        }
        copy_addon_file(&hoangsa_root, name, project_dir);
    }

    active.sort();

    if !set_active_addons(project_dir, &active) {
        out(&json!({ "error": "Failed to update config.json" }));
        return;
    }

    // Get metadata for active addons to sync worker-rules
    let active_metadata: Vec<Value> = available
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .map(|n| active.contains(&n.to_string()))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    sync_worker_rules(project_dir, &active_metadata);

    out(&json!({
        "success": true,
        "active_addons": active,
        "synced": ["config.json", "worker-rules.md"],
    }));
}

/// `addon remove <projectDir> <json_array>` — disable addons by name.
pub fn cmd_remove(project_dir: Option<&str>, addons_json: Option<&str>) {
    let project_dir = match project_dir {
        Some(d) => d,
        None => {
            out(&json!({ "error": "projectDir is required" }));
            return;
        }
    };
    let addons_json = match addons_json {
        Some(j) => j,
        None => {
            out(&json!({ "error": "addons JSON array is required, e.g. '[\"vue\"]'" }));
            return;
        }
    };

    let requested: Vec<String> = match serde_json::from_str(addons_json) {
        Ok(v) => v,
        Err(e) => {
            out(&json!({ "error": format!("Invalid JSON array: {}", e) }));
            return;
        }
    };

    let hoangsa_root = match resolve_hoangsa_root(project_dir) {
        Some(r) => r,
        None => {
            out(&json!({ "error": "Cannot find HOANGSA installation" }));
            return;
        }
    };

    let mut active = get_active_addons(project_dir);

    for name in &requested {
        active.retain(|a| a != name);
        remove_addon_file(name, project_dir);
    }

    if !set_active_addons(project_dir, &active) {
        out(&json!({ "error": "Failed to update config.json" }));
        return;
    }

    let available = scan_available_addons(&hoangsa_root);
    let active_metadata: Vec<Value> = available
        .iter()
        .filter(|a| {
            a["name"]
                .as_str()
                .map(|n| active.contains(&n.to_string()))
                .unwrap_or(false)
        })
        .cloned()
        .collect();
    sync_worker_rules(project_dir, &active_metadata);

    out(&json!({
        "success": true,
        "active_addons": active,
        "synced": ["config.json", "worker-rules.md"],
    }));
}
