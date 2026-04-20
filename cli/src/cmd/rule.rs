use crate::helpers::out;
use glob::Pattern;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesConfig {
    pub version: String,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub matcher: String,
    pub conditions: Vec<Condition>,
    pub action: RuleAction,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,
    pub op: ConditionOp,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConditionOp {
    Glob,
    Regex,
    Contains,
    NotContains,
    StartsWith,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleAction {
    Block,
    Warn,
}

pub fn evaluate_condition(condition: &Condition, field_value: &str) -> bool {
    match condition.op {
        ConditionOp::Glob => Pattern::new(&condition.value)
            .map(|p| p.matches(field_value))
            .unwrap_or(false),
        ConditionOp::Regex => Regex::new(&condition.value)
            .map(|r| r.is_match(field_value))
            .unwrap_or(false),
        ConditionOp::Contains => field_value.contains(condition.value.as_str()),
        ConditionOp::NotContains => !field_value.contains(condition.value.as_str()),
        ConditionOp::StartsWith => field_value.starts_with(condition.value.as_str()),
    }
}

pub fn evaluate_rule_conditions(rule: &Rule, tool_input: &serde_json::Value) -> bool {
    for condition in &rule.conditions {
        let field_value = match tool_input.get(&condition.field).and_then(|v| v.as_str()) {
            Some(v) => v,
            None => return false,
        };
        if !evaluate_condition(condition, field_value) {
            return false;
        }
    }
    true
}

fn rules_path(project_dir: &str) -> std::path::PathBuf {
    Path::new(project_dir).join(".hoangsa/rules.json")
}

fn read_rules_config(project_dir: &str) -> Result<Option<RulesConfig>, Box<dyn std::error::Error>> {
    let path = rules_path(project_dir);
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(&path)?;
    let config: RulesConfig = serde_json::from_str(&content)?;
    Ok(Some(config))
}

fn write_rules_config(project_dir: &str, config: &RulesConfig) -> Result<(), Box<dyn std::error::Error>> {
    let hoangsa_dir = Path::new(project_dir).join(".hoangsa");
    if !hoangsa_dir.exists() {
        fs::create_dir_all(&hoangsa_dir)?;
    }
    let path = rules_path(project_dir);
    fs::write(&path, serde_json::to_string_pretty(config)?)?;
    Ok(())
}

pub fn cmd_rule_list(project_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    match read_rules_config(project_dir)? {
        None => {
            out(&json!({ "rules": [], "count": 0, "enabled": 0, "disabled": 0 }));
        }
        Some(config) => {
            let enabled = config.rules.iter().filter(|r| r.enabled).count();
            let disabled = config.rules.len() - enabled;
            out(&json!({
                "rules": config.rules,
                "count": config.rules.len(),
                "enabled": enabled,
                "disabled": disabled,
            }));
        }
    }
    Ok(())
}

pub fn cmd_rule_add(project_dir: &str, rule_json: &str) -> Result<(), Box<dyn std::error::Error>> {
    let rule: Rule = serde_json::from_str(rule_json)?;
    let mut config = read_rules_config(project_dir)?.unwrap_or(RulesConfig {
        version: "1.0".to_string(),
        rules: Vec::new(),
    });

    if config.rules.iter().any(|r| r.id == rule.id) {
        return Err(format!("Rule with id '{}' already exists", rule.id).into());
    }

    let id = rule.id.clone();
    config.rules.push(rule);
    let count = config.rules.len();
    write_rules_config(project_dir, &config)?;

    out(&json!({ "success": true, "id": id, "rules_count": count }));
    Ok(())
}

pub fn cmd_rule_remove(project_dir: &str, rule_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = read_rules_config(project_dir)?
        .ok_or("rules.json not found")?;

    let before = config.rules.len();
    config.rules.retain(|r| r.id != rule_id);
    if config.rules.len() == before {
        return Err(format!("Rule '{}' not found", rule_id).into());
    }

    let count = config.rules.len();
    write_rules_config(project_dir, &config)?;

    out(&json!({ "success": true, "removed": rule_id, "rules_count": count }));
    Ok(())
}

pub fn cmd_rule_enable(project_dir: &str, rule_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = read_rules_config(project_dir)?
        .ok_or("rules.json not found")?;

    let rule = config.rules.iter_mut().find(|r| r.id == rule_id)
        .ok_or_else(|| format!("Rule '{}' not found", rule_id))?;
    rule.enabled = true;
    let id = rule.id.clone();

    write_rules_config(project_dir, &config)?;

    out(&json!({ "success": true, "id": id, "enabled": true }));
    Ok(())
}

pub fn cmd_rule_disable(project_dir: &str, rule_id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = read_rules_config(project_dir)?
        .ok_or("rules.json not found")?;

    let rule = config.rules.iter_mut().find(|r| r.id == rule_id)
        .ok_or_else(|| format!("Rule '{}' not found", rule_id))?;
    rule.enabled = false;
    let id = rule.id.clone();

    write_rules_config(project_dir, &config)?;

    out(&json!({ "success": true, "id": id, "enabled": false }));
    Ok(())
}
