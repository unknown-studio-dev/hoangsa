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

pub fn cmd_rule_gate() -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;

    // Read all of stdin
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    // Parse the hook payload: {tool_name, tool_input}
    let parsed: serde_json::Value = serde_json::from_str(&input).unwrap_or(serde_json::json!({}));
    let tool_name = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_input = parsed
        .get("tool_input")
        .cloned()
        .unwrap_or(serde_json::json!({}));

    // Resolve rules.json path via cwd
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Graceful degradation: rules.json missing → approve
    let config = match read_rules_config(&cwd) {
        Ok(Some(c)) => c,
        Ok(None) => {
            out(&json!({"decision": "approve"}));
            return Ok(());
        }
        Err(_) => {
            // Parse/IO error → approve (graceful degradation, REQ-09)
            out(&json!({"decision": "approve"}));
            return Ok(());
        }
    };

    let mut warnings: Vec<(String, String, String)> = Vec::new(); // (rule_id, rule_name, message)

    for rule in &config.rules {
        if !rule.enabled {
            continue;
        }

        // Check tool_name matches rule.matcher (pipe-split list)
        let matcher_matches = rule
            .matcher
            .split('|')
            .any(|m| m.trim() == tool_name);
        if !matcher_matches {
            continue;
        }

        // Evaluate all conditions against tool_input
        if !evaluate_rule_conditions(rule, &tool_input) {
            continue;
        }

        // All conditions matched
        match rule.action {
            RuleAction::Block => {
                // First match wins for block
                let matched_condition = rule.conditions.first();
                let field_info = matched_condition
                    .map(|c| format!("Field: {} matched {} '{}'", c.field, op_label(&c.op), c.value))
                    .unwrap_or_default();
                let reason = format!(
                    "⛔ RULE VIOLATION: {}\n\nRule: {}\n{}\nAction: BLOCK\n\n{}",
                    rule.id, rule.name, field_info, rule.message
                );
                out(&json!({"decision": "block", "reason": reason}));
                return Ok(());
            }
            RuleAction::Warn => {
                warnings.push((rule.id.clone(), rule.name.clone(), rule.message.clone()));
            }
        }
    }

    // No blocking rule matched
    if warnings.is_empty() {
        out(&json!({"decision": "approve"}));
    } else {
        let reason = warnings
            .iter()
            .map(|(id, name, msg)| {
                format!("⚠️ RULE WARNING: {}\n\nRule: {}\n\n{}", id, name, msg)
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n");
        out(&json!({"decision": "approve", "reason": reason}));
    }

    Ok(())
}

fn op_label(op: &ConditionOp) -> &'static str {
    match op {
        ConditionOp::Glob => "glob",
        ConditionOp::Regex => "regex",
        ConditionOp::Contains => "contains",
        ConditionOp::NotContains => "not_contains",
        ConditionOp::StartsWith => "starts_with",
    }
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

fn condition_summary(condition: &Condition) -> String {
    let op_str = op_label(&condition.op);
    format!("{} {} \"{}\"", condition.field, op_str, condition.value)
}

fn build_rules_block(enabled_rules: &[&Rule]) -> String {
    let block_rules: Vec<&&Rule> = enabled_rules
        .iter()
        .filter(|r| matches!(r.action, RuleAction::Block))
        .collect();
    let warn_rules: Vec<&&Rule> = enabled_rules
        .iter()
        .filter(|r| matches!(r.action, RuleAction::Warn))
        .collect();

    let mut lines: Vec<String> = Vec::new();
    lines.push("<!-- hoangsa-rules-start -->".to_string());
    lines.push("## HOANGSA Rules (auto-generated — DO NOT edit manually)".to_string());
    lines.push(String::new());

    if enabled_rules.is_empty() {
        lines.push("No active rules.".to_string());
    } else {
        // Hard Rules (block)
        lines.push("### ⛔ Hard Rules (block)".to_string());
        if block_rules.is_empty() {
            lines.push("_None_".to_string());
        } else {
            lines.push("| Rule | Trigger | Condition | Message |".to_string());
            lines.push("|------|---------|-----------|---------|".to_string());
            for rule in &block_rules {
                let condition_col = if rule.conditions.is_empty() {
                    "-".to_string()
                } else {
                    rule.conditions
                        .iter()
                        .map(condition_summary)
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                lines.push(format!(
                    "| {} | {} | {} | {} |",
                    rule.name, rule.matcher, condition_col, rule.message
                ));
            }
        }
        lines.push(String::new());

        // Warnings
        lines.push("### ⚠️ Warnings".to_string());
        if warn_rules.is_empty() {
            lines.push("_None_".to_string());
        } else {
            lines.push("| Rule | Trigger | Condition | Message |".to_string());
            lines.push("|------|---------|-----------|---------|".to_string());
            for rule in &warn_rules {
                let condition_col = if rule.conditions.is_empty() {
                    "-".to_string()
                } else {
                    rule.conditions
                        .iter()
                        .map(condition_summary)
                        .collect::<Vec<_>>()
                        .join("; ")
                };
                lines.push(format!(
                    "| {} | {} | {} | {} |",
                    rule.name, rule.matcher, condition_col, rule.message
                ));
            }
        }
    }

    lines.push(String::new());
    lines.push("<!-- hoangsa-rules-end -->".to_string());
    lines.join("\n")
}

pub fn cmd_rule_sync(project_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let claude_md_path = Path::new(project_dir).join("CLAUDE.md");

    // 1. Read rules config
    let config = match read_rules_config(project_dir)? {
        None => {
            out(&json!({
                "success": true,
                "synced": 0,
                "claude_md": claude_md_path.to_string_lossy()
            }));
            return Ok(());
        }
        Some(c) => c,
    };

    // 2. Collect enabled rules
    let enabled_rules: Vec<&Rule> = config.rules.iter().filter(|r| r.enabled).collect();
    let synced = enabled_rules.len();

    // 3. Build markdown block
    let block = build_rules_block(&enabled_rules);

    // 4. Read or initialize CLAUDE.md
    let existing = if claude_md_path.exists() {
        fs::read_to_string(&claude_md_path)?
    } else {
        String::new()
    };

    // 5. Replace between markers or append
    const START_MARKER: &str = "<!-- hoangsa-rules-start -->";
    const END_MARKER: &str = "<!-- hoangsa-rules-end -->";

    let updated = if let (Some(start_idx), Some(end_idx)) = (
        existing.find(START_MARKER),
        existing.find(END_MARKER),
    ) {
        let end_of_end = end_idx + END_MARKER.len();
        format!("{}{}{}", &existing[..start_idx], block, &existing[end_of_end..])
    } else if existing.is_empty() {
        block
    } else if existing.ends_with('\n') {
        format!("{}\n{}", existing, block)
    } else {
        format!("{}\n\n{}", existing, block)
    };

    // 6. Write CLAUDE.md
    fs::write(&claude_md_path, updated)?;

    out(&json!({
        "success": true,
        "synced": synced,
        "claude_md": claude_md_path.to_string_lossy()
    }));
    Ok(())
}
