// ───────────────────────── hooks submodule ─────────────────────────
//
// Port of `bin/install`'s `ensureHoangsaHooks` + `cleanupHooksFromSettings`
// + the top-level `settings.json` read/write helpers. Owns:
//
//   * HOANGSA hook payload construction (command = `~/.hoangsa/bin/hoangsa-cli hook <event>`)
//   * idempotent merge into an existing Claude Code `settings.json`
//   * statusLine preservation (we only default; we never clobber a user-tuned value)
//
// The hook entry shape matches what the Node installer emits — each entry
// carries `_hoangsa_managed: true` so future runs (and uninstall) can find
// and replace them without touching user-authored hooks.
//
// Source of truth for the hook list: `bin/install` (search for
// `ensureHoangsaHooks`). If `templates/.claude/settings.json` ever lands
// in the template tree we can switch to reading from there; today we
// inline the hook payload here.

use super::*;
use serde_json::{Value, json};

/// Sentinel key we write on every HOANGSA-managed hook entry so we can
/// find (and replace) our own entries without walking command strings.
pub const MANAGED_SENTINEL: &str = "_hoangsa_managed";

/// Resolve the `settings.json` path for the given install mode.
/// `global` → `$CLAUDE_CONFIG_DIR/settings.json` (fallback `~/.claude/settings.json`);
/// `local`  → `<cwd>/.claude/settings.json`.
pub fn settings_path(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
    match mode {
        "global" => Ok(super::claude_config_dir()?.join("settings.json")),
        _ => Ok(cwd.join(".claude").join("settings.json")),
    }
}

/// Read existing `settings.json`. Returns an empty object when the file
/// is missing (fresh install), surfaces a parse failure as an error so
/// the caller aborts rather than overwriting a corrupt-but-recoverable
/// config with an empty shell. Other I/O errors bubble up unchanged.
///
/// A JSON value that parses but isn't an object (e.g. `null`, array,
/// scalar) is treated as "not a settings file" and converted to an
/// empty object — preserves the prior lenient behavior for that one
/// edge case while still failing hard on actual JSON corruption.
pub fn load_settings(path: &Path) -> io::Result<Value> {
    match fs::read_to_string(path) {
        Ok(raw) => match serde_json::from_str::<Value>(&raw) {
            Ok(v) if v.is_object() => Ok(v),
            Ok(_) => Ok(Value::Object(serde_json::Map::new())),
            Err(e) => Err(io::Error::other(format!(
                "parse settings.json at {}: {e}",
                path.display()
            ))),
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Value::Object(serde_json::Map::new())),
        Err(e) => Err(e),
    }
}

/// Save `settings` with two-space pretty JSON + trailing newline — matches
/// the format Claude Code writes (and matches the Node installer).
pub fn save_settings(path: &Path, settings: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut out = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    out.push('\n');
    fs::write(path, out)
}

/// Build the HOANGSA-managed hook tree keyed by Claude Code event name.
/// `target_dir` is the `.claude/` directory (parent of `hoangsa/`). The CLI
/// itself lives globally in `~/.hoangsa/bin/` (or whatever
/// `$HOANGSA_INSTALL_DIR/bin` resolves to), not under the project-scoped
/// template tree, so the hook command points at that global launcher.
/// Mirrors `ensureHoangsaHooks` in `bin/install`.
/// Marker embedded in the hsp PreToolUse hook command so either
/// `hsp uninit` (which looks for this string) or a subsequent
/// `hoangsa-cli install` (via `is_hoangsa_entry`) can identify and
/// remove the entry. Must match `hoangsa_proxy::init::HSP_MARKER`.
pub const HSP_MARKER: &str = "# __hsp";

pub fn build_hoangsa_hooks(_target_dir: &Path) -> Value {
    build_hoangsa_hooks_inner(super::memory_install_dir().ok().as_deref())
}

