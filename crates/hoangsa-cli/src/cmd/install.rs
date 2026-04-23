use crate::helpers;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;
use time::macros::format_description;

/// CLI version stamped into the manifest. Pulled from Cargo at compile time.
const CLI_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Resolve the user's home directory via `$HOME` without pulling the `dirs`
/// crate. Shared by `templates`, `hooks`, `relocate`, and `install_dst_dir`
/// so every home-anchored path in the installer agrees on the same root.
fn home_path() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "cannot resolve $HOME".to_string())
}

/// Resolve the Claude Code config directory — the parent of
/// `skills/`, `commands/`, `agents/`, `hoangsa/`, and `settings.json`.
///
/// Honors `CLAUDE_CONFIG_DIR` (respected by upstream Claude Code; typically
/// set via a shell alias like `zclaude='CLAUDE_CONFIG_DIR=~/.zclaude claude'`)
/// so that installs aimed at an alternate Claude profile actually land there.
/// Falls back to `$HOME/.claude` when the env var is unset or empty.
///
/// Tilde-expansion: we accept a leading `~/` because a verbatim forwarded env
/// value (e.g. `CLAUDE_CONFIG_DIR=~/.zclaude` written in an alias that then
/// gets re-exported) may arrive unexpanded. POSIX shells only tilde-expand
/// assignments made as standalone statements, not ones propagated through
/// nested invocations.
fn claude_config_dir() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        let s = raw.to_string_lossy().into_owned();
        if !s.is_empty() {
            if s == "~" {
                return home_path();
            }
            if let Some(rest) = s.strip_prefix("~/") {
                return Ok(home_path()?.join(rest));
            }
            return Ok(PathBuf::from(s));
        }
    }
    Ok(home_path()?.join(".claude"))
}

/// Path to the Claude Code global MCP config file (`.claude.json`).
///
/// Upstream layout: without `CLAUDE_CONFIG_DIR`, the file sits at `$HOME/.claude.json`
/// (NOT inside `$HOME/.claude/`). When `CLAUDE_CONFIG_DIR` is set, the file
/// moves inside that dir — `$CLAUDE_CONFIG_DIR/.claude.json`. We match that
/// shape so zclaude-style installs write to the same path the zclaude session
/// reads.
fn claude_json_path() -> Result<PathBuf, String> {
    match std::env::var_os("CLAUDE_CONFIG_DIR") {
        Some(raw) if !raw.is_empty() => Ok(claude_config_dir()?.join(".claude.json")),
        _ => Ok(home_path()?.join(".claude.json")),
    }
}

/// Root directory for the installed `hoangsa-memory` tree (bins + manifest).
/// Honors the `HOANGSA_INSTALL_DIR` env var (set by `scripts/install.sh`) so
/// a user who points the installer at a non-default location still gets MCP
/// entries whose `command` field matches where the bins actually landed.
/// Falls back to `$HOME/.hoangsa` when the env var is unset or empty.
fn memory_install_dir() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("HOANGSA_INSTALL_DIR") {
        let s = raw.to_string_lossy().into_owned();
        if !s.is_empty() {
            return Ok(PathBuf::from(s));
        }
    }
    Ok(home_path()?.join(".hoangsa"))
}

/// Compact `YYYYMMDD-HHMMSS` UTC stamp used as a suffix for template patch
/// backups. `settings.json` uses a single stable `.bak` instead — see
/// [`hooks::backup_settings`].
fn backup_timestamp() -> String {
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .unwrap_or_else(|_| String::from("00000000-000000"))
}

/// Parsed install flags. Kept in one struct so later tasks (T-04/T-05/T-06)
/// can extend without touching the parser skeleton.
#[derive(Debug, Default)]
struct InstallFlags {
    global: bool,
    local: bool,
    dry_run: bool,
    no_memory: bool,
    skip_path_edit: bool,
    /// Value of `--task-manager[=<clickup|asana|none>]`; None when not provided.
    task_manager: Option<String>,
}

fn parse_flags(args: &[&str]) -> Result<InstallFlags, String> {
    let mut f = InstallFlags::default();
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        match a {
            "--global" => f.global = true,
            "--local" => f.local = true,
            "--dry-run" => f.dry_run = true,
            "--no-memory" => f.no_memory = true,
            "--skip-path-edit" => f.skip_path_edit = true,
            "--task-manager" => {
                i += 1;
                if i >= args.len() {
                    return Err("--task-manager requires a value (clickup|asana|none)".into());
                }
                f.task_manager = Some(args[i].to_string());
            }
            s if s.starts_with("--task-manager=") => {
                f.task_manager = Some(s["--task-manager=".len()..].to_string());
            }
            other => return Err(format!("Unknown flag: {other}")),
        }
        i += 1;
    }
    Ok(f)
}

fn validate(f: &InstallFlags) -> Result<(), String> {
    if f.global && f.local {
        return Err("--global and --local are mutually exclusive".into());
    }
    Ok(())
}

fn mode_str(f: &InstallFlags) -> &'static str {
    if f.global {
        "global"
    } else {
        // Default mode when --global is not specified. `--local` is the only
        // other option and falls through here too.
        "local"
    }
}

// ───────────────────────── templates submodule ─────────────────────────
//
// Holds template copy + SHA256 manifest + patch-backup logic so both the
// live install flow and its unit tests can exercise it without touching
// the outer scaffold.
pub mod templates {
    use super::*;

    /// On-disk shape of `~/.hoangsa/manifest.json`. Relative paths use
    /// forward slashes for portability; hex-encoded SHA256 digests.
    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    pub struct Manifest {
        pub version: String,
        pub timestamp: String,
        pub files: BTreeMap<String, String>,
    }

    impl Manifest {
        pub fn new(version: impl Into<String>) -> Self {
            Self {
                version: version.into(),
                timestamp: now_iso(),
                files: BTreeMap::new(),
            }
        }
    }

