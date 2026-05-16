//! Rule CRUD against `.hoangsa/rules.json`.
//!
//! Reuses `hoangsa_cli::cmd::rule` types (Rule, RulesConfig) so the UI and
//! the CLI never drift on the rule schema. Writes go through this module's
//! atomic write so concurrent edits from the CLI are detected via mtime.

use hoangsa_cli::cmd::rule::{default_rules, read_rules_config_pub, Rule, RulesConfig};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    #[error("rules.json missing: run `hoangsa-cli rule init <project_dir>` first")]
    NotInitialized,
    #[error("rule not found: {0}")]
    NotFound(String),
    #[error("rule already exists: {0}")]
    Duplicate(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("conflict: rules.json changed externally")]
    Conflict,
    #[error("invalid: {0}")]
    Invalid(String),
}

pub fn rules_path(project_dir: &Path) -> PathBuf {
    project_dir.join(".hoangsa/rules.json")
}

pub fn read(project_dir: &Path) -> Result<Option<RulesConfig>, RuleError> {
    read_rules_config_pub(&project_dir.to_string_lossy())
        .map_err(|e| RuleError::Invalid(e.to_string()))
}

pub fn mtime_ms(path: &Path) -> Option<i128> {
    let meta = fs::metadata(path).ok()?;
    let m = meta.modified().ok()?;
    let dur = m.duration_since(SystemTime::UNIX_EPOCH).ok()?;
    Some(dur.as_millis() as i128)
}

fn write_atomic(path: &Path, config: &RulesConfig) -> Result<(), RuleError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    let pretty = serde_json::to_string_pretty(config)
        .map_err(|e| RuleError::Invalid(format!("serialize: {e}")))?;
    tmp.write_all(pretty.as_bytes())?;
    tmp.write_all(b"\n")?;
    tmp.persist(path).map_err(|e| RuleError::Io(e.error))?;
    Ok(())
}

fn load_or_init(project_dir: &Path) -> Result<RulesConfig, RuleError> {
    match read(project_dir)? {
        Some(c) => Ok(c),
        None => Err(RuleError::NotInitialized),
    }
}

fn check_mtime(path: &Path, expected: Option<i128>) -> Result<(), RuleError> {
    let Some(expected) = expected else { return Ok(()) };
    let actual = mtime_ms(path).unwrap_or(0);
    if expected != actual {
        return Err(RuleError::Conflict);
    }
    Ok(())
}

pub fn add(project_dir: &Path, rule: Rule, expected_mtime: Option<i128>) -> Result<RulesConfig, RuleError> {
    let path = rules_path(project_dir);
    check_mtime(&path, expected_mtime)?;
    let mut config = load_or_init(project_dir)?;
    if config.rules.iter().any(|r| r.id == rule.id) {
        return Err(RuleError::Duplicate(rule.id));
    }
    config.rules.push(rule);
    write_atomic(&path, &config)?;
    Ok(config)
}

pub fn remove(project_dir: &Path, rule_id: &str, expected_mtime: Option<i128>) -> Result<RulesConfig, RuleError> {
    let path = rules_path(project_dir);
    check_mtime(&path, expected_mtime)?;
    let mut config = load_or_init(project_dir)?;
    let before = config.rules.len();
    config.rules.retain(|r| r.id != rule_id);
    if config.rules.len() == before {
        return Err(RuleError::NotFound(rule_id.to_string()));
    }
    write_atomic(&path, &config)?;
    Ok(config)
}

pub fn set_enabled(
    project_dir: &Path,
    rule_id: &str,
    enabled: bool,
    expected_mtime: Option<i128>,
) -> Result<RulesConfig, RuleError> {
    let path = rules_path(project_dir);
    check_mtime(&path, expected_mtime)?;
    let mut config = load_or_init(project_dir)?;
    let rule = config
        .rules
        .iter_mut()
        .find(|r| r.id == rule_id)
        .ok_or_else(|| RuleError::NotFound(rule_id.to_string()))?;
    rule.enabled = enabled;
    write_atomic(&path, &config)?;
    Ok(config)
}

pub fn replace(project_dir: &Path, rule: Rule, expected_mtime: Option<i128>) -> Result<RulesConfig, RuleError> {
    let path = rules_path(project_dir);
    check_mtime(&path, expected_mtime)?;
    let mut config = load_or_init(project_dir)?;
    let entry = config
        .rules
        .iter_mut()
        .find(|r| r.id == rule.id)
        .ok_or_else(|| RuleError::NotFound(rule.id.clone()))?;
    *entry = rule;
    write_atomic(&path, &config)?;
    Ok(config)
}

