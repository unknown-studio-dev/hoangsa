use crate::helpers::{out, read_file};
use serde_json::{Value, json};
use std::path::Path;

/// All recognized roles and their purpose:
///
/// | Role         | Used by              | Nature                        |
/// |--------------|----------------------|-------------------------------|
/// | researcher   | research agents      | Read + summarize, no creation |
/// | designer     | menu (write specs)   | Architectural thinking        |
/// | planner      | prepare (DAG tasks)  | Structured decomposition      |
/// | orchestrator | cook/fix dispatch    | Routing, monitoring — light   |
/// | worker       | cook/fix implement   | Write code — varies by task   |
/// | reviewer     | cook semantic review | Read + compare against spec   |
/// | tester       | taste workflow       | Run commands, report — light  |
/// | committer    | plate workflow       | Git ops — very light          |
const ROLES: &[&str] = &[
    "researcher",
    "designer",
    "planner",
    "orchestrator",
    "worker",
    "reviewer",
    "tester",
    "committer",
];

/// Profile definitions: profile_name → [(role, model), ...]
fn get_profiles() -> Vec<(&'static str, Vec<(&'static str, &'static str)>)> {
    vec![
        (
            "quality",
            vec![
                ("researcher", "opus"),
                ("designer", "opus"),
                ("planner", "opus"),
                ("orchestrator", "opus"),
                ("worker", "opus"),
                ("reviewer", "opus"),
                ("tester", "sonnet"),
                ("committer", "sonnet"),
            ],
        ),
        (
            "balanced",
            vec![
                ("researcher", "sonnet"),
                ("designer", "opus"),
                ("planner", "sonnet"),
                ("orchestrator", "opus"),
                ("worker", "sonnet"),
                ("reviewer", "sonnet"),
                ("tester", "haiku"),
                ("committer", "haiku"),
            ],
        ),
        (
            "budget",
            vec![
                ("researcher", "haiku"),
                ("designer", "sonnet"),
                ("planner", "haiku"),
                ("orchestrator", "haiku"),
                ("worker", "haiku"),
                ("reviewer", "haiku"),
                ("tester", "haiku"),
                ("committer", "haiku"),
            ],
        ),
        (
            "minimal",
            vec![
                ("researcher", "haiku"),
                ("designer", "sonnet"),
                ("planner", "haiku"),
                ("orchestrator", "sonnet"),
                ("worker", "haiku"),
                ("reviewer", "haiku"),
                ("tester", "haiku"),
                ("committer", "haiku"),
            ],
        ),
    ]
}

fn resolve_from_profile(profile: &str, role: &str) -> &'static str {
    for (name, mappings) in get_profiles() {
        if name == profile {
            for (r, m) in &mappings {
                if *r == role {
                    return m;
                }
            }
        }
    }
    // Fallback: balanced profile
    for (name, mappings) in get_profiles() {
        if name == "balanced" {
            for (r, m) in &mappings {
                if *r == role {
                    return m;
                }
            }
        }
    }
    "sonnet"
}

/// `resolve-model <role>` — resolve which model to use for a given role.
///
/// Resolution order:
/// 1. `model_overrides.<role>` in config.json (per-role override)
/// 2. Profile-based mapping (from `profile` in config.json)
/// 3. Fallback: "sonnet"
pub fn resolve_model(role: &str, cwd: &str) {
    // Validate role
    if !ROLES.contains(&role) {
        out(&json!({
            "error": format!("Unknown role: '{}'. Known roles: {}", role, ROLES.join(", ")),
            "known_roles": ROLES,
        }));
        return;
    }

    let mut profile = "balanced".to_string();
    let mut model_overrides: Option<Value> = None;

    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    if let Some(content) = read_file(config_path.to_str().unwrap_or(""))
        && let Ok(cfg) = serde_json::from_str::<Value>(&content) {
            if let Some(p) = cfg.get("profile").and_then(|v| v.as_str()) {
                profile = p.to_string();
            }
            model_overrides = cfg.get("model_overrides").cloned();
        }

    // Check per-role override first
    let model = if let Some(overrides) = &model_overrides {
        if let Some(m) = overrides.get(role).and_then(|v| v.as_str()) {
            m.to_string()
        } else {
            resolve_from_profile(&profile, role).to_string()
        }
    } else {
        resolve_from_profile(&profile, role).to_string()
    };

    let source = if model_overrides.as_ref().and_then(|o| o.get(role)).is_some() {
        "override"
    } else {
        "profile"
    };

    out(&json!({
        "role": role,
        "model": model,
        "profile": profile,
        "source": source,
    }));
}

/// `resolve-model --all` — show all role→model mappings for current config.
pub fn resolve_all(cwd: &str) {
    let mut profile = "balanced".to_string();
    let mut model_overrides: Option<Value> = None;

    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    if let Some(content) = read_file(config_path.to_str().unwrap_or(""))
        && let Ok(cfg) = serde_json::from_str::<Value>(&content) {
            if let Some(p) = cfg.get("profile").and_then(|v| v.as_str()) {
                profile = p.to_string();
            }
            model_overrides = cfg.get("model_overrides").cloned();
        }

    let mut mappings = serde_json::Map::new();
    for role in ROLES {
        let model = if let Some(overrides) = &model_overrides {
            if let Some(m) = overrides.get(*role).and_then(|v| v.as_str()) {
                m.to_string()
            } else {
                resolve_from_profile(&profile, role).to_string()
            }
        } else {
            resolve_from_profile(&profile, role).to_string()
        };
        mappings.insert(role.to_string(), json!(model));
    }

    out(&json!({
        "profile": profile,
        "models": mappings,
        "overrides": model_overrides.unwrap_or(json!({})),
    }));
}