    /// Outcome of a `copy_templates` call — always returned so callers can
    /// print a summary whether or not anything changed.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct CopyReport {
        pub copied: Vec<PathBuf>,
        pub patched_backups: Vec<PathBuf>,
        pub skipped: Vec<PathBuf>,
    }

    /// Planned action for a `--dry-run`. `src` is the template source path
    /// on disk; `dst` is where we would write; `backup` is only present for
    /// `action == "backup"`.
    #[derive(Debug, Clone, Serialize)]
    pub struct PlannedAction {
        pub action: String,
        pub src: PathBuf,
        pub dst: PathBuf,
        #[serde(skip_serializing_if = "Option::is_none")]
        pub backup: Option<PathBuf>,
    }

    /// Locate the template source directory.
    ///
    /// Precedence:
    ///   1. `$HOANGSA_TEMPLATES_DIR` env var (set by `install.sh` to the extracted tarball dir).
    ///   2. `global` mode fallback: `~/.hoangsa/share/templates`.
    ///   3. `local` mode fallback: walk up from `cwd` looking for a `templates/` dir
    ///      that sits alongside a `.hoangsa/` marker (repo root).
    pub fn templates_source_dir(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
        if let Ok(env_dir) = std::env::var("HOANGSA_TEMPLATES_DIR") {
            let p = PathBuf::from(env_dir);
            if p.is_dir() {
                return Ok(p);
            }
            return Err(format!(
                "HOANGSA_TEMPLATES_DIR is set but not a directory: {}",
                p.display()
            ));
        }
        match mode {
            "global" => {
                let home = super::home_path()?;
                let p = home.join(".hoangsa").join("share").join("templates");
                if p.is_dir() {
                    Ok(p)
                } else {
                    Err(format!(
                        "global template dir not found: {} (set HOANGSA_TEMPLATES_DIR)",
                        p.display()
                    ))
                }
            }
            _ => {
                // Walk up from cwd looking for a sibling `templates/` next to `.hoangsa/`.
                let mut cur: Option<&Path> = Some(cwd);
                while let Some(dir) = cur {
                    let templates = dir.join("templates");
                    let marker = dir.join(".hoangsa");
                    if templates.is_dir() && marker.exists() {
                        return Ok(templates);
                    }
                    cur = dir.parent();
                }
                Err(format!(
                    "could not locate templates/ starting from {} (set HOANGSA_TEMPLATES_DIR)",
                    cwd.display()
                ))
            }
        }
    }

    fn now_iso() -> String {
        OffsetDateTime::now_utc()
            .format(format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
            ))
            .unwrap_or_else(|_| String::from("1970-01-01T00:00:00Z"))
    }

    /// Compute the SHA256 digest of a file as a lowercase hex string.
    pub fn compute_file_sha256(path: &Path) -> io::Result<String> {
        let bytes = fs::read(path)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Ok(hex_encode(&hasher.finalize()))
    }

    fn hex_encode(bytes: &[u8]) -> String {
        let mut s = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            s.push_str(&format!("{:02x}", b));
        }
        s
    }

    /// Manifest loader that distinguishes "missing" (Ok(None) — fresh install)
    /// from corruption (Err — abort so we never overwrite user edits without
    /// the patch-backup gate). Other I/O errors also surface as Err so a
    /// permission issue doesn't silently masquerade as a fresh install.
    pub fn load_manifest(path: &Path) -> Result<Option<Manifest>, String> {
        match fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<Manifest>(&raw) {
                Ok(m) => Ok(Some(m)),
                Err(e) => Err(format!("parse manifest at {}: {e}", path.display())),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("read manifest at {}: {e}", path.display())),
        }
    }

    /// Write manifest as pretty JSON, creating parent dirs as needed.
    pub fn save_manifest(path: &Path, manifest: &Manifest) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(manifest).map_err(io::Error::other)?;
        fs::write(path, json)
    }

    /// Recursively list every regular file under `dir`, returning absolute paths.
    fn walk_files(dir: &Path) -> io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        walk_files_inner(dir, &mut out)?;
        out.sort();
        Ok(out)
    }

    fn walk_files_inner(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ft = entry.file_type()?;
            if ft.is_dir() {
                walk_files_inner(&path, out)?;
            } else if ft.is_file() {
                out.push(path);
            }
            // Symlinks intentionally skipped — templates are plain files.
        }
        Ok(())
    }

    /// Normalize a relative path to forward-slash form for manifest keys.
    fn rel_key(rel: &Path) -> String {
        rel.components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Map a template-relative path to the on-disk layout Claude Code actually
    /// scans. `dst` is the `.claude/` dir, so each top-level template subdir
    /// lands where Claude's discovery expects:
    ///
    ///   * `workflows/<f>`               → `hoangsa/workflows/<f>` (internal —
    ///     slash commands resolve their body from here; NOT auto-discovered
    ///     by Claude Code)
    ///   * `commands/<f>`                → `commands/<f>` (subdir becomes the
    ///     `hoangsa:` namespace)
    ///   * `skills/hoangsa/<skill>/<f>`  → `skills/<skill>/<f>` (flatten the
    ///     extra `hoangsa/` level — Claude expects `<skill>/SKILL.md` directly
    ///     under `skills/`)
    ///   * `agents/<f>`                  → `agents/<f>`
    ///
    /// Unrecognized top-level dirs are preserved as-is so unit tests (which
    /// use synthetic trees without these names) still pass.
    fn route_rel(rel: &Path) -> PathBuf {
        let mut comps = rel.components();
        let Some(first) = comps.next() else {
            return rel.to_path_buf();
        };
        let tail = comps.as_path().to_path_buf();
        let first_str = first.as_os_str().to_string_lossy();
        match first_str.as_ref() {
            "workflows" => Path::new("hoangsa").join("workflows").join(&tail),
            "commands" => Path::new("commands").join(&tail),
            "agents" => Path::new("agents").join(&tail),
            "skills" => {
                // Strip an optional leading `hoangsa/` namespace subdir so a
                // skill lands at `skills/<name>/SKILL.md`.
                let mut t = tail.components();
                match t.next() {
                    Some(c) if c.as_os_str() == "hoangsa" => {
                        Path::new("skills").join(t.as_path())
                    }
                    _ => Path::new("skills").join(&tail),
                }
            }
            _ => rel.to_path_buf(),
        }
    }

    /// Copy `src` → `dst` recursively, backing up any `dst` file that the user
    /// modified since the previous install. A file counts as "modified" when
    /// its current SHA256 differs from the hash recorded in `prev_manifest`.
    ///
    /// Backups land at `<dst>/hoangsa-patches/<relpath>.bak-<stamp>`.
    /// Returns both the report and a freshly computed manifest (keyed by `src`).
    pub fn copy_templates(
        src: &Path,
        dst: &Path,
        prev_manifest: &Option<Manifest>,
    ) -> io::Result<(CopyReport, Manifest)> {
        if !src.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                format!("template source not found: {}", src.display()),
            ));
        }
        fs::create_dir_all(dst)?;

        let patch_root = patches_root(dst);
        let stamp = super::backup_timestamp();
        let mut report = CopyReport::default();
        let mut new_manifest = Manifest::new(CLI_VERSION);

        for src_file in walk_files(src)? {
            let rel = src_file
                .strip_prefix(src)
                .map_err(|_| io::Error::other("strip_prefix failed"))?;
            let rel_str = rel_key(rel);
            let dst_file = dst.join(route_rel(rel));

            // Record the source hash — the manifest tracks pristine install state.
            let src_hash = compute_file_sha256(&src_file)?;
            new_manifest.files.insert(rel_str.clone(), src_hash.clone());

            // Patch-backup gate: only if dst already exists AND prev manifest had it
            // AND the current on-disk hash disagrees with what we last wrote.
            if dst_file.exists()
                && let Some(prev) = prev_manifest
                && let Some(prev_hash) = prev.files.get(&rel_str)
            {
                let current_hash = compute_file_sha256(&dst_file)?;
                if &current_hash != prev_hash {
                    let backup_path = patch_root.join(format!("{}.bak-{}", rel_str, stamp));
                    if let Some(parent) = backup_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&dst_file, &backup_path)?;
                    report.patched_backups.push(backup_path);
                }
            }

            // Decide copy vs skip: skip when the dst already matches the src byte-for-byte.
            let needs_copy = match (dst_file.exists(), prev_manifest.is_some()) {
                (false, _) => true,
                (true, _) => compute_file_sha256(&dst_file)? != src_hash,
            };

            if needs_copy {
                if let Some(parent) = dst_file.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(&src_file, &dst_file)?;
                report.copied.push(dst_file);
            } else {
                report.skipped.push(dst_file);
            }
        }

        Ok((report, new_manifest))
    }

    /// Directory where `.bak-<stamp>` files land. Kept under `dst/hoangsa-patches/`
    /// so backups stay co-located with the install tree instead of polluting
    /// the caller's cwd (dst is now `.claude/`, whose parent is the project
    /// root or `$HOME`).
    fn patches_root(dst: &Path) -> PathBuf {
        dst.join("hoangsa-patches")
    }

    /// Build the `actions` array for `--dry-run`: one `copy` per source file,
    /// plus one `backup` per file that would be detected as user-modified.
    pub fn plan_actions(
        src: &Path,
        dst: &Path,
        prev_manifest: &Option<Manifest>,
    ) -> io::Result<Vec<PlannedAction>> {
        let mut actions = Vec::new();
        if !src.is_dir() {
            return Ok(actions);
        }
        let patch_root = patches_root(dst);
        let stamp = super::backup_timestamp();

        for src_file in walk_files(src)? {
            let rel = src_file
                .strip_prefix(src)
                .map_err(|_| io::Error::other("strip_prefix failed"))?;
            let rel_str = rel_key(rel);
            let dst_file = dst.join(route_rel(rel));

            if dst_file.exists()
                && let Some(prev) = prev_manifest
                && let Some(prev_hash) = prev.files.get(&rel_str)
            {
                let current_hash = compute_file_sha256(&dst_file)?;
                if &current_hash != prev_hash {
                    actions.push(PlannedAction {
                        action: "backup".into(),
                        src: dst_file.clone(),
                        dst: patch_root.join(format!("{}.bak-{}", rel_str, stamp)),
                        backup: Some(patch_root.join(format!("{}.bak-{}", rel_str, stamp))),
                    });
                }
            }

            actions.push(PlannedAction {
                action: "copy".into(),
                src: src_file.clone(),
                dst: dst_file,
                backup: None,
            });
        }
        Ok(actions)
    }

    /// Resolve the manifest path for a given destination tree.
    ///
    /// Per Decision #11 the real install writes to `~/.hoangsa/manifest.json`,
    /// but tests pass a tempdir — so the caller computes it.
    pub fn default_manifest_path() -> Result<PathBuf, String> {
        Ok(super::memory_install_dir()?.join("manifest.json"))
    }
}

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
pub mod hooks {
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

        // hsp ships alongside hoangsa-cli in local-dev installs but is absent
        // from release tarballs — register the rewrite hook only when the
        // binary is actually present, so we never leave a dangling command
        // pointing at a missing executable.
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
}

// ───────────────────────── relocate submodule ─────────────────────────
//
// Moves the bundled `hoangsa-memory` + `hoangsa-memory-mcp` binaries out of
// the tarball staging area and into the stable per-user directory
// `~/.hoangsa/bin/` — regardless of `--global` or `--local` mode
// (REQ-10, Decision #5: memory bins are a shared per-user resource, not a
// per-project asset).
//
// `scripts/install.sh` already performs this relocation on the happy curl|sh
// path. This submodule covers the complementary entry points:
//   * `npx hoangsa-cc` / `bin/install` (Node shim) invoking the CLI directly,
//   * `hoangsa-cli install --local` re-run after a partial install,
//   * CI / tests that hand a staging dir to the Rust installer.
//
// When neither `HOANGSA_STAGING_DIR` nor `HOANGSA_TEMPLATES_DIR` is set the
// relocate step is skipped with a recorded note — this is the normal case
// for a `--local` re-run where memory bins were already placed globally.
pub mod relocate {
    use super::*;

    /// Binary file names the relocator looks for under the staging dir.
    /// Kept as a constant so `plan` + `execute` share one source of truth.
    pub const MEMORY_BINS: &[&str] = &["hoangsa-memory", "hoangsa-memory-mcp"];