#[derive(Debug)]
pub struct SyncReport {
    pub added: Vec<String>,
    pub replaced: Vec<String>,
    pub user_kept: Vec<String>,
    pub config: RulesConfig,
}

/// Replace-by-id reconciliation: for every rule in `default_rules()`, replace
/// the entry of the same id in `rules.json`; user-added rules (ids not in
/// defaults) are preserved untouched. Locked decision Q7 from BRAINSTORM —
/// user customizations on default rules (disabled, custom enforcement) get
/// reset on upgrade. Acceptable tradeoff for simplicity.
pub fn sync_defaults(project_dir: &Path, expected_mtime: Option<i128>) -> Result<SyncReport, RuleError> {
    let path = rules_path(project_dir);
    check_mtime(&path, expected_mtime)?;
    let mut config = load_or_init(project_dir)?;

    let defaults = default_rules();
    let default_ids: std::collections::BTreeSet<String> =
        defaults.iter().map(|r| r.id.clone()).collect();

    let user_rules: Vec<Rule> = config
        .rules
        .iter()
        .filter(|r| !default_ids.contains(&r.id))
        .cloned()
        .collect();
    let user_kept: Vec<String> = user_rules.iter().map(|r| r.id.clone()).collect();

    let existing_default_ids: std::collections::BTreeSet<String> = config
        .rules
        .iter()
        .filter(|r| default_ids.contains(&r.id))
        .map(|r| r.id.clone())
        .collect();
    let mut added = Vec::new();
    let mut replaced = Vec::new();
    for d in &defaults {
        if existing_default_ids.contains(&d.id) {
            replaced.push(d.id.clone());
        } else {
            added.push(d.id.clone());
        }
    }

    config.rules = defaults;
    config.rules.extend(user_rules);
    write_atomic(&path, &config)?;

    Ok(SyncReport {
        added,
        replaced,
        user_kept,
        config,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hoangsa_cli::cmd::rule::{Condition, ConditionOp, Enforcement, RuleAction};

    fn sample_rule(id: &str) -> Rule {
        Rule {
            id: id.to_string(),
            name: format!("rule {id}"),
            enabled: true,
            enforcement: Enforcement::Prompt,
            matcher: "Edit".to_string(),
            conditions: vec![Condition {
                field: "file_path".to_string(),
                op: ConditionOp::Contains,
                value: "test/".to_string(),
            }],
            action: RuleAction::Warn,
            message: "test".to_string(),
            stateful: None,
        }
    }

    fn init_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let cfg = RulesConfig {
            version: "1.0".into(),
            rules: vec![],
        };
        write_atomic(&rules_path(dir.path()), &cfg).unwrap();
        dir
    }

    #[test]
    fn add_then_remove() {
        let dir = init_dir();
        let rule = sample_rule("foo");
        let cfg = add(dir.path(), rule.clone(), None).unwrap();
        assert_eq!(cfg.rules.len(), 1);

        let err = add(dir.path(), rule.clone(), None).unwrap_err();
        assert!(matches!(err, RuleError::Duplicate(_)));

        let cfg = remove(dir.path(), "foo", None).unwrap();
        assert!(cfg.rules.is_empty());
    }

    #[test]
    fn toggle_enabled() {
        let dir = init_dir();
        add(dir.path(), sample_rule("foo"), None).unwrap();
        let cfg = set_enabled(dir.path(), "foo", false, None).unwrap();
        assert!(!cfg.rules[0].enabled);
    }

    #[test]
    fn sync_replaces_defaults_keeps_user_rules() {
        let dir = init_dir();
        // Stale default + a user rule
        let mut stale_default = default_rules().into_iter().next().unwrap();
        stale_default.message = "STALE".to_string();
        let user = sample_rule("my-custom-rule");
        write_atomic(
            &rules_path(dir.path()),
            &RulesConfig {
                version: "1.0".into(),
                rules: vec![stale_default.clone(), user.clone()],
            },
        )
        .unwrap();

        let report = sync_defaults(dir.path(), None).unwrap();
        let cfg = report.config;
        assert!(report.user_kept.contains(&"my-custom-rule".to_string()));
        // Stale message was overwritten by current default
        let refreshed = cfg.rules.iter().find(|r| r.id == stale_default.id).unwrap();
        assert_ne!(refreshed.message, "STALE");
        // User rule preserved
        assert!(cfg.rules.iter().any(|r| r.id == "my-custom-rule"));
    }

    #[test]
    fn conflict_when_mtime_mismatch() {
        let dir = init_dir();
        let err = add(dir.path(), sample_rule("foo"), Some(1)).unwrap_err();
        assert!(matches!(err, RuleError::Conflict));
    }
}
