// ───────────────────────── uninstall ─────────────────────────
//
// Removes everything `install` wrote. Ported from `scripts/uninstall.sh`,
// which until now was the only supported removal path — and it lives in the
// repo, so anyone who installed with `curl | sh` had to clone the source
// before they could uninstall. A tool you cannot remove without fetching its
// source is a tool that overstayed its welcome.
//
// The port is not a translation. Two behaviours change, both because Rust has
// what POSIX sh does not:
//
//   * The shell version needs `jq` to touch settings.json and the MCP config,
//     and warns-and-skips when it is absent. That is a fail-open in an
//     uninstaller: it reports success while leaving hook entries pointing at a
//     binary it just deleted, so every later session fires hooks that cannot
//     run. serde_json is compiled in, so this path cannot be skipped.
//   * Removing the running binary is allowed here. On Unix the inode survives
//     until the process exits, so `hoangsa-cli uninstall` deleting
//     `hoangsa-cli` completes normally. It reads like a bug; it is not.
//
// What is preserved without `--purge`: `~/.hoangsa/memory/` (the long-term
// store), `~/.hoangsa/share/`, and every per-project `.hoangsa/` — those hold
// user work that has nothing to do with whether the tool is installed.

use crate::helpers::out;
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const MARK_START: &str = "# hoangsa:managed start";
const MARK_END: &str = "# hoangsa:managed end";

/// Binaries the installer places, relative to the dir each one lives in.
const CLI_BINS: &[&str] = &["hoangsa-cli", "hsp"];
const INSTALL_BINS: &[&str] = &["hoangsa-memory", "hoangsa-memory-mcp", "hoangsa-ui"];

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Flags {
    pub global: bool,
    pub local: bool,
    pub dry_run: bool,
    pub purge: bool,
    pub yes: bool,
}

pub fn parse_flags(args: &[&str]) -> Result<Flags, String> {
    let mut f = Flags::default();
    for a in args {
        match *a {
            "--global" => f.global = true,
            "--local" => f.local = true,
            "--dry-run" => f.dry_run = true,
            "--purge" => f.purge = true,
            "--yes" | "-y" => f.yes = true,
            other => return Err(format!("unknown flag: {other} (try --help)")),
        }
    }
    if f.global && f.local {
        return Err("--global and --local are mutually exclusive".into());
    }
    if !f.global && !f.local {
        return Err("must specify --global or --local (try --help)".into());
    }
    if f.purge && !f.global {
        return Err("--purge requires --global".into());
    }
    Ok(f)
}

/// Every mutation routes through one record so `--dry-run` has a single gate
/// and the JSON report lists exactly what a real run would have done.
#[derive(Debug, Default)]
pub struct Actions {
    pub removed: Vec<String>,
    pub edited: Vec<String>,
    pub skipped: Vec<String>,
}

impl Actions {
    fn removed(&mut self, p: &Path) {
        self.removed.push(p.display().to_string());
    }
    fn edited(&mut self, p: &Path, what: &str) {
        self.edited.push(format!("{}: {what}", p.display()));
    }
    fn skipped(&mut self, why: String) {
        self.skipped.push(why);
    }
}

