//! PreToolUse hook installer.
//!
//! `hsp init` writes a PreToolUse entry into Claude Code's `settings.json`
//! (global: `~/.claude/settings.json`, or project: `<cwd>/.claude/settings.local.json`)
//! that invokes `hsp hook rewrite`. Existing unrelated hook entries are preserved.
//!
//! The installer is intentionally conservative: it refuses to clobber an
//! entry it did not write, and `uninit` only removes entries it recognises
//! by the `__hsp` marker embedded in the command string.

use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

pub const HSP_MARKER: &str = "# __hsp";

#[derive(Debug, Clone, Copy)]
pub enum Scope {
    Global,
    Project,
}

pub fn settings_path(scope: Scope, cwd: &Path) -> Option<PathBuf> {
    match scope {
        Scope::Global => dirs::home_dir().map(|h| h.join(".claude/settings.json")),
        Scope::Project => Some(cwd.join(".claude/settings.local.json")),
    }
}

pub fn install(scope: Scope, cwd: &Path) -> anyhow::Result<PathBuf> {
    let path = settings_path(scope, cwd)
        .ok_or_else(|| anyhow::anyhow!("could not resolve settings.json path"))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut settings = load_json(&path)?;
    let settings_obj = settings
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("settings.json root is not an object"))?;

    let hooks = settings_obj
        .entry("hooks")
        .or_insert_with(|| json!({}));
    let hooks_obj = hooks
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("hooks entry is not an object"))?;

    let pre_tool = hooks_obj
        .entry("PreToolUse")
        .or_insert_with(|| json!([]));
    let pre_tool_arr = pre_tool
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("PreToolUse entry is not an array"))?;

    // Remove any prior hsp-owned entry first (idempotent install).
    pre_tool_arr.retain(|entry| !is_hsp_entry(entry));

    // Append the fresh entry.
    pre_tool_arr.push(hsp_hook_entry());

    save_json(&path, &settings)?;
    Ok(path)
}

pub fn uninstall(scope: Scope, cwd: &Path) -> anyhow::Result<PathBuf> {
    let path = settings_path(scope, cwd)
        .ok_or_else(|| anyhow::anyhow!("could not resolve settings.json path"))?;
    if !path.exists() {
        return Ok(path);
    }
    let mut settings = load_json(&path)?;
    if let Some(hooks) = settings.get_mut("hooks").and_then(|v| v.as_object_mut()) {
        if let Some(arr) = hooks.get_mut("PreToolUse").and_then(|v| v.as_array_mut()) {
            arr.retain(|e| !is_hsp_entry(e));
        }
    }
    save_json(&path, &settings)?;
    Ok(path)
}

fn load_json(path: &Path) -> anyhow::Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let raw = fs::read_to_string(path)?;
    if raw.trim().is_empty() {
        return Ok(json!({}));
    }
    let v: Value = serde_json::from_str(&raw)
        .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", path.display()))?;
    Ok(v)
}

fn save_json(path: &Path, v: &Value) -> anyhow::Result<()> {
    let pretty = serde_json::to_string_pretty(v)?;
    fs::write(path, format!("{pretty}\n"))?;
    Ok(())
}

fn is_hsp_entry(entry: &Value) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else {
        return false;
    };
    hooks.iter().any(|h| {
        h.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|s| s.contains(HSP_MARKER))
    })
}

fn hsp_hook_entry() -> Value {
    json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": format!("hsp hook rewrite {HSP_MARKER}")
        }]
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn install_creates_and_idempotent() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();

        let settings = install(Scope::Project, &path).unwrap();
        assert!(settings.exists());
        let v: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);

        // Install again — still one entry.
        install(Scope::Project, &path).unwrap();
        let v2: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(v2["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn install_preserves_other_entries() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        let settings = path.join(".claude/settings.local.json");
        fs::create_dir_all(settings.parent().unwrap()).unwrap();
        fs::write(
            &settings,
            r#"{"hooks":{"PreToolUse":[{"matcher":"Edit","hooks":[{"type":"command","command":"other"}]}]}}"#,
        )
        .unwrap();

        install(Scope::Project, &path).unwrap();
        let v: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 2, "preserve existing entries");
    }

    #[test]
    fn uninstall_removes_only_hsp_entry() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();
        install(Scope::Project, &path).unwrap();
        let settings = path.join(".claude/settings.local.json");
        // Add a foreign entry
        let mut v: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        v["hooks"]["PreToolUse"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "matcher": "Write",
                "hooks": [{"type": "command", "command": "foreign-hook"}]
            }));
        fs::write(&settings, serde_json::to_string_pretty(&v).unwrap()).unwrap();

        uninstall(Scope::Project, &path).unwrap();
        let v2: Value = serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        let arr = v2["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], "foreign-hook");
    }
}