    /// Summary of a relocate run. `relocated` lists the destination paths we
    /// wrote (or overwrote); `skipped_missing` lists bin names that weren't
    /// present in the staging tree — useful for the install report JSON.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct RelocateReport {
        pub relocated: Vec<PathBuf>,
        pub skipped_missing: Vec<String>,
    }

    /// Production destination: `<HOANGSA_INSTALL_DIR>/bin/`, which defaults to
    /// `~/.hoangsa/bin/` when the env var is unset. Resolves via the
    /// shared [`super::memory_install_dir`] helper so the Rust installer and
    /// `scripts/install.sh` agree on where bins land.
    pub fn memory_bin_dir() -> Result<PathBuf, String> {
        Ok(super::memory_install_dir()?.join("bin"))
    }

    /// Resolve the staging directory the CLI should pull memory bins from.
    ///
    /// Precedence:
    ///   1. `$HOANGSA_STAGING_DIR` — explicit handoff from `install.sh`.
    ///   2. Parent of `$HOANGSA_TEMPLATES_DIR` — `install.sh` points templates
    ///      at `<PKG_DIR>/templates` and the bins live at `<PKG_DIR>/bin`.
    ///   3. None — caller skips the relocate step.
    pub fn staging_dir_from_env() -> Option<PathBuf> {
        if let Ok(s) = std::env::var("HOANGSA_STAGING_DIR") {
            let p = PathBuf::from(s);
            if p.is_dir() {
                return Some(p);
            }
        }
        if let Ok(t) = std::env::var("HOANGSA_TEMPLATES_DIR") {
            let tp = PathBuf::from(t);
            if let Some(parent) = tp.parent()
                && parent.is_dir()
            {
                return Some(parent.to_path_buf());
            }
        }
        None
    }

    /// Discover the absolute paths of memory bins inside `staging`. Looks
    /// under `<staging>/bin/` first (the canonical tarball layout) and falls
    /// back to `<staging>/` for flatter test fixtures. Missing bins are
    /// silently ignored — `relocate_memory_bins_to` surfaces them via
    /// `skipped_missing`.
    pub fn source_memory_bins(staging: &Path) -> Vec<PathBuf> {
        let mut found = Vec::new();
        for name in MEMORY_BINS {
            let bin_subdir = staging.join("bin").join(name);
            if bin_subdir.is_file() {
                found.push(bin_subdir);
                continue;
            }
            let at_root = staging.join(name);
            if at_root.is_file() {
                found.push(at_root);
            }
        }
        found
    }

    /// Same semantics as [`relocate_memory_bins`] but with an explicit
    /// destination — keeps tests hermetic (they never touch the real
    /// `~/.hoangsa/bin/`).
    pub fn relocate_memory_bins_to(staging: &Path, dest: &Path) -> io::Result<RelocateReport> {
        fs::create_dir_all(dest)?;
        let mut report = RelocateReport::default();
        let found = source_memory_bins(staging);
        let found_names: std::collections::HashSet<String> = found
            .iter()
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect();

        for src in &found {
            let name = match src.file_name() {
                Some(n) => n.to_owned(),
                None => continue,
            };
            let dst = dest.join(&name);
            // Atomic-ish overwrite: copy to sibling tmp, chmod, rename. Matches
            // `install.sh`'s `install_bin` so behavior stays consistent.
            let tmp = dst.with_extension(format!(
                "new.{}",
                std::process::id()
            ));
            fs::copy(src, &tmp)?;
            set_executable(&tmp)?;
            // `rename` across the same dir is atomic on POSIX + Windows.
            match fs::rename(&tmp, &dst) {
                Ok(()) => {}
                Err(e) => {
                    // Best-effort cleanup of the tmp file; propagate the error.
                    let _ = fs::remove_file(&tmp);
                    return Err(e);
                }
            }
            report.relocated.push(dst);
        }

        for name in MEMORY_BINS {
            if !found_names.contains(*name) {
                report.skipped_missing.push((*name).to_string());
            }
        }

        Ok(report)
    }

    /// Copy the memory bins from `staging` into `~/.hoangsa/bin/`.
    /// Idempotent: re-running overwrites the existing copies. Missing sources
    /// are reported, not an error (matches `install_bin` in `install.sh`).
    pub fn relocate_memory_bins(staging: &Path) -> io::Result<RelocateReport> {
        let dest = memory_bin_dir().map_err(io::Error::other)?;
        relocate_memory_bins_to(staging, &dest)
    }

    /// Set the executable bit (0o755) on unix; no-op on windows where the
    /// concept doesn't apply. Kept tiny + in one place so the test and the
    /// prod call share identical permission logic.
    #[cfg(unix)]
    fn set_executable(path: &Path) -> io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)
    }

    #[cfg(not(unix))]
    fn set_executable(_path: &Path) -> io::Result<()> {
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        //! Unit tests for the memory-bin relocate pipeline. Every test uses
        //! `tempfile::tempdir()` for BOTH source and destination — we never
        //! write to the real `~/.hoangsa/bin/`.

        use super::*;
        use std::fs;
        use tempfile::tempdir;

        fn touch_bin(path: &Path, contents: &str) {
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create parent");
            }
            fs::write(path, contents).expect("write fixture");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(path).expect("meta").permissions();
                perms.set_mode(0o755);
                fs::set_permissions(path, perms).expect("chmod");
            }
        }

        #[test]
        fn source_memory_bins_finds_in_bin_dir() {
            let tmp = tempdir().expect("tempdir");
            let staging = tmp.path();
            touch_bin(&staging.join("bin/hoangsa-memory"), "#!memory");
            touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "#!mcp");

            let found = source_memory_bins(staging);
            let names: Vec<String> = found
                .iter()
                .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .collect();
            assert!(names.iter().any(|n| n == "hoangsa-memory"));
            assert!(names.iter().any(|n| n == "hoangsa-memory-mcp"));
            assert_eq!(found.len(), 2);
        }

        #[test]
        fn source_memory_bins_missing_returns_empty() {
            let tmp = tempdir().expect("tempdir");
            // Empty staging → no bins discovered.
            let found = source_memory_bins(tmp.path());
            assert!(found.is_empty());
        }

        #[test]
        fn source_memory_bins_falls_back_to_root() {
            let tmp = tempdir().expect("tempdir");
            let staging = tmp.path();
            // Only root-level bin (no bin/ subdir) — flatter test layout.
            touch_bin(&staging.join("hoangsa-memory"), "#!memory");
            let found = source_memory_bins(staging);
            assert_eq!(found.len(), 1);
            assert_eq!(
                found[0].file_name().expect("name").to_string_lossy(),
                "hoangsa-memory"
            );
        }

        #[test]
        fn relocate_copies_and_sets_executable() {
            let tmp = tempdir().expect("tempdir");
            let staging = tmp.path().join("staging");
            let dest = tmp.path().join("dest/bin");
            touch_bin(&staging.join("bin/hoangsa-memory"), "v1-memory");
            touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "v1-mcp");

            let report = relocate_memory_bins_to(&staging, &dest).expect("relocate");

            assert_eq!(report.relocated.len(), 2);
            assert!(report.skipped_missing.is_empty());
            assert_eq!(
                fs::read_to_string(dest.join("hoangsa-memory")).expect("read"),
                "v1-memory"
            );
            assert_eq!(
                fs::read_to_string(dest.join("hoangsa-memory-mcp")).expect("read"),
                "v1-mcp"
            );

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                for name in MEMORY_BINS {
                    let mode = fs::metadata(dest.join(name))
                        .expect("meta")
                        .permissions()
                        .mode()
                        & 0o777;
                    assert_eq!(mode, 0o755, "expected 0o755 on {name}, got {:o}", mode);
                }
            }
        }

        #[test]
        fn relocate_reports_missing_bins() {
            let tmp = tempdir().expect("tempdir");
            let staging = tmp.path().join("staging");
            let dest = tmp.path().join("dest/bin");
            // Only one of the two expected bins is present.
            touch_bin(&staging.join("bin/hoangsa-memory"), "v1");

            let report = relocate_memory_bins_to(&staging, &dest).expect("relocate");
            assert_eq!(report.relocated.len(), 1);
            assert_eq!(report.skipped_missing, vec!["hoangsa-memory-mcp".to_string()]);
        }

        #[test]
        fn relocate_idempotent() {
            let tmp = tempdir().expect("tempdir");
            let staging = tmp.path().join("staging");
            let dest = tmp.path().join("dest/bin");

            touch_bin(&staging.join("bin/hoangsa-memory"), "v1");
            touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "v1-mcp");
            let _r1 = relocate_memory_bins_to(&staging, &dest).expect("relocate v1");

            // Bump the source content — second run must overwrite.
            touch_bin(&staging.join("bin/hoangsa-memory"), "v2");
            touch_bin(&staging.join("bin/hoangsa-memory-mcp"), "v2-mcp");
            let r2 = relocate_memory_bins_to(&staging, &dest).expect("relocate v2");

            assert_eq!(r2.relocated.len(), 2);
            assert_eq!(
                fs::read_to_string(dest.join("hoangsa-memory")).expect("read"),
                "v2"
            );
            assert_eq!(
                fs::read_to_string(dest.join("hoangsa-memory-mcp")).expect("read"),
                "v2-mcp"
            );
        }
    }
}