pub fn cmd_uninstall(args: &[&str], cwd: &Path) {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("uninstall: {e}");
            std::process::exit(2);
        }
    };

    let install_dir = install_dir();
    let cli_dir = cli_dir(&install_dir);
    let mut acts = Actions::default();

    let (dst_root, settings, mcp) = if flags.global {
        let dst = pick_config_dir();
        let mcp = match std::env::var("CLAUDE_CONFIG_DIR") {
            Ok(d) if !d.is_empty() => PathBuf::from(d).join(".claude.json"),
            _ => home().join(".claude.json"),
        };
        (dst.clone(), dst.join("settings.json"), mcp)
    } else {
        let dst = cwd.join(".claude");
        (
            dst.clone(),
            dst.join("settings.json"),
            cwd.join(".mcp.json"),
        )
    };

    let manifest = install_dir.join("manifest.json");
    remove_templates(&manifest, &dst_root, flags.dry_run, &mut acts);
    strip_managed_hooks(&settings, flags.dry_run, &mut acts);
    strip_mcp_entry(&mcp, flags.dry_run, &mut acts);

    for b in CLI_BINS {
        rm_file(&cli_dir.join(b), flags.dry_run, &mut acts);
    }
    for b in INSTALL_BINS {
        rm_file(&install_dir.join("bin").join(b), flags.dry_run, &mut acts);
    }

    if flags.global {
        remove_fastembed_cache(&install_dir, flags.dry_run, &mut acts);
        strip_rc_files(flags.dry_run, &mut acts);
    }

    rmdir_if_empty(&cli_dir, flags.dry_run, &mut acts);
    rmdir_if_empty(&install_dir.join("bin"), flags.dry_run, &mut acts);

    let mut purged = false;
    if flags.purge {
        purged = purge(&install_dir, flags.dry_run, flags.yes, &mut acts);
    }

    out(&json!({
        "status": "ok",
        "mode": if flags.global { "global" } else { "local" },
        "dry_run": flags.dry_run,
        "purged": purged,
        "removed": acts.removed,
        "edited": acts.edited,
        "skipped": acts.skipped,
        "note": if flags.global && !flags.dry_run {
            "open a new shell (or source your rc file) so PATH changes take effect"
        } else { "" },
    }));
}

// ─── paths ───────────────────────────────────────────────────────────────────

fn home() -> PathBuf {
    std::env::var("HOME").map(PathBuf::from).unwrap_or_default()
}

fn install_dir() -> PathBuf {
    match std::env::var("HOANGSA_INSTALL_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => home().join(".hoangsa"),
    }
}

fn cli_dir(install: &Path) -> PathBuf {
    match std::env::var("HOANGSA_CLI_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => install.join("bin"),
    }
}

/// The dir to clean. `CLAUDE_CONFIG_DIR` wins; otherwise the first candidate
/// that exists. Guessing wrong here leaves a full install behind while
/// reporting success, so an existing dir beats the conventional default.
fn pick_config_dir() -> PathBuf {
    let candidates = crate::cmd::addon::claude_config_dirs();
    candidates
        .iter()
        .find(|d| d.exists())
        .cloned()
        .or_else(|| candidates.first().cloned())
        .unwrap_or_else(|| home().join(".claude"))
}

// ─── primitives ──────────────────────────────────────────────────────────────

fn rm_file(p: &Path, dry: bool, acts: &mut Actions) {
    if !p.exists() && p.symlink_metadata().is_err() {
        return;
    }
    if dry {
        acts.removed(p);
        return;
    }
    match fs::remove_file(p) {
        Ok(()) => acts.removed(p),
        Err(e) => acts.skipped(format!("{}: {e}", p.display())),
    }
}

fn rmdir_if_empty(d: &Path, dry: bool, acts: &mut Actions) {
    let empty = fs::read_dir(d)
        .map(|mut it| it.next().is_none())
        .unwrap_or(false);
    if !empty {
        return;
    }
    if dry {
        acts.removed(d);
        return;
    }
    if fs::remove_dir(d).is_ok() {
        acts.removed(d);
    }
}

/// Rewrite through a sibling temp file, then rename. A partial write to
/// settings.json costs the user their whole Claude Code configuration, and
/// this command exists to leave the machine tidy.
fn write_atomic(path: &Path, body: &str) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("hoangsa.tmp.{}", std::process::id()));
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(body.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)
}

// ─── 1. templates, via the manifest ──────────────────────────────────────────