/// Core payload builder. `install_root` is `~/.hoangsa/` (or
/// `$HOANGSA_INSTALL_DIR`) — tests inject a sandboxed path so the hsp
/// presence check is deterministic instead of reading the caller's env.
pub fn build_hoangsa_hooks_inner(install_root: Option<&Path>) -> Value {
    let cli = install_root
        .map(|d| d.join("bin").join("hoangsa-cli"))
        .unwrap_or_else(|| PathBuf::from("hoangsa-cli"))
        .display()
        .to_string();

    let managed_entry = |command: String, timeout: u64, matcher: Option<&str>| -> Value {
        let mut obj = serde_json::Map::new();
        obj.insert(MANAGED_SENTINEL.into(), Value::Bool(true));
        if let Some(m) = matcher {
            obj.insert("matcher".into(), Value::String(m.into()));
        }
        obj.insert(
            "hooks".into(),
            json!([{
                "type": "command",
                "command": command,
                "timeout": timeout,
            }]),
        );
        Value::Object(obj)
    };

    let mut pre_tool_use = vec![
        managed_entry(format!("{cli} hook lesson-guard"), 10, Some("Edit|Write")),
        managed_entry(format!("{cli} hook enforce"), 10, Some("Edit|Write|Bash|NotebookEdit")),
    ];

    // `hsp` normally ships alongside the memory bins in both release
    // tarballs and local-dev installs. We still gate the hook on the
    // file actually being present so test fixtures, partial installs,
    // or users who manually removed `hsp` never leave a dangling
    // command pointing at a missing executable.
    if let Some(hsp) = install_root
        .map(|d| d.join("bin").join("hsp"))
        .filter(|p| p.exists())
    {
        pre_tool_use.push(managed_entry(
            format!("{} hook rewrite {HSP_MARKER}", hsp.display()),
            10,
            Some("Bash"),
        ));
    }

    json!({
        "SessionStart": [
            managed_entry(format!("{cli} hook state-clear"), 5, None),
            // Post-install auto-bootstrap: first SessionStart per
            // project kicks off a detached `hoangsa-cli bootstrap`
            // so users don't have to run `hoangsa-memory index` +
            // `archive ingest` by hand. Subsequent fires short-
            // circuit via the `.bootstrap-done` sentinel.
            managed_entry(format!("{cli} hook session-start"), 5, None),
        ],
        "Stop": [
            managed_entry(format!("{cli} hook stop-check"), 5, None),
            managed_entry(format!("{cli} hook session-usage"), 5, None),
        ],
        "PostToolUse": [managed_entry(
            format!("{cli} hook post-enforce"),
            5,
            Some("mcp__hoangsa-memory__memory_impact|mcp__hoangsa-memory__memory_detect_changes|mcp__hoangsa-memory__memory_recall|Edit|Write|MultiEdit"),
        )],
        "PreToolUse": pre_tool_use,
        "PreCompact": [managed_entry(format!("{cli} hook session-archive"), 5, None)],
        "SessionEnd": [managed_entry(format!("{cli} hook session-archive"), 5, None)],
    })
}

/// Return `true` iff `entry` is a HOANGSA-managed hook object (carries
/// the sentinel flag OR references our binary via the legacy command form).
fn is_hoangsa_entry(entry: &Value) -> bool {
    let Some(obj) = entry.as_object() else {
        return false;
    };
    if obj.get(MANAGED_SENTINEL).and_then(|v| v.as_bool()).unwrap_or(false) {
        return true;
    }
    if let Some(hooks) = obj.get("hooks").and_then(|h| h.as_array()) {
        for h in hooks {
            if let Some(cmd) = h.get("command").and_then(|c| c.as_str())
                && (cmd.contains("hoangsa-cli") || cmd.contains(HSP_MARKER))
            {
                return true;
            }
        }
    }
    false
}

