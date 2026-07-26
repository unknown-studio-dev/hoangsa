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
/// | simplify     | cook simplify pass   | Edits code — same risk as worker |
const ROLES: &[&str] = &[
    "researcher",
    "designer",
    "planner",
    "orchestrator",
    "worker",
    "reviewer",
    "tester",
    "committer",
    "simplify",
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
                ("simplify", "opus"),
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
                ("simplify", "sonnet"),
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
                ("simplify", "haiku"),
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
                ("simplify", "haiku"),
            ],
        ),
    ]
}

/// Is `profile` one of the routing profiles? Used to surface a config that
/// names something else — silently falling back to balanced while reporting
/// `source: "profile"` made a mis-set key indistinguishable from a working
/// one. `pref set … profile full` used to produce exactly that.
pub(crate) fn is_known_profile(profile: &str) -> bool {
    get_profiles().iter().any(|(name, _)| *name == profile)
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

/// Active harness from `.hoangsa/config.json` (`claude` | `codex` | `cowork`),
/// overridable with `HOANGSA_HARNESS` for global installs that have no
/// project config. Defaults to `claude`.
pub(crate) fn harness(cwd: &str) -> String {
    if let Ok(h) = std::env::var("HOANGSA_HARNESS")
        && !h.trim().is_empty()
    {
        return h.trim().to_lowercase();
    }
    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    if let Some(content) = read_file(config_path.to_str().unwrap_or(""))
        && let Ok(cfg) = serde_json::from_str::<Value>(&content)
        && let Some(h) = cfg.get("harness").and_then(|v| v.as_str())
    {
        return h.to_lowercase();
    }
    "claude".to_string()
}

/// Codex scales cost and quality with reasoning effort, not by swapping
/// models — so a profile tier maps onto effort there, and the model stays
/// whatever the Codex session is already running.
pub(crate) fn codex_effort_for_tier(tier: &str) -> &'static str {
    match tier {
        "fable" | "opus" => "high",
        "haiku" => "low",
        _ => "medium",
    }
}

/// The `model` key from `~/.codex/config.toml`, for reporting only —
/// HOANGSA never overrides the Codex session model. Reads the top-level
/// table, stopping at the first `[section]` so a `model` nested under a
/// profile can't be mistaken for the global one.
pub(crate) fn codex_session_model() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    let content = read_file(
        Path::new(&home)
            .join(".codex")
            .join("config.toml")
            .to_str()?,
    )?;
    for line in content.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            break;
        }
        if let Some(rest) = l.strip_prefix("model")
            && let Some(value) = rest.trim_start().strip_prefix('=')
        {
            let v = value.trim().trim_matches('"').trim_matches('\'');
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
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

    // On Codex the tier is not a model id. Reporting "sonnet" there would
    // be an instruction to spawn a model that doesn't exist, so the tier
    // becomes a reasoning effort and the model stays the session's own.
    if harness(cwd) == "codex" {
        out(&json!({
            "role": role,
            "harness": "codex",
            "tier": model,
            "reasoning_effort": codex_effort_for_tier(&model),
            "model": codex_session_model(),
            "model_source": "Codex session (~/.codex/config.toml) — HOANGSA does not override it",
            "profile": profile,
            "source": source,
        }));
        return;
    }

    let mut payload = json!({
        "role": role,
        "harness": harness(cwd),
        "model": model,
        "profile": profile,
        "source": source,
    });
    if let Some(w) = unknown_profile_warning(&profile)
        && let Some(o) = payload.as_object_mut()
    {
        o.insert("warning".to_string(), json!(w));
    }
    out(&payload);
}

/// Warning text when `config.json` names a profile that does not exist.
fn unknown_profile_warning(profile: &str) -> Option<String> {
    if is_known_profile(profile) {
        return None;
    }
    Some(format!(
        "unknown profile '{profile}' in .hoangsa/config.json — falling back to balanced. \
         Valid: quality, balanced, budget, minimal. The workflow preset \
         (full|balanced|minimal) belongs to `pref set . workflow_profile`, not this key."
    ))
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

    #[test]
    fn harness_defaults_to_claude_and_reads_config() {
        let dir = std::env::temp_dir().join("hoangsa-model-test-harness");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(dir.join(".hoangsa"));
        let cwd = dir.to_str().unwrap();
        assert_eq!(harness(cwd), "claude", "no config → claude");

        std::fs::write(dir.join(".hoangsa/config.json"), r#"{"harness":"CODEX"}"#).unwrap();
        assert_eq!(harness(cwd), "codex", "config value is lowercased");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every profile tier must land on a real Codex effort — an unmapped
    /// tier silently becoming "medium" would hide a routing bug.
    #[test]
    fn every_tier_maps_to_a_codex_effort() {
        assert_eq!(codex_effort_for_tier("fable"), "high");
        assert_eq!(codex_effort_for_tier("opus"), "high");
        assert_eq!(codex_effort_for_tier("sonnet"), "medium");
        assert_eq!(codex_effort_for_tier("haiku"), "low");
        for (_, mappings) in get_profiles() {
            for (role, tier) in mappings {
                assert!(
                    ["low", "medium", "high"].contains(&codex_effort_for_tier(tier)),
                    "{role} tier {tier} has no Codex effort"
                );
            }
        }
    }

    /// `model_reasoning_effort` starts with "model" — the top-level scan
    /// must not mistake it for the `model` key.
    #[test]
    fn codex_model_parse_ignores_lookalike_keys_and_sections() {
        let dir = std::env::temp_dir().join("hoangsa-model-test-codex-home");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".codex")).unwrap();
        std::fs::write(
            dir.join(".codex/config.toml"),
            "model_reasoning_effort = \"high\"\nmodel = \"gpt-5.5\"\n\n[profiles.other]\nmodel = \"wrong\"\n",
        )
        .unwrap();
        // SAFETY: single-threaded assertion window inside this test only.
        unsafe { std::env::set_var("HOME", &dir) };
        assert_eq!(codex_session_model().as_deref(), Some("gpt-5.5"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