/// The manifest's `files` keys are forward-slash paths relative to the
/// TEMPLATE SOURCE, not to the config dir — `copy_templates` records the
/// source path and routes the destination separately. So every key must go
/// back through `route_rel` before it means anything on disk.
///
/// Entries that are absolute or contain `..` are refused rather than
/// resolved: a manifest is a file on disk, and an uninstaller that follows one
/// out of its target directory is an arbitrary-delete primitive.
pub fn manifest_entries(manifest_json: &str) -> (Vec<String>, Vec<String>) {
    let mut ok = Vec::new();
    let mut refused = Vec::new();
    let v: Value = match serde_json::from_str(manifest_json) {
        Ok(v) => v,
        Err(_) => return (ok, refused),
    };
    let Some(files) = v.get("files").and_then(|f| f.as_object()) else {
        return (ok, refused);
    };
    for key in files.keys() {
        let suspicious = key.is_empty()
            || key.starts_with('/')
            || key.starts_with('\\')
            || key.split('/').any(|seg| seg == "..")
            || key.contains(':');
        if suspicious {
            refused.push(key.clone());
        } else {
            ok.push(key.clone());
        }
    }
    ok.sort();
    (ok, refused)
}

/// A source-relative manifest key, mapped to where the installer actually
/// wrote it. Delegates to `install::templates::route_rel` so the two can
/// never disagree about the layout.
pub fn routed_rel(rel: &str) -> String {
    crate::cmd::install::templates::route_rel(Path::new(rel))
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every parent directory of every tracked file, deepest first, so pruning
/// cascades: removing `commands/hoangsa/x.md` should be able to take
/// `commands/hoangsa` and then `commands` when nothing else lives there.
pub fn parent_dirs_deepest_first(rels: &[String]) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    for rel in rels {
        let parts: Vec<&str> = rel.split('/').collect();
        for i in 1..parts.len() {
            dirs.push(parts[..i].join("/"));
        }
    }
    dirs.sort();
    dirs.dedup();
    dirs.sort_by(|a, b| b.cmp(a));
    dirs
}

fn remove_templates(manifest: &Path, dst_root: &Path, dry: bool, acts: &mut Actions) {
    let body = match fs::read_to_string(manifest) {
        Ok(b) => b,
        Err(_) => {
            acts.skipped(format!(
                "no manifest at {} — templates were not tracked and stay put",
                manifest.display()
            ));
            return;
        }
    };
    let (rels, refused) = manifest_entries(&body);
    for r in refused {
        acts.skipped(format!("suspicious manifest entry refused: {r}"));
    }
    if rels.is_empty() {
        acts.skipped("manifest tracks no files".into());
    }
    let routed: Vec<String> = rels.iter().map(|r| routed_rel(r)).collect();
    for rel in &routed {
        rm_file(&dst_root.join(rel), dry, acts);
    }
    for d in parent_dirs_deepest_first(&routed) {
        rmdir_if_empty(&dst_root.join(d), dry, acts);
    }
    rm_file(manifest, dry, acts);
}

// ─── 2. settings.json hooks ──────────────────────────────────────────────────

/// Drop every hook object carrying `_hoangsa_managed: true`, then drop hook
/// arrays that ended up empty, then drop `hooks` if nothing is left. Returns
/// `None` when the document needs no change, so an untouched file is never
/// rewritten.
pub fn strip_hooks(doc: &Value) -> Option<Value> {
    let obj = doc.as_object()?;
    let hooks = obj.get("hooks")?.as_object()?;

    let mut kept = serde_json::Map::new();
    let mut dropped = 0usize;
    for (event, arr) in hooks {
        let Some(items) = arr.as_array() else {
            kept.insert(event.clone(), arr.clone());
            continue;
        };
        let survivors: Vec<Value> = items
            .iter()
            .filter(|item| {
                let managed = item
                    .get("_hoangsa_managed")
                    .and_then(|m| m.as_bool())
                    .unwrap_or(false);
                if managed {
                    dropped += 1;
                }
                !managed
            })
            .cloned()
            .collect();
        if !survivors.is_empty() {
            kept.insert(event.clone(), Value::Array(survivors));
        }
    }
    if dropped == 0 {
        return None;
    }

    let mut new_doc = obj.clone();
    if kept.is_empty() {
        new_doc.remove("hooks");
    } else {
        new_doc.insert("hooks".into(), Value::Object(kept));
    }
    Some(Value::Object(new_doc))
}