// ───────────────────────── mode submodule ─────────────────────────
//
// Mode-aware global/local semantics per REQ-07..REQ-09 + Decision #4 + #13:
//   * `--global` → MCP registration in `~/.claude.json`; no cwd writes.
//   * `--local`  → MCP registration in `<cwd>/.mcp.json`; exit 3 if the
//                  `hoangsa-memory-mcp` binary is absent from
//                  `~/.hoangsa/bin/` (REQ-09 hint).
//   * Rule + `.memoryignore` seeds are **local-only** — `--global` must
//     never create them in the user's current directory.
//   * Quality-gate skills (`silent-failure-hunter`, `pr-test-analyzer`,
//     `comment-analyzer`, `type-design-analyzer`) install only in
//     `--global` mode, landing under `~/.claude/skills/<skill>/` (the
//     caller is responsible for gating).
//
// The port mirrors `registerMemoryMcp` in `bin/install` (scaffold keeps
// `{command, args, env}` shape, preserves existing keys) with two extra
// hermetic-friendly variants: `_to_home()` / `register_mcp_local_to()` so
// the unit tests can point at a tempdir pretend-home without touching
// the real `~/.claude.json` / `~/.claude/skills/`.
pub mod mode {
    use super::*;
    use serde_json::{Value, json};

    /// The quality-gate skills shipped with `--global` installs (REQ /
    /// Decision #13). Kept as a single source of truth so dry-run preview,
    /// the live installer, and tests agree on the set.
    pub const QUALITY_SKILLS: &[&str] = &[
        "silent-failure-hunter",
        "pr-test-analyzer",
        "comment-analyzer",
        "type-design-analyzer",
    ];

    /// Standard `.memoryignore` seed written in `--local` mode when
    /// the project doesn't already carry one. Covers hoangsa-memory's own
    /// data dir, common JS/TS build output, and generated/large files.
    /// Matches the repo's top-level `.memoryignore` so a fresh
    /// HOANGSA project starts with the same baseline the monorepo uses.
    pub const DEFAULT_MEMORY_IGNORE: &str = "\
# .memoryignore — hoangsa-memory-specific ignore rules (gitignore syntax).
# Layered on top of .gitignore. Edit freely.

# hoangsa data (always ignored by the watcher, but explicit here too)
.hoangsa/

# Node / JS / TS
node_modules/
dist/
build/
.next/
.nuxt/
coverage/
*.min.js
*.bundle.js
package-lock.json
yarn.lock
pnpm-lock.yaml

# Common generated / large files
*.generated.*
*.min.css
*.map
*.pb.rs
";

    /// Minimal `rules.json` seed — an empty rule list keyed by schema
    /// version. The real HOANGSA rule set is seeded separately via
    /// `hoangsa-cli rule init`; this keeps the on-disk file-shape valid
    /// for first-run detection without committing us to a specific rule
    /// inventory here.
    pub const DEFAULT_RULES_JSON: &str = "{\n  \"version\": \"1.0\",\n  \"rules\": []\n}\n";

    /// Path to Claude Code's global MCP config file (`.claude.json`).
    /// Delegates to the crate-level `claude_json_path` helper so the same
    /// `$CLAUDE_CONFIG_DIR` resolution drives both settings and MCP writes.
    pub fn claude_json_path() -> Result<PathBuf, String> {
        super::claude_json_path()
    }

    /// Path to `<cwd>/.mcp.json` — the Claude Code per-project MCP config.
    pub fn local_mcp_path(cwd: &Path) -> PathBuf {
        cwd.join(".mcp.json")
    }

    /// Absolute path to the globally-installed `hoangsa-memory-mcp` binary.
    /// Resolves under `$HOANGSA_INSTALL_DIR` (default `~/.hoangsa`) so
    /// a user who overrode the install dir in `scripts/install.sh` still gets
    /// an MCP `command` field pointing at the real bin location.
    /// `--local` register requires this to exist (REQ-09 exit 3).
    pub fn memory_mcp_bin() -> Result<PathBuf, String> {
        Ok(super::memory_install_dir()?
            .join("bin")
            .join("hoangsa-memory-mcp"))
    }

    /// Load a JSON object from disk or return `{}` on missing / unreadable
    /// / malformed. Keeps the merge helpers free of IO branches.
    fn load_json_object(path: &Path) -> Value {
        match fs::read_to_string(path) {
            Ok(raw) => match serde_json::from_str::<Value>(&raw) {
                Ok(v) if v.is_object() => v,
                _ => Value::Object(serde_json::Map::new()),
            },
            Err(_) => Value::Object(serde_json::Map::new()),
        }
    }

