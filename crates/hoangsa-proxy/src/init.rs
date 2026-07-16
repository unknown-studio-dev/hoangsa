//! PreToolUse hook installer.
//!
//! `hsp init` writes a PreToolUse entry into the harness's hook config —
//! Claude Code's `settings.json` (global: `~/.claude/settings.json`, or
//! project: `<cwd>/.claude/settings.local.json`) or, with `--codex`,
//! Codex's `hooks.json` (global: `~/.codex/hooks.json`, project:
//! `<cwd>/.codex/hooks.json`). Both formats nest entries under a
//! top-level `hooks` key with the same entry shape, so one merge path
//! serves both. Existing unrelated hook entries are preserved.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Harness {
    Claude,
    Codex,
}

pub fn settings_path(scope: Scope, cwd: &Path) -> Option<PathBuf> {
    hook_config_path(scope, Harness::Claude, cwd)
}

pub fn hook_config_path(scope: Scope, harness: Harness, cwd: &Path) -> Option<PathBuf> {
    match (harness, scope) {
        (Harness::Claude, Scope::Global) => {
            dirs::home_dir().map(|h| h.join(".claude/settings.json"))
        }
        (Harness::Claude, Scope::Project) => Some(cwd.join(".claude/settings.local.json")),
        (Harness::Codex, Scope::Global) => codex_home().map(|h| h.join("hooks.json")),
        (Harness::Codex, Scope::Project) => Some(cwd.join(".codex/hooks.json")),
    }
}

/// `$CODEX_HOME` (as Codex itself resolves it) or `~/.codex`.
fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME") {
        Some(raw) if !raw.is_empty() => Some(PathBuf::from(raw)),
        _ => dirs::home_dir().map(|h| h.join(".codex")),
    }
}

pub fn install(scope: Scope, cwd: &Path) -> anyhow::Result<PathBuf> {
    install_for(scope, Harness::Claude, cwd)
}

pub fn install_for(scope: Scope, harness: Harness, cwd: &Path) -> anyhow::Result<PathBuf> {
    let path = hook_config_path(scope, harness, cwd)
        .ok_or_else(|| anyhow::anyhow!("could not resolve hook config path"))?;
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
    pre_tool_arr.push(hsp_hook_entry(harness));

    save_json(&path, &settings)?;
    Ok(path)
}

pub fn uninstall(scope: Scope, cwd: &Path) -> anyhow::Result<PathBuf> {
    uninstall_for(scope, Harness::Claude, cwd)
}

pub fn uninstall_for(scope: Scope, harness: Harness, cwd: &Path) -> anyhow::Result<PathBuf> {
    let path = hook_config_path(scope, harness, cwd)
        .ok_or_else(|| anyhow::anyhow!("could not resolve hook config path"))?;
    if !path.exists() {
        return Ok(path);
    }
    let mut settings = load_json(&path)?;
    if let Some(hooks) = settings.get_mut("hooks").and_then(|v| v.as_object_mut())
        && let Some(arr) = hooks.get_mut("PreToolUse").and_then(|v| v.as_array_mut())
    {
        arr.retain(|e| !is_hsp_entry(e));
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

fn hsp_hook_entry(harness: Harness) -> Value {
    // Codex needs the flag so `hook rewrite` emits Codex-shaped output
    // (permissionDecision/updatedInput instead of decision/modifiedToolInput).
    let command = match harness {
        Harness::Claude => format!("hsp hook rewrite {HSP_MARKER}"),
        Harness::Codex => format!("hsp hook rewrite --codex {HSP_MARKER}"),
    };
    json!({
        "matcher": "Bash",
        "hooks": [{
            "type": "command",
            "command": command
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
    fn install_codex_targets_hooks_json_with_codex_flag() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().to_path_buf();

        let file = install_for(Scope::Project, Harness::Codex, &path).unwrap();
        assert!(file.ends_with(".codex/hooks.json"), "{}", file.display());
        let v: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        let cmd = v["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
            .as_str()
            .unwrap();
        assert!(cmd.contains("--codex"), "{cmd}");
        assert!(cmd.contains(HSP_MARKER));

        // Idempotent.
        install_for(Scope::Project, Harness::Codex, &path).unwrap();
        let v2: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(v2["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);

        // Uninstall removes it.
        uninstall_for(Scope::Project, Harness::Codex, &path).unwrap();
        let v3: Value = serde_json::from_str(&fs::read_to_string(&file).unwrap()).unwrap();
        assert!(v3["hooks"]["PreToolUse"].as_array().unwrap().is_empty());
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
