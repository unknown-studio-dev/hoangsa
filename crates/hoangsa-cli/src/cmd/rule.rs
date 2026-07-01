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
    #[serde(default = "default_enforcement")]
    pub enforcement: Enforcement,
    pub matcher: String,
    pub conditions: Vec<Condition>,
    pub action: RuleAction,
    pub message: String,
    /// Names a stateful check to run instead of pattern conditions.
    /// Valid values: "require-memory-impact", "require-detect-changes".
    /// When set, `conditions` is ignored and the named check in hook.rs fires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stateful: Option<String>,
}

fn default_enforcement() -> Enforcement {
    Enforcement::Prompt
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Enforcement {
    Hook,
    Preflight,
    Prompt,
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

/// Path to the global rules file (`~/.hoangsa/rules.json`).
///
/// Honors `HOANGSA_HOME` (the hoangsa home dir) when set so tests and isolated
/// setups can point the global layer elsewhere; otherwise resolves `~/.hoangsa`
/// the same way the rest of the toolchain does. Returns `None` only when no
/// home directory can be resolved at all.
fn global_rules_path() -> Option<std::path::PathBuf> {
    // An empty HOANGSA_HOME is treated as unset — otherwise `Path::new("")`
    // would yield the relative path "rules.json" resolved against the process
    // cwd, letting a stray file in the working directory masquerade as the
    // global rules layer.
    match std::env::var_os("HOANGSA_HOME") {
        Some(home) if !home.is_empty() => Some(Path::new(&home).join("rules.json")),
        _ => hoangsa_memory_core::default_hoangsa_home()
            .ok()
            .map(|h| h.join("rules.json")),
    }
}

fn read_rules_config_at(path: &Path) -> Result<Option<RulesConfig>, Box<dyn std::error::Error>> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    let config: RulesConfig = serde_json::from_str(&content)?;
    Ok(Some(config))
}

fn read_rules_config(project_dir: &str) -> Result<Option<RulesConfig>, Box<dyn std::error::Error>> {
    read_rules_config_at(&rules_path(project_dir))
}

/// Project-layer-only read of `<project_dir>/.hoangsa/rules.json`.
///
/// Kept as a library export for `hoangsa-ui-server`, which manages the project
/// rules file directly (not the merged view). Unused by the CLI binary itself —
/// enforcement paths go through [`read_effective_rules_config`] instead.
#[allow(dead_code)]
pub fn read_rules_config_pub(project_dir: &str) -> Result<Option<RulesConfig>, Box<dyn std::error::Error>> {
    read_rules_config(project_dir)
}

/// Merge the global rule layer under the project layer. Project rules override
/// global rules with the same `id`; project-only ids are appended after the
/// (possibly overridden) global rules, preserving global order. A missing layer
/// contributes zero rules — no implicit defaults are ever injected.
fn merge_rule_layers(global: Option<RulesConfig>, project: Option<RulesConfig>) -> RulesConfig {
    let version = project
        .as_ref()
        .map(|c| c.version.clone())
        .or_else(|| global.as_ref().map(|c| c.version.clone()))
        .unwrap_or_else(|| "1.0".to_string());

    let mut rules: Vec<Rule> = global.map(|c| c.rules).unwrap_or_default();
    if let Some(project) = project {
        for pr in project.rules {
            match rules.iter().position(|r| r.id == pr.id) {
                Some(pos) => {
                    rules[pos] = pr;
                    // A hand-authored layer may repeat an id; the project
                    // override must win uniquely, so drop any later duplicates
                    // of the same id rather than leaving a stale copy that would
                    // keep enforcing after the override.
                    let id = rules[pos].id.clone();
                    let mut i = rules.len();
                    while i > pos + 1 {
                        i -= 1;
                        if rules[i].id == id {
                            rules.remove(i);
                        }
                    }
                }
                None => rules.push(pr),
            }
        }
    }
    RulesConfig { version, rules }
}

/// Read a single rules layer, tolerating both absence and malformed content:
/// a missing OR unparseable file yields `None` (that layer contributes no
/// rules) rather than an error. This keeps one corrupt file from taking down
/// enforcement for the OTHER layer.
fn read_layer_lenient(path: &Path) -> Option<RulesConfig> {
    read_rules_config_at(path).ok().flatten()
}

/// Resolve the effective rule set for `project_dir`: the global layer
/// (`~/.hoangsa/rules.json`) overlaid by the project layer
/// (`<project_dir>/.hoangsa/rules.json`), with project rules overriding global
/// rules of the same `id`.
///
/// Both layers are optional and each degrades independently. A missing OR
/// malformed file at either layer simply contributes no rules — so a project
/// that never ran `rule init` is unruled (nothing enforced implicitly), and a
/// corrupt global file can never silently disable a valid project rule. When
/// neither layer yields rules the result is an empty set.
pub fn read_effective_rules_config(project_dir: &str) -> RulesConfig {
    let global = global_rules_path().and_then(|path| read_layer_lenient(&path));
    let project = read_layer_lenient(&rules_path(project_dir));
    merge_rule_layers(global, project)
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

    // Effective rules = global (~/.hoangsa/rules.json) overlaid by project;
    // project overrides global by id. A missing OR malformed file at either
    // layer contributes no rules (graceful degradation, REQ-09); with an empty
    // rule set nothing matches and the gate approves.
    let config = read_effective_rules_config(&cwd);

    let mut warnings: Vec<(String, String, String)> = Vec::new(); // (rule_id, rule_name, message)

    for rule in &config.rules {
        if !rule.enabled {
            continue;
        }
        // Stateful rules are dispatched by `hook enforce`, not by rule-gate.
        // Skip here so empty conditions don't vacuously match every tool call.
        if rule.stateful.is_some() {
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
                format!("⚠️ RULE WARNING: {id}\n\nRule: {name}\n\n{msg}")
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

/// Default rule set seeded into `.hoangsa/rules.json` on first install.
/// Keep in sync with the brainstorm table at
/// `.hoangsa/sessions/brainstorm/rule-enforcement-without-duplication/BRAINSTORM.md`.
pub fn default_rules() -> Vec<Rule> {
    fn cond(field: &str, op: ConditionOp, value: &str) -> Condition {
        Condition { field: field.to_string(), op, value: value.to_string() }
    }
    #[allow(clippy::too_many_arguments)]
    fn rule(
        id: &str,
        name: &str,
        enforcement: Enforcement,
        matcher: &str,
        conditions: Vec<Condition>,
        action: RuleAction,
        message: &str,
        stateful: Option<&str>,
    ) -> Rule {
        Rule {
            id: id.to_string(),
            name: name.to_string(),
            enabled: true,
            enforcement,
            matcher: matcher.to_string(),
            conditions,
            action,
            message: message.to_string(),
            stateful: stateful.map(String::from),
        }
    }
    vec![
        rule(
            "no-edit-claude",
            "Block direct .claude/ edits",
            Enforcement::Prompt,
            "Edit|Write",
            vec![cond("file_path", ConditionOp::Contains, ".claude/")],
            RuleAction::Block,
            "Do not edit files in .claude/ directly — use hoangsa-cli or bin/install to manage",
            None,
        ),
        rule(
            "no-bare-unwrap",
            "Avoid bare unwrap()",
            Enforcement::Prompt,
            "Edit|Write",
            vec![cond("new_string", ConditionOp::Regex, r"\bunwrap\(\)")],
            RuleAction::Warn,
            "Use expect(\"context\") or ? instead of unwrap() — makes panic debugging easier",
            None,
        ),
        rule(
            "no-todo-unimplemented",
            "No todo!/unimplemented! in commits",
            Enforcement::Prompt,
            "Edit|Write",
            vec![cond("new_string", ConditionOp::Regex, r"\b(todo!|unimplemented!)")],
            RuleAction::Warn,
            "Do not commit unimplemented code — finish it or create an issue instead",
            None,
        ),
        rule(
            "no-git-add-force",
            "Block git add --force",
            Enforcement::Prompt,
            "Bash",
            vec![cond("command", ConditionOp::Regex, r"git\s+add\s+(-f|--force)")],
            RuleAction::Block,
            "Do not force-add gitignored files — check .gitignore or remove the -f flag",
            None,
        ),
        rule(
            "warn-git-add-all",
            "Warn on git add . / git add -A",
            Enforcement::Prompt,
            "Bash",
            vec![cond("command", ConditionOp::Regex, r"git\s+add\s+(-A|\.)")],
            RuleAction::Warn,
            "Prefer adding specific files by name — git add . may include unwanted files",
            None,
        ),
        rule(
            "no-git-stash",
            "Block git stash",
            Enforcement::Hook,
            "Bash",
            vec![cond("command", ConditionOp::Regex, r"git\s+stash")],
            RuleAction::Block,
            "Never use git stash — leads to lost work and confusing state",
            None,
        ),
        rule(
            "no-force-push-main",
            "Block git push --force to main/master",
            Enforcement::Hook,
            "Bash",
            vec![cond("command", ConditionOp::Regex, r"git\s+push.*--force.*(main|master)")],
            RuleAction::Block,
            "Never force-push to main/master — rewrites shared history",
            None,
        ),
        rule(
            "no-skip-hooks",
            "Block --no-verify",
            Enforcement::Hook,
            "Bash",
            vec![cond("command", ConditionOp::Regex, r"--no-verify")],
            RuleAction::Block,
            "Never skip git hooks — fix the underlying issue instead",
            None,
        ),
        rule(
            "require-memory-impact",
            "Require memory_impact before first edit to a source file",
            Enforcement::Hook,
            "Edit|Write",
            vec![],
            RuleAction::Block,
            "Run memory_impact on this file before editing. Softened: subsequent edits to the same file in this session are allowed.",
            Some("require-memory-impact"),
        ),
        rule(
            "require-detect-changes",
            "Require memory_detect_changes before git commit",
            Enforcement::Hook,
            "Bash",
            vec![],
            RuleAction::Block,
            "Run memory_detect_changes before committing to verify the change scope.",
            Some("require-detect-changes"),
        ),
        rule(
            "no-git-add-ignored",
            "Block git add of gitignored files",
            Enforcement::Hook,
            "Bash",
            vec![],
            RuleAction::Block,
            "git add contains gitignored files: {files}. Remove them from the command or update .gitignore.",
            Some("no-git-add-ignored"),
        ),
    ]
}

/// `rule init [project_dir]` — seed defaults if .hoangsa/rules.json is missing.
/// Idempotent: reports `already_initialized: true` and does nothing when the
/// file already exists, so re-running install never overwrites user edits.
pub fn cmd_rule_init(project_dir: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = rules_path(project_dir);
    if path.exists() {
        out(&json!({ "success": true, "already_initialized": true, "path": path.to_string_lossy() }));
        return Ok(());
    }
    let config = RulesConfig { version: "1.0".to_string(), rules: default_rules() };
    write_rules_config(project_dir, &config)?;
    out(&json!({
        "success": true,
        "already_initialized": false,
        "path": path.to_string_lossy(),
        "rules_count": config.rules.len(),
    }));
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
        return Err(format!("Rule '{rule_id}' not found").into());
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
        .ok_or_else(|| format!("Rule '{rule_id}' not found"))?;
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
        .ok_or_else(|| format!("Rule '{rule_id}' not found"))?;
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

    // 2. Collect enabled rules with prompt enforcement (hook/preflight rules are invisible to LLM)
    let enabled_rules: Vec<&Rule> = config
        .rules
        .iter()
        .filter(|r| r.enabled && r.enforcement == Enforcement::Prompt)
        .collect();
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
        block.clone()
    } else if existing.ends_with('\n') {
        format!("{existing}\n{block}")
    } else {
        format!("{existing}\n\n{block}")
    };

    // 6. Write CLAUDE.md
    fs::write(&claude_md_path, &updated)?;

    // 7. Sync to AGENTS.md (subagents read this instead of CLAUDE.md)
    let agents_md_path = Path::new(project_dir).join("AGENTS.md");
    let agents_existing = if agents_md_path.exists() {
        fs::read_to_string(&agents_md_path)?
    } else {
        String::new()
    };
    let agents_updated = if let (Some(start_idx), Some(end_idx)) = (
        agents_existing.find(START_MARKER),
        agents_existing.find(END_MARKER),
    ) {
        let end_of_end = end_idx + END_MARKER.len();
        format!("{}{}{}", &agents_existing[..start_idx], block, &agents_existing[end_of_end..])
    } else if agents_existing.is_empty() {
        block
    } else if agents_existing.ends_with('\n') {
        format!("{agents_existing}\n{block}")
    } else {
        format!("{agents_existing}\n\n{block}")
    };
    fs::write(&agents_md_path, agents_updated)?;

    out(&json!({
        "success": true,
        "synced": synced,
        "claude_md": claude_md_path.to_string_lossy(),
        "agents_md": agents_md_path.to_string_lossy()
    }));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_condition(field: &str, op: ConditionOp, value: &str) -> Condition {
        Condition {
            field: field.to_string(),
            op,
            value: value.to_string(),
        }
    }

    fn make_rule(matcher: &str, conditions: Vec<Condition>) -> Rule {
        Rule {
            id: "test-rule".to_string(),
            name: "Test Rule".to_string(),
            enabled: true,
            enforcement: Enforcement::Prompt,
            matcher: matcher.to_string(),
            conditions,
            action: RuleAction::Block,
            message: "Test block message".to_string(),
            stateful: None,
        }
    }

    // ── condition operator tests ──────────────────────────────────────────────

    #[test]
    fn test_rule_evaluate_glob_match() {
        let cond = make_condition("path", ConditionOp::Glob, "dist/*");
        assert!(evaluate_condition(&cond, "dist/bundle.js"));
    }

    #[test]
    fn test_rule_evaluate_glob_no_match() {
        let cond = make_condition("path", ConditionOp::Glob, "dist/*");
        assert!(!evaluate_condition(&cond, "src/main.rs"));
    }

    #[test]
    fn test_rule_evaluate_regex_match() {
        let cond = make_condition("content", ConditionOp::Regex, r"\beval\s*\(");
        assert!(evaluate_condition(&cond, "code eval(x)"));
    }

    #[test]
    fn test_rule_evaluate_regex_no_match() {
        let cond = make_condition("content", ConditionOp::Regex, r"\beval\s*\(");
        assert!(!evaluate_condition(&cond, "code evaluate(x)"));
    }

    #[test]
    fn test_rule_evaluate_contains_match() {
        let cond = make_condition("text", ConditionOp::Contains, "todo");
        assert!(evaluate_condition(&cond, "add todo item"));
    }

    #[test]
    fn test_rule_evaluate_not_contains_match() {
        let cond = make_condition("text", ConditionOp::NotContains, "todo");
        assert!(evaluate_condition(&cond, "add item"));
    }

    #[test]
    fn test_rule_evaluate_starts_with_match() {
        let cond = make_condition("path", ConditionOp::StartsWith, "/tmp");
        assert!(evaluate_condition(&cond, "/tmp/file.txt"));
    }

    #[test]
    fn test_rule_evaluate_starts_with_no_match() {
        let cond = make_condition("path", ConditionOp::StartsWith, "/tmp");
        assert!(!evaluate_condition(&cond, "/var/file.txt"));
    }

    #[test]
    fn test_rule_evaluate_invalid_regex() {
        // An unclosed bracket is an invalid regex — must return false, not panic
        let cond = make_condition("content", ConditionOp::Regex, "[unclosed");
        assert!(!evaluate_condition(&cond, "anything"));
    }

    #[test]
    fn test_rule_evaluate_invalid_glob() {
        // A pattern with only `**` and extra `[` is malformed in some glob libs
        // We use a pattern that the `glob` crate rejects (unmatched bracket)
        let cond = make_condition("path", ConditionOp::Glob, "[invalid");
        assert!(!evaluate_condition(&cond, "anything"));
    }

    // ── AND logic tests ───────────────────────────────────────────────────────

    #[test]
    fn test_rule_multi_condition_all_match() {
        let rule = make_rule(
            "Edit",
            vec![
                make_condition("path", ConditionOp::Glob, "dist/*"),
                make_condition("path", ConditionOp::Contains, "bundle"),
            ],
        );
        let input = json!({ "path": "dist/bundle.js" });
        assert!(evaluate_rule_conditions(&rule, &input));
    }

    #[test]
    fn test_rule_multi_condition_partial_match() {
        // First condition matches, second does not — AND → false
        let rule = make_rule(
            "Edit",
            vec![
                make_condition("path", ConditionOp::Glob, "dist/*"),
                make_condition("path", ConditionOp::Contains, "vendor"),
            ],
        );
        let input = json!({ "path": "dist/bundle.js" });
        assert!(!evaluate_rule_conditions(&rule, &input));
    }

    #[test]
    fn test_rule_missing_field() {
        // Condition references a field not present in tool_input → false
        let rule = make_rule(
            "Edit",
            vec![make_condition("nonexistent_field", ConditionOp::Contains, "foo")],
        );
        let input = json!({ "path": "dist/bundle.js" });
        assert!(!evaluate_rule_conditions(&rule, &input));
    }

    // ── gate / matcher logic tests ────────────────────────────────────────────

    #[test]
    fn test_rule_matcher_match() {
        // Tool name "Edit" should match against matcher "Edit|Write"
        let rule = make_rule(
            "Edit|Write",
            vec![make_condition("path", ConditionOp::Contains, "dist")],
        );
        let tool_name = "Edit";
        let tool_input = json!({ "path": "dist/bundle.js" });

        let matcher_matches = rule
            .matcher
            .split('|')
            .any(|m| m.trim() == tool_name);
        assert!(matcher_matches, "Expected tool_name 'Edit' to match matcher 'Edit|Write'");

        // Conditions also pass, so the rule would fire
        assert!(evaluate_rule_conditions(&rule, &tool_input));
    }

    // ── enforcement field tests ────────────────────────────────────────────

    #[test]
    fn test_enforcement_default_to_prompt() {
        let json_str = r#"{
            "id": "test", "name": "Test", "enabled": true,
            "matcher": "Bash", "conditions": [], "action": "block", "message": "msg"
        }"#;
        let rule: Rule = serde_json::from_str(json_str).expect("should parse without enforcement field");
        assert_eq!(rule.enforcement, Enforcement::Prompt);
    }

    #[test]
    fn test_enforcement_hook_roundtrip() {
        let json_str = r#"{
            "id": "test", "name": "Test", "enabled": true, "enforcement": "hook",
            "matcher": "Bash", "conditions": [], "action": "block", "message": "msg"
        }"#;
        let rule: Rule = serde_json::from_str(json_str).expect("should parse with enforcement=hook");
        assert_eq!(rule.enforcement, Enforcement::Hook);
        let serialized = serde_json::to_string(&rule).expect("should serialize");
        assert!(serialized.contains(r#""enforcement":"hook""#));
    }

    #[test]
    fn test_enforcement_preflight_roundtrip() {
        let json_str = r#"{
            "id": "test", "name": "Test", "enabled": true, "enforcement": "preflight",
            "matcher": "Bash", "conditions": [], "action": "block", "message": "msg"
        }"#;
        let rule: Rule = serde_json::from_str(json_str).expect("should parse");
        assert_eq!(rule.enforcement, Enforcement::Preflight);
    }

    #[test]
    fn test_build_rules_block_excludes_hook_enforcement() {
        let prompt_rule = Rule {
            id: "prompt-rule".to_string(),
            name: "Prompt Rule".to_string(),
            enabled: true,
            enforcement: Enforcement::Prompt,
            matcher: "Bash".to_string(),
            conditions: vec![],
            action: RuleAction::Block,
            message: "visible".to_string(),
            stateful: None,
        };
        let hook_rule = Rule {
            id: "hook-rule".to_string(),
            name: "Hook Rule".to_string(),
            enabled: true,
            enforcement: Enforcement::Hook,
            matcher: "Bash".to_string(),
            conditions: vec![],
            action: RuleAction::Block,
            message: "invisible".to_string(),
            stateful: None,
        };
        let prompt_only: Vec<&Rule> = vec![&prompt_rule, &hook_rule]
            .into_iter()
            .filter(|r| r.enforcement == Enforcement::Prompt)
            .collect();
        let block = build_rules_block(&prompt_only);
        assert!(block.contains("Prompt Rule"));
        assert!(!block.contains("Hook Rule"));
    }

    #[test]
    fn test_rule_matcher_no_match() {
        // Tool name "Bash" should NOT match matcher "Edit|Write"
        let rule = make_rule(
            "Edit|Write",
            vec![make_condition("path", ConditionOp::Contains, "dist")],
        );
        let tool_name = "Bash";

        let matcher_matches = rule
            .matcher
            .split('|')
            .any(|m| m.trim() == tool_name);
        assert!(!matcher_matches, "Expected tool_name 'Bash' NOT to match matcher 'Edit|Write'");
    }

    // ── layered scope (global → project) tests ─────────────────────────────────

    fn rule_with_id(id: &str) -> Rule {
        let mut r = make_rule("Edit", vec![]);
        r.id = id.to_string();
        r
    }

    fn config(rules: Vec<Rule>) -> RulesConfig {
        RulesConfig { version: "1.0".to_string(), rules }
    }

    #[test]
    fn merge_both_absent_is_empty_no_implicit_defaults() {
        // The core "chưa init → coi như không có rules" contract: nothing on
        // either layer means an empty rule set, never the built-in defaults.
        let merged = merge_rule_layers(None, None);
        assert!(merged.rules.is_empty());
    }

    #[test]
    fn merge_single_layer_passes_through() {
        let g = merge_rule_layers(Some(config(vec![rule_with_id("g")])), None);
        assert_eq!(g.rules.len(), 1);
        assert_eq!(g.rules[0].id, "g");

        let p = merge_rule_layers(None, Some(config(vec![rule_with_id("p")])));
        assert_eq!(p.rules.len(), 1);
        assert_eq!(p.rules[0].id, "p");
    }

    #[test]
    fn merge_project_overrides_global_by_id_and_appends_new() {
        let mut g_shared = rule_with_id("shared");
        g_shared.message = "GLOBAL".to_string();
        let global = config(vec![g_shared, rule_with_id("global-only")]);

        let mut p_shared = rule_with_id("shared");
        p_shared.message = "PROJECT".to_string();
        let project = config(vec![p_shared, rule_with_id("project-only")]);

        let merged = merge_rule_layers(Some(global), Some(project));
        // shared id → project wins
        let shared = merged.rules.iter().find(|r| r.id == "shared").unwrap();
        assert_eq!(shared.message, "PROJECT");
        // global-only kept, project-only appended, no duplicate for shared
        assert_eq!(merged.rules.len(), 3);
        assert!(merged.rules.iter().any(|r| r.id == "global-only"));
        assert!(merged.rules.iter().any(|r| r.id == "project-only"));
    }

    #[test]
    fn merge_project_can_disable_a_global_rule() {
        let mut g = rule_with_id("x");
        g.enabled = true;
        let mut p = rule_with_id("x");
        p.enabled = false;
        let merged = merge_rule_layers(Some(config(vec![g])), Some(config(vec![p])));
        assert_eq!(merged.rules.len(), 1);
        assert!(!merged.rules[0].enabled, "project override switches the global rule off");
    }

    #[test]
    fn merge_override_removes_duplicate_global_ids() {
        // A malformed global layer repeats an id; a project override must win
        // uniquely and not leave a stale enabled copy still enforcing.
        let mut g1 = rule_with_id("dup");
        g1.enabled = true;
        let mut g2 = rule_with_id("dup");
        g2.enabled = true;
        let mut p = rule_with_id("dup");
        p.enabled = false;
        let merged = merge_rule_layers(
            Some(config(vec![g1, rule_with_id("keep"), g2])),
            Some(config(vec![p])),
        );
        let dups: Vec<&Rule> = merged.rules.iter().filter(|r| r.id == "dup").collect();
        assert_eq!(dups.len(), 1, "only one rule with the overridden id survives");
        assert!(!dups[0].enabled, "the survivor is the project override");
        assert!(
            merged.rules.iter().any(|r| r.id == "keep"),
            "an unrelated global rule is untouched"
        );
    }
}