fn strip_managed_hooks(settings: &Path, dry: bool, acts: &mut Actions) {
    let Ok(body) = fs::read_to_string(settings) else {
        return;
    };
    let doc: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            acts.skipped(format!(
                "{}: not valid JSON ({e}) — left alone",
                settings.display()
            ));
            return;
        }
    };
    let Some(new_doc) = strip_hooks(&doc) else {
        return;
    };
    if dry {
        acts.edited(settings, "would strip managed hooks");
        return;
    }
    let pretty = serde_json::to_string_pretty(&new_doc).unwrap_or(body);
    match write_atomic(settings, &pretty) {
        Ok(()) => acts.edited(settings, "stripped managed hooks"),
        Err(e) => acts.skipped(format!("{}: {e}", settings.display())),
    }
}

// ─── 3. MCP registration ─────────────────────────────────────────────────────

pub fn strip_mcp(doc: &Value) -> Option<Value> {
    let obj = doc.as_object()?;
    let servers = obj.get("mcpServers")?.as_object()?;
    if !servers.contains_key("hoangsa-memory") {
        return None;
    }
    let mut rest = servers.clone();
    rest.remove("hoangsa-memory");
    let mut new_doc = obj.clone();
    if rest.is_empty() {
        new_doc.remove("mcpServers");
    } else {
        new_doc.insert("mcpServers".into(), Value::Object(rest));
    }
    Some(Value::Object(new_doc))
}

fn strip_mcp_entry(mcp: &Path, dry: bool, acts: &mut Actions) {
    let Ok(body) = fs::read_to_string(mcp) else {
        return;
    };
    let doc: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            acts.skipped(format!(
                "{}: not valid JSON ({e}) — left alone",
                mcp.display()
            ));
            return;
        }
    };
    let Some(new_doc) = strip_mcp(&doc) else {
        return;
    };
    if dry {
        acts.edited(mcp, "would remove mcpServers.hoangsa-memory");
        return;
    }
    let pretty = serde_json::to_string_pretty(&new_doc).unwrap_or(body);
    match write_atomic(mcp, &pretty) {
        Ok(()) => acts.edited(mcp, "removed mcpServers.hoangsa-memory"),
        Err(e) => acts.skipped(format!("{}: {e}", mcp.display())),
    }
}

// ─── 4. rc file PATH block ───────────────────────────────────────────────────

/// Remove the marker-delimited block and nothing else. An unterminated start
/// marker means a hand-edited file: bail rather than truncate everything
/// after it, which is what a naive "delete from marker to EOF" would do.
pub fn strip_managed_block(rc: &str) -> Option<String> {
    if !rc.contains(MARK_START) {
        return None;
    }
    if !rc.contains(MARK_END) {
        return None;
    }
    let kept: Vec<&str> = {
        let mut inside = false;
        rc.lines()
            .filter(|line| {
                if line.contains(MARK_START) {
                    inside = true;
                    return false;
                }
                if line.contains(MARK_END) {
                    inside = false;
                    return false;
                }
                !inside
            })
            .collect()
    };
    Some(kept.join("\n"))
}

fn strip_rc_files(dry: bool, acts: &mut Actions) {
    if std::env::var("HOANGSA_NO_PATH_EDIT").as_deref() == Ok("1") {
        acts.skipped("HOANGSA_NO_PATH_EDIT=1 — rc files left alone".into());
        return;
    }
    let h = home();
    for rc in [
        h.join(".zshrc"),
        h.join(".bashrc"),
        h.join(".bash_profile"),
        h.join(".config/fish/config.fish"),
    ] {
        let Ok(body) = fs::read_to_string(&rc) else {
            continue;
        };
        if body.contains(MARK_START) && !body.contains(MARK_END) {
            acts.skipped(format!(
                "{}: managed block has no end marker — left alone",
                rc.display()
            ));
            continue;
        }
        let Some(new_body) = strip_managed_block(&body) else {
            continue;
        };
        if dry {
            acts.edited(&rc, "would strip managed PATH block");
            continue;
        }
        match write_atomic(&rc, &new_body) {
            Ok(()) => acts.edited(&rc, "stripped managed PATH block"),
            Err(e) => acts.skipped(format!("{}: {e}", rc.display())),
        }
    }
}

