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

/// Core resolution (override > profile > balanced fallback), reusable without
/// printing — `envelope` stamps every worker prompt with this so orchestrators
/// can't forget to honor config routing.
pub(crate) fn resolve_model_parts(role: &str, cwd: &str) -> (String, String, &'static str) {
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

    let (model, source) = match model_overrides
        .as_ref()
        .and_then(|o| o.get(role))
        .and_then(|v| v.as_str())
    {
        Some(m) => (m.to_string(), "override"),
        None => (resolve_from_profile(&profile, role).to_string(), "profile"),
    };
    (model, profile, source)
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

    let (model, profile, source) = resolve_model_parts(role, cwd);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_parts_defaults_to_balanced_profile() {
        let dir = std::env::temp_dir().join("hoangsa-model-test-empty");
        let _ = std::fs::create_dir_all(&dir);
        let (model, profile, source) = resolve_model_parts("worker", dir.to_str().unwrap());
        assert_eq!((model.as_str(), profile.as_str(), source), ("sonnet", "balanced", "profile"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_parts_override_beats_profile() {
        let dir = std::env::temp_dir().join("hoangsa-model-test-ovr");
        let _ = std::fs::create_dir_all(dir.join(".hoangsa"));
        std::fs::write(
            dir.join(".hoangsa/config.json"),
            r#"{"profile":"quality","model_overrides":{"worker":"haiku"}}"#,
        )
        .unwrap();
        let (model, profile, source) = resolve_model_parts("worker", dir.to_str().unwrap());
        assert_eq!((model.as_str(), profile.as_str(), source), ("haiku", "quality", "override"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