    /// Pretty-write JSON with 2-space indent + trailing newline — matches
    /// the on-disk shape Claude Code (and `bin/install`) uses.
    fn save_json_object(path: &Path, value: &Value) -> io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
        out.push('\n');
        fs::write(path, out)
    }

    /// Build the `hoangsa-memory` MCP server entry. Preserves an existing
    /// `env` block on repeat installs so a user-set `HOANGSA_MEMORY_ROOT`
    /// survives; mirrors the env-preservation in `registerMemoryMcp`.
    fn build_mcp_entry(command: &Path, existing_entry: Option<&Value>) -> Value {
        let mut env_map = serde_json::Map::new();
        env_map.insert("RUST_LOG".into(), Value::String("info".into()));
        if let Some(existing) = existing_entry
            && let Some(env) = existing.get("env").and_then(|e| e.as_object())
        {
            for (k, v) in env {
                env_map.insert(k.clone(), v.clone());
            }
        }
        json!({
            "command": command.display().to_string(),
            "args": [],
            "env": Value::Object(env_map),
        })
    }

    /// Merge the `hoangsa-memory` MCP entry into the JSON object at
    /// `json_path`, preserving all other top-level keys and every other
    /// entry in `mcpServers`. Used by both the global (`~/.claude.json`)
    /// and local (`<cwd>/.mcp.json`) registration paths, which only
    /// differ in where they look for prerequisites and how they
    /// surface errors.
    fn merge_mcp_entry(json_path: &Path, memory_bin: &Path) -> io::Result<()> {
        let mut data = load_json_object(json_path);
        let obj = data
            .as_object_mut()
            .ok_or_else(|| io::Error::other("MCP config root is not an object"))?;

        let servers_val = obj
            .entry("mcpServers".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()));
        if !servers_val.is_object() {
            *servers_val = Value::Object(serde_json::Map::new());
        }
        let servers = servers_val
            .as_object_mut()
            .expect("mcpServers normalized to object");

        let existing = servers.get("hoangsa-memory").cloned();
        servers.insert(
            "hoangsa-memory".into(),
            build_mcp_entry(memory_bin, existing.as_ref()),
        );

        save_json_object(json_path, &data)
    }

    /// Register the `hoangsa-memory` MCP server in an explicit
    /// `claude.json` target (test-friendly variant of
    /// [`register_mcp_global`]). Preserves all other top-level keys and
    /// every other entry in `mcpServers`.
    ///
    /// `memory_bin` is the absolute path recorded in `command`. The
    /// caller is responsible for existence-checking it and emitting any
    /// warning — `register_mcp_global_to` deliberately does not fail
    /// on a missing bin (Decision: warn, still write, so the config
    /// lands even if the user has the bin on `PATH` via some other
    /// mechanism).
    pub fn register_mcp_global_to(claude_json: &Path, memory_bin: &Path) -> io::Result<()> {
        merge_mcp_entry(claude_json, memory_bin)
    }

    /// Register the `hoangsa-memory` MCP server in `~/.claude.json`
    /// (REQ-08, Decision #4). Emits a warning on stderr if the memory
    /// binary is absent — still writes the config so an ambient
    /// `PATH`-based bin keeps working.
    pub fn register_mcp_global() -> Result<(), String> {
        let claude_json = claude_json_path()?;
        let memory_bin = memory_mcp_bin()?;
        if !memory_bin.exists() {
            eprintln!(
                "install: warning — hoangsa-memory-mcp not found at {} (writing config anyway)",
                memory_bin.display()
            );
        }
        register_mcp_global_to(&claude_json, &memory_bin).map_err(|e| e.to_string())
    }

    /// Error carrying an explicit exit code — used by `register_mcp_local`
    /// to surface REQ-09's exit-3 contract without smuggling integers
    /// through `String` error values.
    #[derive(Debug)]
    pub struct InstallError {
        pub exit_code: i32,
        pub message: String,
    }

    impl std::fmt::Display for InstallError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for InstallError {}

    /// Test-friendly variant of [`register_mcp_local`]. Writes to the
    /// explicit `mcp_json` path and treats `memory_bin` as the
    /// existence-gated prerequisite.
    pub fn register_mcp_local_to(
        mcp_json: &Path,
        memory_bin: &Path,
    ) -> Result<(), InstallError> {
        if !memory_bin.exists() {
            return Err(InstallError {
                exit_code: 3,
                message: format!(
                    "hoangsa-memory-mcp not found at {} — run `hoangsa-cli install --global` first to install hoangsa-memory bins",
                    memory_bin.display()
                ),
            });
        }
        merge_mcp_entry(mcp_json, memory_bin).map_err(|e| InstallError {
            exit_code: 1,
            message: format!("write {}: {}", mcp_json.display(), e),
        })
    }

    /// Register the memory MCP server in `<cwd>/.mcp.json` (REQ-09).
    /// Exits (via `InstallError`) with code 3 when the globally-installed
    /// `hoangsa-memory-mcp` is absent.
    pub fn register_mcp_local(cwd: &Path) -> Result<(), InstallError> {
        let memory_bin = memory_mcp_bin().map_err(|m| InstallError {
            exit_code: 1,
            message: m,
        })?;
        register_mcp_local_to(&local_mcp_path(cwd), &memory_bin)
    }

    /// Create `<cwd>/.hoangsa/rules.json` with the minimal HOANGSA rule
    /// seed when the file is absent. Never overwrites an existing file —
    /// users may have customized rules via `hoangsa-cli rule`.
    pub fn seed_local_rules(cwd: &Path) -> io::Result<bool> {
        let path = cwd.join(".hoangsa").join("rules.json");
        if path.exists() {
            return Ok(false);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, DEFAULT_RULES_JSON)?;
        Ok(true)
    }

    /// Create `<cwd>/.memoryignore` with the default seed when the file
    /// is absent. Idempotent — preserves user customizations on re-run.
    pub fn seed_memory_ignore(cwd: &Path) -> io::Result<bool> {
        let path = cwd.join(".memoryignore");
        if path.exists() {
            return Ok(false);
        }
        fs::write(&path, DEFAULT_MEMORY_IGNORE)?;
        Ok(true)
    }

    /// Summary of an `install_quality_skills` run — the set of skill
    /// names that were already present and the ones that are still
    /// outstanding. This function only prepares the host directory and
    /// reports state; the Rust installer does not ship with a built-in
    /// `npx skills add` equivalent, so `pending` is reflected as a
    /// top-level install warning (status = "partial") rather than
    /// silently being reported as a successful install.
    #[derive(Debug, Default, Clone, PartialEq, Eq)]
    pub struct QualitySkillsReport {
        pub already_present: Vec<String>,
        pub pending: Vec<String>,
    }

    /// Test-friendly variant. Computes the quality-skills status against
    /// an explicit `<home>/.claude/skills/` root so tests can stage a
    /// tempdir pretend-home without touching `~/.claude/skills/`.
    pub fn install_quality_skills_to(skills_root: &Path) -> io::Result<QualitySkillsReport> {
        fs::create_dir_all(skills_root)?;
        let mut report = QualitySkillsReport::default();
        for skill in QUALITY_SKILLS {
            let dir = skills_root.join(skill);
            if dir.is_dir() {
                report.already_present.push((*skill).to_string());
            } else {
                report.pending.push((*skill).to_string());
            }
        }
        Ok(report)
    }

    /// Production entry point — operates on `$CLAUDE_CONFIG_DIR/skills/`
    /// (fallback `~/.claude/skills/`). ONLY call from the `--global` flow
    /// (Decision #13).
    pub fn install_quality_skills() -> Result<QualitySkillsReport, String> {
        let skills_root = super::claude_config_dir()?.join("skills");
        install_quality_skills_to(&skills_root).map_err(|e| e.to_string())
    }

    #[cfg(test)]
    mod tests {
        //! Hermetic unit tests for mode-aware semantics. Every test uses
        //! `tempfile::tempdir()` for pretend-home and pretend-cwd — never
        //! touches the real `~/.claude.json`, `~/.claude/skills/`, or the
        //! real cwd.

        use super::*;
        use serde_json::json;
        use tempfile::tempdir;

        /// Mirror the dry-run action planner under `--global`, run against
        /// a pretend `home` + `cwd`, and assert none of the produced
        /// target paths live under `cwd`. Exercises REQ-07.
        fn global_actions_for(home: &Path, cwd: &Path) -> Vec<Value> {
            let mut actions = Vec::new();
            // MCP register target (global).
            actions.push(json!({
                "action": "register_mcp_global",
                "target": home.join(".claude.json"),
            }));
            // Quality-gate skills root.
            actions.push(json!({
                "action": "install_quality_skills",
                "target": home.join(".claude").join("skills"),
            }));
            // Sanity: a local-only action that MUST NOT appear in global —
            // included here only so the assertion catches regressions if
            // the planner mistakenly merges local actions into global.
            let _forbidden_for_global = [cwd.join(".mcp.json"),
                cwd.join(".hoangsa").join("rules.json"),
                cwd.join(".memoryignore")];
            actions
        }

        #[test]
        fn global_no_cwd_writes() {
            let home_dir = tempdir().expect("home tempdir");
            let cwd_dir = tempdir().expect("cwd tempdir");
            let actions = global_actions_for(home_dir.path(), cwd_dir.path());

            // No action target may live under the pretend cwd.
            for a in &actions {
                let target = a
                    .get("target")
                    .and_then(|t| t.as_str().map(PathBuf::from))
                    .or_else(|| {
                        a.get("target").and_then(|t| {
                            serde_json::from_value::<PathBuf>(t.clone()).ok()
                        })
                    })
                    .expect("action target present");
                assert!(
                    !target.starts_with(cwd_dir.path()),
                    "global action must not write under cwd: {:?}",
                    target
                );
            }
            // And must at least register MCP in the pretend home.
            let has_mcp = actions
                .iter()
                .any(|a| a.get("action").and_then(|s| s.as_str()) == Some("register_mcp_global"));
            assert!(has_mcp, "global must plan register_mcp_global");
        }

        #[test]
        fn global_registers_mcp_preserving_keys() {
            let home = tempdir().expect("home tempdir");
            let claude_json = home.path().join(".claude.json");

            // Seed with a top-level key plus a pre-existing MCP server.
            let seed = json!({
                "foo": "bar",
                "mcpServers": {
                    "existing": { "command": "x", "args": [] }
                }
            });
            fs::write(
                &claude_json,
                serde_json::to_string_pretty(&seed).expect("encode"),
            )
            .expect("write seed");

            let bin = home.path().join("fake-memory-mcp");
            fs::write(&bin, "#!/bin/sh\n").expect("write fake bin");

            register_mcp_global_to(&claude_json, &bin).expect("register");

            let raw = fs::read_to_string(&claude_json).expect("read back");
            let back: Value = serde_json::from_str(&raw).expect("parse back");

            assert_eq!(back.get("foo").and_then(|v| v.as_str()), Some("bar"));
            let servers = back
                .get("mcpServers")
                .and_then(|s| s.as_object())
                .expect("mcpServers present");
            assert!(
                servers.contains_key("existing"),
                "existing MCP server preserved"
            );
            assert!(
                servers.contains_key("hoangsa-memory"),
                "hoangsa-memory added"
            );
            assert_eq!(
                servers["hoangsa-memory"]["command"].as_str(),
                Some(bin.display().to_string().as_str())
            );
        }

        #[test]
        fn local_missing_mcp_bin_exits_3() {
            let cwd = tempdir().expect("cwd tempdir");
            let home = tempdir().expect("home tempdir");
            let missing_bin = home.path().join("nope-memory-mcp");

            let err = register_mcp_local_to(&local_mcp_path(cwd.path()), &missing_bin)
                .expect_err("missing bin must fail");
            assert_eq!(err.exit_code, 3, "REQ-09 requires exit code 3");
            assert!(
                err.message.contains("--global") || err.message.contains("hoangsa-memory"),
                "error message should hint at the global-install remedy, got: {}",
                err.message
            );
        }

        #[test]
        fn local_merge_existing_mcp_json() {
            let cwd = tempdir().expect("cwd tempdir");
            let mcp_json = local_mcp_path(cwd.path());

            // Pre-populate with a user-authored server.
            let seed = json!({
                "mcpServers": {
                    "user-custom": { "command": "/usr/local/bin/my-mcp", "args": [] }
                }
            });
            fs::write(
                &mcp_json,
                serde_json::to_string_pretty(&seed).expect("encode"),
            )
            .expect("write seed");

            // Fake bin that actually exists so we pass the exit-3 guard.
            let fake_bin = cwd.path().join("fake-hoangsa-memory-mcp");
            fs::write(&fake_bin, "#!/bin/sh\n").expect("write fake bin");

            register_mcp_local_to(&mcp_json, &fake_bin).expect("register");

            let raw = fs::read_to_string(&mcp_json).expect("read back");
            let back: Value = serde_json::from_str(&raw).expect("parse back");
            let servers = back
                .get("mcpServers")
                .and_then(|s| s.as_object())
                .expect("mcpServers");
            assert!(
                servers.contains_key("user-custom"),
                "user-authored server preserved"
            );
            assert!(
                servers.contains_key("hoangsa-memory"),
                "hoangsa-memory registered"
            );
        }

        #[test]
        fn seed_memory_ignore_preserves_existing() {
            let cwd = tempdir().expect("cwd tempdir");
            let existing = "custom/\n# user edits\n";
            fs::write(cwd.path().join(".memoryignore"), existing).expect("seed existing");

            let wrote = seed_memory_ignore(cwd.path()).expect("seed");
            assert!(!wrote, "must not overwrite existing .memoryignore");

            let back = fs::read_to_string(cwd.path().join(".memoryignore")).expect("read back");
            assert_eq!(back, existing, "user content preserved byte-for-byte");
        }

        #[test]
        fn seed_memory_ignore_creates_when_absent() {
            let cwd = tempdir().expect("cwd tempdir");
            let wrote = seed_memory_ignore(cwd.path()).expect("seed");
            assert!(wrote, "fresh cwd should get a seeded .memoryignore");
            let back = fs::read_to_string(cwd.path().join(".memoryignore")).expect("read back");
            assert!(back.contains("node_modules/"), "seed contains standard ignores");
        }

        #[test]
        fn seed_rules_preserves_existing() {
            let cwd = tempdir().expect("cwd tempdir");
            let rules_path = cwd.path().join(".hoangsa").join("rules.json");
            fs::create_dir_all(rules_path.parent().expect("parent")).expect("mkdir");
            let existing = "{\n  \"version\": \"1.0\",\n  \"rules\": [\"custom\"]\n}\n";
            fs::write(&rules_path, existing).expect("seed existing");

            let wrote = seed_local_rules(cwd.path()).expect("seed");
            assert!(!wrote, "must not overwrite existing rules.json");
            let back = fs::read_to_string(&rules_path).expect("read back");
            assert_eq!(back, existing);
        }

        #[test]
        fn seed_rules_creates_when_absent() {
            let cwd = tempdir().expect("cwd tempdir");
            let wrote = seed_local_rules(cwd.path()).expect("seed");
            assert!(wrote);
            let path = cwd.path().join(".hoangsa").join("rules.json");
            let back = fs::read_to_string(&path).expect("read back");
            let v: Value = serde_json::from_str(&back).expect("valid JSON");
            assert_eq!(v.get("version").and_then(|s| s.as_str()), Some("1.0"));
        }

        #[test]
        fn install_quality_skills_lists_missing() {
            let home = tempdir().expect("home tempdir");
            let skills_root = home.path().join(".claude").join("skills");

            let report = install_quality_skills_to(&skills_root).expect("scan");
            assert!(report.already_present.is_empty());
            assert_eq!(report.pending.len(), QUALITY_SKILLS.len());
            assert!(skills_root.is_dir(), "skills root should be created");
        }

        #[test]
        fn install_quality_skills_marks_present() {
            let home = tempdir().expect("home tempdir");
            let skills_root = home.path().join(".claude").join("skills");
            fs::create_dir_all(skills_root.join("silent-failure-hunter")).expect("mkdir");

            let report = install_quality_skills_to(&skills_root).expect("scan");
            assert!(
                report
                    .already_present
                    .iter()
                    .any(|s| s == "silent-failure-hunter")
            );
            assert_eq!(
                report.already_present.len() + report.pending.len(),
                QUALITY_SKILLS.len()
            );
        }

        #[test]
        fn register_mcp_global_honors_install_dir_env() {
            // Bug A regression: `memory_mcp_bin()` used to hardcode
            // `$HOME/.hoangsa/bin/hoangsa-memory-mcp`, ignoring the
            // `HOANGSA_INSTALL_DIR` override from `scripts/install.sh`.
            // With the fix, setting the env var must be reflected in the
            // `command` field written into `claude.json`.
            let custom = tempdir().expect("custom install tempdir");
            let home = tempdir().expect("home tempdir");
            let claude_json = home.path().join(".claude.json");

            // Scope the env var change to this test — reset on drop even if
            // the assertions below panic.
            struct EnvGuard(&'static str, Option<std::ffi::OsString>);
            impl Drop for EnvGuard {
                fn drop(&mut self) {
                    match self.1.take() {
                        Some(v) => unsafe { std::env::set_var(self.0, v) },
                        None => unsafe { std::env::remove_var(self.0) },
                    }
                }
            }
            let _guard = EnvGuard("HOANGSA_INSTALL_DIR", std::env::var_os("HOANGSA_INSTALL_DIR"));
            unsafe {
                std::env::set_var("HOANGSA_INSTALL_DIR", custom.path());
            }

            // Resolve the bin path through the same helper the production
            // `register_mcp_global` uses — must land inside `custom`.
            let bin = memory_mcp_bin().expect("memory_mcp_bin");
            assert!(
                bin.starts_with(custom.path()),
                "memory_mcp_bin must honor HOANGSA_INSTALL_DIR: {:?} not under {:?}",
                bin,
                custom.path()
            );

            // Persist a fake bin so `register_mcp_global_to` doesn't complain,
            // then exercise the merge directly with the resolved path.
            register_mcp_global_to(&claude_json, &bin).expect("register");

            let raw = fs::read_to_string(&claude_json).expect("read back");
            let back: Value = serde_json::from_str(&raw).expect("parse");
            let command = back["mcpServers"]["hoangsa-memory"]["command"]
                .as_str()
                .expect("command field present");
            assert!(
                command.starts_with(custom.path().to_string_lossy().as_ref()),
                "MCP command must point inside HOANGSA_INSTALL_DIR override: got {command}, expected prefix {:?}",
                custom.path()
            );
        }

        #[test]
        fn global_quality_skills_target_not_under_cwd() {
            // Defense-in-depth for REQ-07: the resolved target for the
            // quality-skills write must never live under the current
            // working directory regardless of where the user runs from.
            let home = tempdir().expect("home tempdir");
            let cwd = tempdir().expect("cwd tempdir");
            let target = home.path().join(".claude").join("skills");
            install_quality_skills_to(&target).expect("install");
            assert!(
                !target.starts_with(cwd.path()),
                "skills root must not live under cwd: {:?} vs {:?}",
                target,
                cwd.path()
            );
        }
    }
}