// ─── 5. fastembed cache + purge ──────────────────────────────────────────────

fn remove_fastembed_cache(install: &Path, dry: bool, acts: &mut Actions) {
    let cache = match std::env::var("FASTEMBED_CACHE_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => install.join("cache/fastembed"),
    };
    if !cache.is_dir() {
        return;
    }
    if dry {
        acts.removed(&cache);
    } else if fs::remove_dir_all(&cache).is_ok() {
        acts.removed(&cache);
        rmdir_if_empty(&install.join("cache"), dry, acts);
    }
    // Clear the opt-out marker so a later reinstall starts from a clean
    // decision rather than inheriting one the user made months ago.
    rm_file(&install.join("no-embed"), dry, acts);
}

fn purge(install: &Path, dry: bool, yes: bool, acts: &mut Actions) -> bool {
    if !install.is_dir() {
        return false;
    }
    if dry {
        acts.removed(install);
        return true;
    }
    if !yes && std::io::IsTerminal::is_terminal(&std::io::stdin()) {
        eprint!(
            "--purge deletes {} including memory/. Continue? [y/N] ",
            install.display()
        );
        let _ = std::io::stderr().flush();
        let mut reply = String::new();
        if std::io::stdin().read_line(&mut reply).is_err()
            || !reply.trim_start().to_lowercase().starts_with('y')
        {
            acts.skipped("purge declined".into());
            return false;
        }
    }
    match fs::remove_dir_all(install) {
        Ok(()) => {
            acts.removed(install);
            true
        }
        Err(e) => {
            acts.skipped(format!("{}: {e}", install.display()));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_conflicting_and_missing_modes() {
        assert!(parse_flags(&["--global", "--local"]).is_err());
        assert!(parse_flags(&[]).is_err());
        assert!(parse_flags(&["--purge", "--local"]).is_err());
        assert!(parse_flags(&["--bogus"]).is_err());
        assert_eq!(
            parse_flags(&["--global", "--dry-run"]).expect("valid"),
            Flags {
                global: true,
                dry_run: true,
                ..Default::default()
            }
        );
    }

    /// A manifest is a file on disk. An uninstaller that joins an unchecked
    /// key onto the config dir will delete whatever the key points at.
    #[test]
    fn refuses_manifest_entries_that_escape_the_target() {
        let m = r#"{"files":{
            "agents/ok.md":"h",
            "../../../etc/passwd":"h",
            "/etc/hosts":"h",
            "a/../../b":"h",
            "":"h"
        }}"#;
        let (ok, refused) = manifest_entries(m);
        assert_eq!(ok, vec!["agents/ok.md".to_string()]);
        assert_eq!(refused.len(), 4, "refused: {refused:?}");
    }

    /// The manifest is keyed by SOURCE path and the installer routes the
    /// destination. Joining a key straight onto the config dir looks for
    /// files that were never written there — `scripts/uninstall.sh` did
    /// exactly that and left 71 of 99 tracked files on disk while printing
    /// "uninstall complete".
    #[test]
    fn manifest_keys_route_to_where_the_installer_wrote_them() {
        assert_eq!(routed_rel("workflows/cook.md"), "hoangsa/workflows/cook.md");
        assert_eq!(
            routed_rel("skills/hoangsa/git-flow/SKILL.md"),
            "skills/git-flow/SKILL.md"
        );
        assert_eq!(
            routed_rel("commands/hoangsa/cook.md"),
            "commands/hoangsa/cook.md"
        );
        assert_eq!(
            routed_rel("agents/hoangsa-reviewer.md"),
            "agents/hoangsa-reviewer.md"
        );
    }

    #[test]
    fn parents_are_deepest_first_so_pruning_cascades() {
        let rels = vec![
            "commands/hoangsa/cook.md".to_string(),
            "agents/x.md".to_string(),
        ];
        let dirs = parent_dirs_deepest_first(&rels);
        let hoangsa = dirs.iter().position(|d| d == "commands/hoangsa").unwrap();
        let commands = dirs.iter().position(|d| d == "commands").unwrap();
        assert!(hoangsa < commands, "child must precede parent: {dirs:?}");
        assert!(dirs.contains(&"agents".to_string()));
    }

    #[test]
    fn strips_only_managed_hooks_and_keeps_the_rest() {
        let doc: Value = serde_json::from_str(
            r#"{"model":"opus","hooks":{
                "PreToolUse":[{"_hoangsa_managed":true,"x":1},{"mine":true}],
                "Stop":[{"_hoangsa_managed":true}]
            }}"#,
        )
        .unwrap();
        let new = strip_hooks(&doc).expect("changed");
        assert_eq!(new["model"], "opus");
        assert_eq!(new["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(new["hooks"]["PreToolUse"][0]["mine"], true);
        assert!(
            new["hooks"].get("Stop").is_none(),
            "an array emptied by the strip must be dropped, not left as []"
        );
    }

    #[test]
    fn leaves_a_file_with_no_managed_hooks_untouched() {
        let doc: Value = serde_json::from_str(r#"{"hooks":{"Stop":[{"mine":true}]}}"#).unwrap();
        assert!(
            strip_hooks(&doc).is_none(),
            "no change means no rewrite — rewriting reformats a file we do not own"
        );
        let empty: Value = serde_json::from_str(r#"{"model":"opus"}"#).unwrap();
        assert!(strip_hooks(&empty).is_none());
    }

    #[test]
    fn drops_the_hooks_key_when_the_strip_empties_it() {
        let doc: Value =
            serde_json::from_str(r#"{"a":1,"hooks":{"Stop":[{"_hoangsa_managed":true}]}}"#)
                .unwrap();
        let new = strip_hooks(&doc).expect("changed");
        assert!(new.get("hooks").is_none(), "got {new}");
        assert_eq!(new["a"], 1);
    }

    #[test]
    fn removes_only_our_mcp_server() {
        let doc: Value = serde_json::from_str(
            r#"{"mcpServers":{"hoangsa-memory":{"command":"x"},"other":{"command":"y"}}}"#,
        )
        .unwrap();
        let new = strip_mcp(&doc).expect("changed");
        assert!(new["mcpServers"].get("hoangsa-memory").is_none());
        assert!(new["mcpServers"].get("other").is_some());

        let only_ours: Value =
            serde_json::from_str(r#"{"mcpServers":{"hoangsa-memory":{}}}"#).unwrap();
        let new = strip_mcp(&only_ours).expect("changed");
        assert!(new.get("mcpServers").is_none());

        let none: Value = serde_json::from_str(r#"{"mcpServers":{"other":{}}}"#).unwrap();
        assert!(strip_mcp(&none).is_none());
    }

    #[test]
    fn strips_the_marked_block_and_nothing_else() {
        let rc = format!("export A=1\n{MARK_START}\nexport PATH=x\n{MARK_END}\nexport B=2\n");
        let out = strip_managed_block(&rc).expect("changed");
        assert_eq!(out, "export A=1\nexport B=2");
    }

    /// A start marker with no end means somebody edited the file by hand.
    /// Deleting from the marker to EOF would take their whole rc with it.
    #[test]
    fn refuses_an_unterminated_managed_block() {
        let rc = format!("export A=1\n{MARK_START}\nexport PATH=x\nexport B=2\n");
        assert!(strip_managed_block(&rc).is_none());
        assert!(strip_managed_block("export A=1\n").is_none());
    }
}