/// Dedupe key for entries: matcher (or "") + first command string.
/// Sufficient for our own entries and for the common user-authored shape.
fn entry_dedupe_key(entry: &Value) -> String {
    let matcher = entry
        .get("matcher")
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let cmd = entry
        .get("hooks")
        .and_then(|h| h.as_array())
        .and_then(|a| a.first())
        .and_then(|h0| h0.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    format!("{matcher}\x1f{cmd}")
}

/// Merge HOANGSA hooks into `settings["hooks"]`:
///
///   1. Strip any prior HOANGSA-managed entries per event (so re-runs stay idempotent).
///   2. Append our fresh entries, deduping by (matcher, first command).
///   3. Preserve every non-HOANGSA entry the user may have authored.
///
/// Returns the count of entries we added.
pub fn merge_hoangsa_hooks(settings: &mut Value, hoangsa_hooks: &Value) -> usize {
    let mut added = 0usize;

    let settings_obj = match settings.as_object_mut() {
        Some(o) => o,
        None => return 0,
    };
    let hooks_val = settings_obj
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    let hooks_obj = match hooks_val.as_object_mut() {
        Some(o) => o,
        None => {
            *hooks_val = Value::Object(serde_json::Map::new());
            hooks_val.as_object_mut().expect("just replaced with object")
        }
    };

    let Some(incoming) = hoangsa_hooks.as_object() else {
        return 0;
    };

    for (event, new_entries) in incoming {
        let Some(new_arr) = new_entries.as_array() else {
            continue;
        };

        // Grab existing array for this event (or start fresh), drop our old entries.
        let existing_arr = hooks_obj
            .remove(event)
            .and_then(|v| v.as_array().cloned())
            .unwrap_or_default();
        let mut preserved: Vec<Value> = existing_arr
            .into_iter()
            .filter(|e| !is_hoangsa_entry(e))
            .collect();

        // Track the dedupe keys already present in `preserved` so we don't
        // duplicate a user's hook that happens to mirror ours.
        let mut seen: std::collections::HashSet<String> =
            preserved.iter().map(entry_dedupe_key).collect();

        for entry in new_arr {
            let key = entry_dedupe_key(entry);
            if seen.insert(key) {
                preserved.push(entry.clone());
                added += 1;
            }
        }

        hooks_obj.insert(event.clone(), Value::Array(preserved));
    }

    added
}

/// Set `settings["statusLine"]` to `statusline_spec`.
///
/// Preserves a *user-authored* statusLine, but heals a *hoangsa-managed*
/// one whose binary path no longer exists on disk — the previous "preserve
/// anything non-null" rule turned tmp-dir test installs into permanently
/// broken statuslines, since later normal installs couldn't overwrite.
///
/// A statusLine is considered hoangsa-managed when its `command` invokes
/// our `hook statusline` subcommand (signature match — we own that
/// argument shape). If the binary in front of `hook statusline` is
/// missing, overwrite. Otherwise preserve.
///
/// Returns `true` iff we wrote a new value.
pub fn apply_statusline(settings: &mut Value, statusline_spec: &Value) -> bool {
    let Some(obj) = settings.as_object_mut() else {
        return false;
    };
    match obj.get("statusLine") {
        Some(v) if !v.is_null() => {
            if !is_stale_managed_statusline(v) {
                return false;
            }
            // Stale managed entry — fall through and overwrite.
        }
        _ => {}
    }
    obj.insert("statusLine".into(), statusline_spec.clone());
    true
}

/// `true` when a statusLine value points at our `hook statusline` handler
/// but the binary in front of it is missing on disk.
fn is_stale_managed_statusline(v: &Value) -> bool {
    let cmd = match v.get("command").and_then(|c| c.as_str()) {
        Some(s) => s,
        None => return false,
    };
    // Signature: ".../hoangsa-cli hook statusline" (any leading binary
    // path, ours or not, as long as the subcommand is `hook statusline`).
    let bin = match cmd.split(" hook statusline").next() {
        Some(b) if b != cmd => b.trim(),
        _ => return false,
    };
    if bin.is_empty() {
        return false;
    }
    !Path::new(bin).exists()
}

/// Default statusLine spec — points at our own `hook statusline` subcommand
/// (the CLI handler for which lives in a later task; we only wire it here).
pub fn default_statusline(_target_dir: &Path) -> Value {
    let cli = super::memory_install_dir()
        .map(|d| d.join("bin").join("hoangsa-cli"))
        .unwrap_or_else(|_| PathBuf::from("hoangsa-cli"))
        .display()
        .to_string();
    json!({
        "type": "command",
        "command": format!("{cli} hook statusline"),
        "padding": 0,
    })
}

/// Write a single stable `.bak` next to the original before any in-place
/// rewrite. Overwrites the previous backup so repeat installs don't
/// pile up `settings.json.bak-<stamp>` files in the user's config dir.
/// Legacy timestamped siblings from earlier versions are swept on the
/// way through. A missing source file is a no-op (fresh install).
pub fn backup_settings(path: &Path) -> io::Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let file_name = path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".to_string());
    let backup = path.with_file_name(format!("{file_name}.bak"));
    fs::copy(path, &backup)?;
    sweep_legacy_backups(path, &backup);
    Ok(Some(backup))
}