/// Destination tree for the installed templates, derived from mode + cwd.
/// `global` → `$CLAUDE_CONFIG_DIR` (fallback `~/.claude/`); `local` →
/// `<cwd>/.claude/`. The `templates` module's `route_rel` fans each template
/// subdir (`commands/`, `skills/`, `agents/`, `workflows/`) into the right
/// spot under this root so Claude Code's discovery (which only scans
/// `{commands,skills,agents}/` inside the config dir) actually finds them.
/// The hoangsa-internal `workflows/` tree lives at `<dst>/hoangsa/workflows/`,
/// matching what each slash command resolves.
fn install_dst_dir(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
    match mode {
        "global" => claude_config_dir(),
        _ => Ok(cwd.join(".claude")),
    }
}

/// Entry point for `hoangsa-cli install ...`.
///
/// The T-01 scaffold handled flags + dry-run preview. T-03 adds the actual
/// template copy + manifest + patch-backup path for non-dry-run `global|local`
/// invocations. Settings merge / MCP register / memory-bin relocate remain
/// deferred to T-04/T-05/T-06.
pub fn cmd_install(args: &[&str]) {
    let flags = match parse_flags(args) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(2);
        }
    };

    if let Err(e) = validate(&flags) {
        eprintln!("install: {e}");
        std::process::exit(2);
    }

    let mode = mode_str(&flags);
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    if flags.dry_run {
        let mut actions_json: Vec<serde_json::Value> = Vec::new();
        let mut warnings: Vec<String> = Vec::new();

        {
            match (
                templates::templates_source_dir(mode, &cwd),
                install_dst_dir(mode, &cwd),
            ) {
                (Ok(src), Ok(dst)) => {
                    let manifest_path = templates::default_manifest_path().ok();
                    let prev = match manifest_path
                        .as_ref()
                        .map(|p| templates::load_manifest(p))
                    {
                        Some(Ok(m)) => m,
                        Some(Err(e)) => {
                            warnings.push(format!("load_manifest: {e}"));
                            None
                        }
                        None => None,
                    };
                    match templates::plan_actions(&src, &dst, &prev) {
                        Ok(acts) => {
                            for a in acts {
                                actions_json.push(serde_json::to_value(a).unwrap_or(json!({})));
                            }
                        }
                        Err(e) => warnings.push(format!("plan_actions: {e}")),
                    }
                }
                (Err(e), _) => warnings.push(e),
                (_, Err(e)) => warnings.push(e),
            }

            // T-06 dry-run: list each memory bin we WOULD relocate out of the
            // tarball staging area into `~/.hoangsa/bin/`. Silent when
            // no staging dir is advertised (normal for re-runs) and skipped
            // entirely under `--no-memory`.
            if !flags.no_memory
                && let Some(staging) = relocate::staging_dir_from_env() {
                    let dest_preview = relocate::memory_bin_dir()
                        .unwrap_or_else(|_| PathBuf::from("~/.hoangsa/bin"));
                    for src in relocate::source_memory_bins(&staging) {
                        let name = src
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        actions_json.push(json!({
                            "action": "relocate_memory_bin",
                            "src": src,
                            "dst": dest_preview.join(&name),
                        }));
                    }
                }

            // T-05: mode-aware targets — MCP register, rule + memory_ignore
            // seed (local-only), and quality-skills (global-only). Every
            // action attaches the resolved absolute target so REQ-07 /
            // REQ-08 / REQ-09 can be asserted from the preview alone.
            match mode {
                "global" => {
                    match mode::claude_json_path() {
                        Ok(p) => actions_json.push(json!({
                            "action": "register_mcp_global",
                            "target": p,
                        })),
                        Err(e) => warnings.push(e),
                    }
                    match claude_config_dir() {
                        Ok(d) => actions_json.push(json!({
                            "action": "install_quality_skills",
                            "target": d.join("skills"),
                            "skills": mode::QUALITY_SKILLS,
                        })),
                        Err(e) => warnings.push(e),
                    }
                }
                "local" => {
                    // Surface the prereq check in the preview so the
                    // caller can see the exit-3 risk before running live.
                    match mode::memory_mcp_bin() {
                        Ok(bin) if !bin.exists() => warnings.push(format!(
                            "hoangsa-memory-mcp missing at {} — live --local will exit 3",
                            bin.display()
                        )),
                        Ok(_) => {}
                        Err(e) => warnings.push(e),
                    }
                    actions_json.push(json!({
                        "action": "register_mcp_local",
                        "target": mode::local_mcp_path(&cwd),
                    }));
                    actions_json.push(json!({
                        "action": "seed_local_rules",
                        "target": cwd.join(".hoangsa").join("rules.json"),
                    }));
                    actions_json.push(json!({
                        "action": "seed_memory_ignore",
                        "target": cwd.join(".memoryignore"),
                    }));
                }
                _ => {}
            }

            // Plan for the settings.json merge too — T-04 owns this leg.
            match hooks::settings_path(mode, &cwd) {
                Ok(settings_file) => {
                    // Dry-run shouldn't read `HOME` for real; still, we load the
                    // existing settings (safe, read-only) so we can preview the
                    // delta honestly. A corrupt file becomes a preview warning
                    // (not fatal) so the user still sees a plan they can act on.
                    let mut preview_settings = match hooks::load_settings(&settings_file) {
                        Ok(v) => v,
                        Err(e) => {
                            warnings.push(format!("load_settings: {e}"));
                            Value::Object(serde_json::Map::new())
                        }
                    };
                    let target_dir = settings_file
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| PathBuf::from(".claude"));
                    let hooks_payload = hooks::build_hoangsa_hooks(&target_dir);
                    let hooks_added = hooks::merge_hoangsa_hooks(&mut preview_settings, &hooks_payload);
                    let statusline_set =
                        hooks::apply_statusline(&mut preview_settings, &hooks::default_statusline(&target_dir));
                    actions_json.push(json!({
                        "action": "merge_settings",
                        "path": settings_file,
                        "hooks_added": hooks_added,
                        "statusline_set": statusline_set,
                    }));
                }
                Err(e) => warnings.push(e),
            }
        }

        let preview = json!({
            "mode": mode,
            "actions": actions_json,
            "warnings": warnings,
            "targets": {
                "global_claude_json": "~/.claude.json",
                "local_claude_dir": ".claude/",
                "memory_bin_dir": "~/.hoangsa/bin/",
                "manifest": "~/.hoangsa/manifest.json"
            },
            "flags": {
                "global": flags.global,
                "local": flags.local,
                "no_memory": flags.no_memory,
                "skip_path_edit": flags.skip_path_edit,
                "task_manager": flags.task_manager
            }
        });
        helpers::out(&preview);
        return;
    }

    // Warnings collector for the live flow. Non-fatal per-step errors
    // (optional seeds, quality-skills, etc.) accumulate here and surface
    // in the final JSON so the top-level `status` can switch to
    // `"partial"` instead of the misleading `"ok"` it used to emit.
    let mut warnings: Vec<String> = Vec::new();

    let src = match templates::templates_source_dir(mode, &cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let dst = match install_dst_dir(mode, &cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let manifest_path = match templates::default_manifest_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };

    let prev = match templates::load_manifest(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let (report, new_manifest) = match templates::copy_templates(&src, &dst, &prev) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("install: copy_templates failed: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = templates::save_manifest(&manifest_path, &new_manifest) {
        eprintln!("install: save_manifest failed: {e}");
        std::process::exit(1);
    }

    // T-06: relocate `hoangsa-memory` + `hoangsa-memory-mcp` into
    // `~/.hoangsa/bin/` (REQ-10) — same destination for both
    // `--global` and `--local`. Skipped when `--no-memory` is set, or when
    // no staging dir was handed off (normal for plain `--local` re-runs
    // where the bins were already installed globally via the curl|sh path).
    let (memory_report, memory_note): (Option<relocate::RelocateReport>, Option<String>) =
        if flags.no_memory {
            (None, Some("skipped: --no-memory".into()))
        } else if let Some(staging) = relocate::staging_dir_from_env() {
            match relocate::relocate_memory_bins(&staging) {
                Ok(r) => (Some(r), None),
                Err(e) => {
                    eprintln!("install: relocate_memory_bins failed: {e}");
                    std::process::exit(1);
                }
            }
        } else {
            (
                None,
                Some(
                    "skipped: no staging dir (set HOANGSA_STAGING_DIR or HOANGSA_TEMPLATES_DIR)"
                        .into(),
                ),
            )
        };

    // T-04: settings.json merge + statusline + legacy cleanup.
    // `dst` is already the `.claude/` dir — hooks/statusline want it verbatim.
    let target_dir = dst.clone();
    let settings_file = match hooks::settings_path(mode, &cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let mut settings = match hooks::load_settings(&settings_file) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("install: load_settings failed: {e}");
            std::process::exit(1);
        }
    };
    let settings_backup = match hooks::backup_settings(&settings_file) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("install: backup_settings failed: {e}");
            std::process::exit(1);
        }
    };
    let hoangsa_hooks = hooks::build_hoangsa_hooks(&target_dir);
    let hooks_added = hooks::merge_hoangsa_hooks(&mut settings, &hoangsa_hooks);
    let statusline_set =
        hooks::apply_statusline(&mut settings, &hooks::default_statusline(&target_dir));
    if let Err(e) = hooks::save_settings(&settings_file, &settings) {
        eprintln!("install: save_settings failed: {e}");
        std::process::exit(1);
    }

    // T-05: mode-aware MCP / rules / memory_ignore / quality-skills.
    // REQ-07 is enforced implicitly — the `Local` arm writes to `cwd`
    // and the `Global` arm writes only under `$HOME`, so no function
    // call here targets the wrong side.
    let mut mcp_target: Option<PathBuf> = None;
    let mut rules_seeded = false;
    let mut memory_ignore_seeded = false;
    let mut quality_skills_pending: Vec<String> = Vec::new();
    let mut quality_skills_present: Vec<String> = Vec::new();
    match mode {
        "global" => {
            // MCP register is a fatal step — if we can't wire memory, there's
            // no point calling the install successful.
            if let Err(e) = mode::register_mcp_global() {
                eprintln!("install: register_mcp_global failed: {e}");
                std::process::exit(1);
            }
            match mode::claude_json_path() {
                Ok(p) => mcp_target = Some(p),
                Err(e) => {
                    eprintln!("install: claude_json_path: {e}");
                    warnings.push(format!("claude_json_path: {e}"));
                }
            }
            // Quality-skills scan is optional — never block the install,
            // but feed both the pending set and any IO failure into
            // `warnings` so the top-level status reflects reality.
            match mode::install_quality_skills() {
                Ok(r) => {
                    if !r.pending.is_empty() {
                        warnings.push(format!(
                            "quality_skills pending (not auto-installed): {}",
                            r.pending.join(", ")
                        ));
                    }
                    quality_skills_pending = r.pending;
                    quality_skills_present = r.already_present;
                }
                Err(e) => {
                    eprintln!("install: install_quality_skills: {e}");
                    warnings.push(format!("install_quality_skills: {e}"));
                }
            }
        }
        "local" => {
            if let Err(e) = mode::register_mcp_local(&cwd) {
                eprintln!("install: {}", e.message);
                std::process::exit(e.exit_code);
            }
            mcp_target = Some(mode::local_mcp_path(&cwd));
            // Seed steps are optional; a failing seed must not abort the
            // install but MUST surface via `warnings` + status=partial.
            match mode::seed_local_rules(&cwd) {
                Ok(wrote) => rules_seeded = wrote,
                Err(e) => {
                    eprintln!("install: seed_local_rules: {e}");
                    warnings.push(format!("seed_local_rules: {e}"));
                }
            }
            match mode::seed_memory_ignore(&cwd) {
                Ok(wrote) => memory_ignore_seeded = wrote,
                Err(e) => {
                    eprintln!("install: seed_memory_ignore: {e}");
                    warnings.push(format!("seed_memory_ignore: {e}"));
                }
            }
        }
        _ => {}
    }

    let memory_relocated: Vec<PathBuf> = memory_report
        .as_ref()
        .map(|r| r.relocated.clone())
        .unwrap_or_default();
    let memory_skipped_missing: Vec<String> = memory_report
        .as_ref()
        .map(|r| r.skipped_missing.clone())
        .unwrap_or_default();

    // Seed project-level CLAUDE.md / AGENTS.md pointer block so Claude
    // Code + subagents know this project is memory-backed. Non-fatal —
    // a sync failure warns and lets the user re-run `hoangsa-cli
    // memory-guidance sync` by hand.
    let (guidance_synced, guidance_report) = match super::guidance::sync(&cwd) {
        Ok(r) => (true, Some(r)),
        Err(e) => {
            warnings.push(format!("memory-guidance sync failed: {e}"));
            (false, None)
        }
    };

    // Status flips to `"partial"` whenever any non-fatal step contributed
    // a warning. Fatal steps already exited above, so reaching this point
    // with an empty `warnings` vec means a clean `"ok"`.
    let status = if warnings.is_empty() { "ok" } else { "partial" };

    helpers::out(&json!({
        "status": status,
        "warnings": warnings,
        "mode": mode,
        "src": src,
        "dst": dst,
        "manifest": manifest_path,
        "copied": report.copied.len(),
        "backups": report.patched_backups.len(),
        "skipped": report.skipped.len(),
        "backups_paths": report.patched_backups,
        "settings": settings_file,
        "settings_backup": settings_backup,
        "hooks_added": hooks_added,
        "statusline_set": statusline_set,
        "memory_relocated": memory_relocated,
        "memory_skipped_missing": memory_skipped_missing,
        "memory_note": memory_note,
        "mcp_target": mcp_target,
        "rules_seeded": rules_seeded,
        "memory_ignore_seeded": memory_ignore_seeded,
        "quality_skills_present": quality_skills_present,
        "quality_skills_pending": quality_skills_pending,
        "memory_guidance_synced": guidance_synced,
        "memory_guidance_claude_updated": guidance_report.as_ref().map(|r| r.claude_md_updated),
        "memory_guidance_agents_updated": guidance_report.as_ref().map(|r| r.agents_md_updated),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_flags() {
        let f = parse_flags(&["--global", "--dry-run"]).expect("parse");
        assert!(f.global);
        assert!(f.dry_run);
        assert!(!f.local);
    }

    #[test]
    fn rejects_unknown_flag() {
        assert!(parse_flags(&["--nope"]).is_err());
    }

    #[test]
    fn task_manager_value_forms() {
        let a = parse_flags(&["--task-manager", "clickup"]).expect("space form");
        assert_eq!(a.task_manager.as_deref(), Some("clickup"));
        let b = parse_flags(&["--task-manager=asana"]).expect("equals form");
        assert_eq!(b.task_manager.as_deref(), Some("asana"));
    }

    #[test]
    fn global_and_local_are_mutually_exclusive() {
        let f = parse_flags(&["--global", "--local"]).expect("parse");
        assert!(validate(&f).is_err());
    }

    #[test]
    fn mode_derivation() {
        let f = parse_flags(&["--global"]).expect("parse");
        assert_eq!(mode_str(&f), "global");
        let f = parse_flags(&["--local"]).expect("parse");
        assert_eq!(mode_str(&f), "local");
        let f = parse_flags(&[]).expect("parse");
        assert_eq!(mode_str(&f), "local");
    }
}

#[cfg(test)]
mod templates_tests {
    //! Unit tests for the template copy + manifest + patch-backup pipeline.
    //!
    //! Every test routes through `tempfile::tempdir()` — we never touch real
    //! `~/.claude/` or `~/.hoangsa/`.

    use super::templates::*;
    use std::fs;
    use tempfile::tempdir;

    fn write(path: &std::path::Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn sha256_of_known_bytes() {
        let dir = tempdir().expect("tempdir");
        let p = dir.path().join("a.txt");
        write(&p, "hello");
        // sha256("hello") = 2cf24dba...9824
        let hash = compute_file_sha256(&p).expect("hash");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn sha256_differs_for_different_content() {
        let dir = tempdir().expect("tempdir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        write(&a, "alpha");
        write(&b, "beta");
        let ha = compute_file_sha256(&a).expect("hash a");
        let hb = compute_file_sha256(&b).expect("hash b");
        assert_ne!(ha, hb);
    }

    #[test]
    fn copy_happy_path_no_prev_manifest() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude");

        write(&src.join("top.md"), "# top");
        write(&src.join("nested/child.md"), "# child");

        let (report, manifest) = copy_templates(&src, &dst, &None).expect("copy");

        assert_eq!(report.copied.len(), 2, "both files copied on fresh install");
        assert!(report.patched_backups.is_empty());
        assert!(report.skipped.is_empty());

        assert_eq!(manifest.files.len(), 2);
        assert!(manifest.files.contains_key("top.md"));
        assert!(manifest.files.contains_key("nested/child.md"));

        // Dst files really exist with the right bytes.
        assert_eq!(
            fs::read_to_string(dst.join("top.md")).expect("read"),
            "# top"
        );
        assert_eq!(
            fs::read_to_string(dst.join("nested/child.md")).expect("read"),
            "# child"
        );
    }

    #[test]
    fn rerun_with_unchanged_files_makes_no_backup() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude");
        write(&src.join("menu.md"), "# menu v1");

        // First run: no prev manifest, everything gets copied.
        let (_first, manifest) = copy_templates(&src, &dst, &None).expect("copy 1");
        let manifest_path = tmp.path().join("manifest.json");
        save_manifest(&manifest_path, &manifest).expect("save manifest");

        // Second run: prev manifest loaded, no user edit → skip path.
        let prev = load_manifest(&manifest_path).expect("load_manifest ok");
        assert!(prev.is_some(), "manifest should roundtrip");
        let (report, _m2) = copy_templates(&src, &dst, &prev).expect("copy 2");

        assert!(
            report.patched_backups.is_empty(),
            "unchanged file must not produce a backup"
        );
        assert_eq!(
            report.copied.len(),
            0,
            "unchanged file must not be recopied"
        );
        assert_eq!(report.skipped.len(), 1);
    }

    #[test]
    fn user_modified_file_is_backed_up_then_overwritten() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude");
        write(&src.join("workflow.md"), "# upstream v1");

        // Run 1 — install v1.
        let (_r1, manifest_v1) = copy_templates(&src, &dst, &None).expect("copy v1");
        let manifest_path = tmp.path().join("manifest.json");
        save_manifest(&manifest_path, &manifest_v1).expect("save v1");

        // User locally edits the installed file.
        write(&dst.join("workflow.md"), "# user's local edit");

        // Upstream bumps the file.
        write(&src.join("workflow.md"), "# upstream v2");

        // Run 2 — should detect drift, back up the user's version, then overwrite.
        let prev = load_manifest(&manifest_path).expect("load_manifest ok");
        let (report, _m2) = copy_templates(&src, &dst, &prev).expect("copy v2");

        assert_eq!(report.patched_backups.len(), 1, "one backup expected");
        assert_eq!(report.copied.len(), 1, "file recopied with upstream v2");
        assert!(report.skipped.is_empty());

        // The backup holds the user's content.
        let backup_path = &report.patched_backups[0];
        assert!(backup_path.exists(), "backup file must exist on disk");
        let backup_contents = fs::read_to_string(backup_path).expect("read backup");
        assert_eq!(backup_contents, "# user's local edit");

        // Backup lands under <dst>/hoangsa-patches/.
        assert!(
            backup_path.starts_with(dst.join("hoangsa-patches")),
            "backup path {:?} should live under {}",
            backup_path,
            dst.join("hoangsa-patches").display()
        );

        // Destination file now has upstream v2.
        assert_eq!(
            fs::read_to_string(dst.join("workflow.md")).expect("read dst"),
            "# upstream v2"
        );
    }

    #[test]
    fn manifest_roundtrip_preserves_files() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("manifest.json");
        let mut m = Manifest::new("0.1.4");
        m.files.insert("a/b.md".into(), "deadbeef".into());
        m.files.insert("c.md".into(), "cafebabe".into());
        save_manifest(&path, &m).expect("save");
        let loaded = load_manifest(&path).expect("load ok").expect("some");
        assert_eq!(loaded, m);
    }

    #[test]
    fn load_manifest_missing_returns_none() {
        let tmp = tempdir().expect("tempdir");
        let res = load_manifest(&tmp.path().join("nope.json")).expect("missing is Ok");
        assert!(res.is_none());
    }

    #[test]
    fn load_manifest_corrupt_returns_err() {
        let tmp = tempdir().expect("tempdir");
        let path = tmp.path().join("manifest.json");
        // Write bytes that are not valid JSON for a Manifest — `load_manifest`
        // must NOT collapse this to `None` (which would look like a fresh
        // install and bypass the patch-backup gate on subsequent copies).
        std::fs::write(&path, "{ not valid json").expect("write corrupt");
        let err = load_manifest(&path).expect_err("corrupt manifest must error");
        assert!(
            err.contains("parse manifest"),
            "error should mention parse failure; got: {err}"
        );
    }

    #[test]
    fn plan_actions_lists_copies_and_backups() {
        let tmp = tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst/.claude");
        write(&src.join("a.md"), "# a v1");

        // Prime: install + snapshot manifest, then user edits.
        let (_r, m1) = copy_templates(&src, &dst, &None).expect("copy v1");
        write(&dst.join("a.md"), "# user edit");
        write(&src.join("a.md"), "# a v2");

        let actions = plan_actions(&src, &dst, &Some(m1)).expect("plan");
        let has_backup = actions.iter().any(|a| a.action == "backup");
        let has_copy = actions.iter().any(|a| a.action == "copy");
        assert!(has_backup, "should plan a backup for the edited file");
        assert!(has_copy, "should plan a copy for the fresh upstream");
    }
}
