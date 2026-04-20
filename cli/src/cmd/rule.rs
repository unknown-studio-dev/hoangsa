use glob::Pattern;
use regex::Regex;
use serde::{Deserialize, Serialize};

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