/// Sweep `<file_name>.bak-*` siblings; `keep` is never deleted. Errors are
/// swallowed — a stale backup is cosmetic, not a reason to fail the install.
fn sweep_legacy_backups(settings_path: &Path, keep: &Path) {
    let Some(dir) = settings_path.parent() else {
        return;
    };
    let Some(file_name) = settings_path.file_name().and_then(|s| s.to_str()) else {
        return;
    };
    let prefix = format!("{file_name}.bak-");
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p == keep {
            continue;
        }
        if p.file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|n| n.starts_with(&prefix))
        {
            let _ = fs::remove_file(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    //! Unit tests for the settings.json merge + statusline + legacy
    //! cleanup pipeline. Every test uses `tempfile::tempdir()` — never
    //! touch the real `~/.claude/settings.json`.

    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn fresh_settings() -> Value {
        Value::Object(serde_json::Map::new())
    }

    /// Test-only install root with no `bin/hsp` inside — keeps hsp
    /// detection deterministic regardless of the developer's `$HOME`.
    fn sandbox_root(tmp: &std::path::Path) -> std::path::PathBuf {
        let root = tmp.join("hoangsa-root");
        std::fs::create_dir_all(root.join("bin")).expect("mkdir root/bin");
        root
    }

    #[test]
    fn merge_empty_settings() {
        let tmp = tempdir().expect("tempdir");
        let root = sandbox_root(tmp.path());
        let mut settings = fresh_settings();
        let added =
            merge_hoangsa_hooks(&mut settings, &build_hoangsa_hooks_inner(Some(&root)));
        // 2 SessionStart + 2 Stop + 1 PostToolUse + 2 PreToolUse + 1 PreCompact + 1 SessionEnd = 9
        assert_eq!(added, 9, "fresh merge lands every managed entry");
        let hooks = settings.get("hooks").and_then(|h| h.as_object()).expect("hooks present");
        assert!(hooks.contains_key("SessionStart"));
        assert!(hooks.contains_key("Stop"));
        assert!(hooks.contains_key("PreToolUse"));
        let pre = hooks.get("PreToolUse").and_then(|v| v.as_array()).expect("PreToolUse array");
        assert_eq!(pre.len(), 2);
    }

    #[test]
    fn preserve_user_hooks() {
        let tmp = tempdir().expect("tempdir");
        let root = sandbox_root(tmp.path());

        // Seed a user-authored PreToolUse hook that has nothing to do with us.
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{ "type": "command", "command": "/usr/local/bin/custom-guard" }]
                }]
            }
        });

        merge_hoangsa_hooks(&mut settings, &build_hoangsa_hooks_inner(Some(&root)));

        let pre = settings["hooks"]["PreToolUse"]
            .as_array()
            .expect("PreToolUse array");
        // 1 user entry + 2 HOANGSA entries
        assert_eq!(pre.len(), 3, "user entry preserved alongside ours");
        let user_present = pre.iter().any(|e| {
            e.get("hooks")
                .and_then(|h| h.as_array())
                .and_then(|a| a.first())
                .and_then(|h0| h0.get("command"))
                .and_then(|c| c.as_str())
                == Some("/usr/local/bin/custom-guard")
        });
        assert!(user_present, "user hook must survive merge");
    }

    #[test]
    fn dedupe_on_rerun() {
        let tmp = tempdir().expect("tempdir");
        let root = sandbox_root(tmp.path());
        let mut settings = fresh_settings();

        let first = merge_hoangsa_hooks(&mut settings, &build_hoangsa_hooks_inner(Some(&root)));
        let second = merge_hoangsa_hooks(&mut settings, &build_hoangsa_hooks_inner(Some(&root)));

        assert_eq!(first, 9);
        assert_eq!(second, 9, "re-merge re-adds the same set (replacing ours)");

        // Total entries across events stays at 9 — never doubles.
        let hooks = settings.get("hooks").and_then(|h| h.as_object()).expect("hooks");
        let total: usize = hooks
            .values()
            .filter_map(|v| v.as_array())
            .map(|a| a.len())
            .sum();
        assert_eq!(total, 9, "rerunning must not duplicate HOANGSA entries");
    }

    #[test]
    fn registers_hsp_hook_when_binary_present() {
        let tmp = tempdir().expect("tempdir");
        let root = sandbox_root(tmp.path());
        // Plant a fake hsp binary so build_hoangsa_hooks_inner detects it.
        std::fs::write(root.join("bin").join("hsp"), b"#!/bin/sh\n").expect("write hsp");

        let payload = build_hoangsa_hooks_inner(Some(&root));
        let pre = payload["PreToolUse"].as_array().expect("PreToolUse array");
        assert_eq!(pre.len(), 3, "lesson-guard + enforce + hsp rewrite");

        let hsp_entry = pre.iter().find(|e| {
            e["hooks"][0]["command"]
                .as_str()
                .is_some_and(|c| c.contains(HSP_MARKER))
        });
        assert!(hsp_entry.is_some(), "hsp rewrite entry must be registered");
        assert_eq!(hsp_entry.unwrap()["matcher"], "Bash");
    }

    #[test]
    fn strips_standalone_hsp_entry_on_merge() {
        let tmp = tempdir().expect("tempdir");
        let root = sandbox_root(tmp.path());
        // No hsp binary inside root — builder will NOT emit its own entry.

        // Seed settings with the entry `hsp init` would leave behind.
        let mut settings = json!({
            "hooks": {
                "PreToolUse": [{
                    "matcher": "Bash",
                    "hooks": [{
                        "type": "command",
                        "command": format!("hsp hook rewrite {HSP_MARKER}")
                    }]
                }]
            }
        });

        merge_hoangsa_hooks(&mut settings, &build_hoangsa_hooks_inner(Some(&root)));

        let pre = settings["hooks"]["PreToolUse"]
            .as_array()
            .expect("PreToolUse array");
        // Only our 2 PreToolUse hooks survive — prior hsp entry was claimed
        // as hoangsa-managed (via HSP_MARKER) and stripped.
        assert_eq!(pre.len(), 2, "standalone hsp entry must be stripped");
        let leftover_hsp = pre.iter().any(|e| {
            e["hooks"][0]["command"]
                .as_str()
                .is_some_and(|c| c.contains(HSP_MARKER))
        });
        assert!(!leftover_hsp, "no hsp marker should remain");
    }

    #[test]
    fn statusline_preserves_user_custom() {
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join(".claude");

        let mut settings = json!({
            "statusLine": { "type": "command", "command": "/my/custom/bar" }
        });
        let wrote = apply_statusline(&mut settings, &default_statusline(&target));
        assert!(!wrote, "user statusLine must be preserved");
        assert_eq!(
            settings["statusLine"]["command"].as_str(),
            Some("/my/custom/bar")
        );

        // Empty settings → we write the default.
        let mut empty = fresh_settings();
        let wrote2 = apply_statusline(&mut empty, &default_statusline(&target));
        assert!(wrote2, "default statusLine applied on empty settings");
        assert!(empty["statusLine"]["command"].is_string());
    }

    #[test]
    fn statusline_overwrites_stale_managed_path() {
        // Simulates the regression: a previous install with a temp-dir
        // HOANGSA_INSTALL_DIR wrote `/tmp/.../hoangsa-cli hook statusline`,
        // tmp dir was cleaned, later installs preserved the broken value
        // and CC silently rendered nothing.
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join(".claude");
        let bogus = tmp.path().join("vanished").join("hoangsa-cli");
        assert!(!bogus.exists());

        let mut settings = json!({
            "statusLine": {
                "type": "command",
                "command": format!("{} hook statusline", bogus.display()),
                "padding": 0,
            }
        });
        let wrote = apply_statusline(&mut settings, &default_statusline(&target));
        assert!(wrote, "stale managed statusLine must be overwritten");
        assert_ne!(
            settings["statusLine"]["command"].as_str(),
            Some(format!("{} hook statusline", bogus.display()).as_str()),
            "command should no longer point to the vanished bin"
        );
    }

    #[test]
    fn statusline_keeps_managed_when_binary_present() {
        // Same signature as ours, but the binary path is real — preserve it.
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join(".claude");
        let real_bin = tmp.path().join("hoangsa-cli");
        std::fs::write(&real_bin, b"#!/bin/sh\n").expect("write fake bin");

        let cmd = format!("{} hook statusline", real_bin.display());
        let mut settings = json!({
            "statusLine": { "type": "command", "command": cmd.clone(), "padding": 0 }
        });
        let wrote = apply_statusline(&mut settings, &default_statusline(&target));
        assert!(!wrote, "valid managed statusLine must be preserved");
        assert_eq!(settings["statusLine"]["command"].as_str(), Some(cmd.as_str()));
    }

    #[test]
    fn statusline_does_not_touch_non_managed_even_if_path_missing() {
        // User pointed at a custom script that doesn't exist yet — we
        // must NOT silently rewrite a foreign command.
        let tmp = tempdir().expect("tempdir");
        let target = tmp.path().join(".claude");

        let mut settings = json!({
            "statusLine": { "type": "command", "command": "/nope/custom-bar.sh" }
        });
        let wrote = apply_statusline(&mut settings, &default_statusline(&target));
        assert!(!wrote, "non-managed statusLine must be preserved unconditionally");
        assert_eq!(
            settings["statusLine"]["command"].as_str(),
            Some("/nope/custom-bar.sh")
        );
    }

    #[test]
    fn load_missing_returns_empty_object() {
        let tmp = tempdir().expect("tempdir");
        let v = load_settings(&tmp.path().join("nope.json")).expect("load");
        assert!(v.is_object());
        assert!(v.as_object().expect("object").is_empty());
    }

    #[test]
    fn load_settings_corrupt_returns_err() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("settings.json");
        // Invalid JSON — previously this silently became `{}` and the
        // installer wrote HOANGSA hooks on top of the empty shell,
        // effectively nuking the (uninspected) user config.
        std::fs::write(&path, "{ broken: true,").expect("write corrupt");
        let err = load_settings(&path).expect_err("corrupt settings must error");
        assert!(
            err.to_string().contains("parse settings.json"),
            "error should mention parse failure; got: {err}"
        );
    }

    #[test]
    fn save_roundtrip_preserves_two_space_indent() {
        let tmp = tempdir().expect("tempdir");
        let p = tmp.path().join("settings.json");
        let v = json!({ "a": { "b": 1 } });
        save_settings(&p, &v).expect("save");
        let raw = std::fs::read_to_string(&p).expect("read");
        assert!(raw.contains("  \"a\""), "expected 2-space indent, got: {raw}");
        assert!(raw.ends_with('\n'), "expected trailing newline");
        let back = load_settings(&p).expect("load back");
        assert_eq!(back, v);
    }

    #[test]
    fn backup_skips_missing_source() {
        let tmp = tempdir().expect("tempdir");
        let result = backup_settings(&tmp.path().join("absent.json")).expect("backup");
        assert!(result.is_none(), "missing source must not create a backup");
    }

    #[test]
    fn backup_overwrites_and_sweeps_legacy_stamped_files() {
        let tmp = tempdir().expect("tempdir");
        let settings = tmp.path().join("settings.json");
        std::fs::write(&settings, b"{\"v\":1}").expect("seed settings");
        // Two stale timestamped backups from a previous installer version.
        let legacy_a = tmp.path().join("settings.json.bak-20250101-000000");
        let legacy_b = tmp.path().join("settings.json.bak-20260101-120000");
        std::fs::write(&legacy_a, b"old").expect("seed legacy a");
        std::fs::write(&legacy_b, b"older").expect("seed legacy b");
        // Unrelated sibling — must not be deleted.
        let unrelated = tmp.path().join("other.json.bak-20260101-120000");
        std::fs::write(&unrelated, b"keep").expect("seed unrelated");

        let out = backup_settings(&settings).expect("backup").expect("path");
        assert_eq!(out, tmp.path().join("settings.json.bak"));
        assert_eq!(std::fs::read(&out).expect("read bak"), b"{\"v\":1}");
        assert!(!legacy_a.exists(), "legacy bak-<stamp> must be swept");
        assert!(!legacy_b.exists(), "legacy bak-<stamp> must be swept");
        assert!(unrelated.exists(), "unrelated sibling must survive");

        // Second run overwrites the single .bak with fresh contents.
        std::fs::write(&settings, b"{\"v\":2}").expect("update settings");
        backup_settings(&settings).expect("backup 2");
        assert_eq!(std::fs::read(&out).expect("read bak 2"), b"{\"v\":2}");
    }
}
