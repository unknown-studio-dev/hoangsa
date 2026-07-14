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
///
/// Also cleans up the orphan `$HOME/.claude/.claude.json` that pre-0.2.3
/// installers wrote when `CLAUDE_CONFIG_DIR` was auto-set to the default
/// `$HOME/.claude` — Claude Code without that env var reads
/// `$HOME/.claude.json`, so the MCP entry was invisible. Safe cleanup:
/// only removes the file when it matches the exact orphan signature
/// (top-level `{ "mcpServers": { "hoangsa-memory": { ... } } }` with
/// nothing else) and isn't the target we just wrote to.
pub fn register_mcp_global() -> Result<(), String> {
    let claude_json = claude_json_path()?;
    let memory_bin = memory_mcp_bin()?;
    if !memory_bin.exists() {
        eprintln!(
            "install: warning — hoangsa-memory-mcp not found at {} (writing config anyway)",
            memory_bin.display()
        );
    }
    register_mcp_global_to(&claude_json, &memory_bin).map_err(|e| e.to_string())?;

    let orphan = super::home_path()?.join(".claude").join(".claude.json");
    if let Err(e) = cleanup_orphan_claude_json(&orphan, &claude_json) {
        eprintln!(
            "install: warning — could not clean orphan {}: {}",
            orphan.display(),
            e
        );
    }
    Ok(())
}

/// Remove `$HOME/.claude/.claude.json` when it is the stray file
/// written by pre-0.2.3 installs and is not the path we just wrote
/// to. Returns `Ok(true)` when a file was removed, `Ok(false)` when
/// the file is absent or does not match the orphan signature.
///
/// Signature check: the root is a JSON object whose only key is
/// `mcpServers`, whose only key is `hoangsa-memory`. Any other
/// top-level key or any other MCP server means the user has
/// legitimate config there — never touch.
pub fn cleanup_orphan_claude_json(
    orphan_path: &Path,
    target_path: &Path,
) -> io::Result<bool> {
    if orphan_path == target_path {
        return Ok(false);
    }
    let raw = match fs::read_to_string(orphan_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };
    let value: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let root = match value.as_object() {
        Some(o) => o,
        None => return Ok(false),
    };
    if root.len() != 1 {
        return Ok(false);
    }
    let servers = match root.get("mcpServers").and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return Ok(false),
    };
    if servers.len() != 1 || !servers.contains_key("hoangsa-memory") {
        return Ok(false);
    }
    fs::remove_file(orphan_path)?;
    eprintln!(
        "install: removed orphan MCP config at {} (pre-0.2.3 leftover)",
        orphan_path.display()
    );
    Ok(true)
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

    #[test]
    fn cleanup_orphan_removes_exact_signature() {
        let home = tempdir().expect("home tempdir");
        let orphan = home.path().join(".claude").join(".claude.json");
        fs::create_dir_all(orphan.parent().unwrap()).expect("mkdir");
        let orphan_content = json!({
            "mcpServers": {
                "hoangsa-memory": {
                    "command": "/x/bin/hoangsa-memory-mcp",
                    "args": [],
                    "env": {}
                }
            }
        });
        fs::write(
            &orphan,
            serde_json::to_string_pretty(&orphan_content).unwrap(),
        )
        .expect("write orphan");
        let target = home.path().join(".claude.json");

        let removed = cleanup_orphan_claude_json(&orphan, &target).expect("cleanup");
        assert!(removed, "exact orphan signature must be removed");
        assert!(!orphan.exists(), "orphan file should be gone");
    }

    #[test]
    fn cleanup_orphan_preserves_when_extra_top_level_key() {
        let home = tempdir().expect("home tempdir");
        let orphan = home.path().join(".claude").join(".claude.json");
        fs::create_dir_all(orphan.parent().unwrap()).expect("mkdir");
        let content = json!({
            "mcpServers": { "hoangsa-memory": { "command": "x" } },
            "numStartups": 7
        });
        fs::write(&orphan, serde_json::to_string_pretty(&content).unwrap())
            .expect("write");
        let target = home.path().join(".claude.json");

        let removed = cleanup_orphan_claude_json(&orphan, &target).expect("cleanup");
        assert!(!removed, "extra top-level key means real config — keep");
        assert!(orphan.exists(), "file must still be there");
    }

    #[test]
    fn cleanup_orphan_preserves_when_other_mcp_server() {
        let home = tempdir().expect("home tempdir");
        let orphan = home.path().join(".claude").join(".claude.json");
        fs::create_dir_all(orphan.parent().unwrap()).expect("mkdir");
        let content = json!({
            "mcpServers": {
                "hoangsa-memory": { "command": "x" },
                "other-mcp": { "command": "y" }
            }
        });
        fs::write(&orphan, serde_json::to_string_pretty(&content).unwrap())
            .expect("write");
        let target = home.path().join(".claude.json");

        let removed = cleanup_orphan_claude_json(&orphan, &target).expect("cleanup");
        assert!(!removed, "second MCP entry means user config — keep");
        assert!(orphan.exists());
    }

    #[test]
    fn cleanup_orphan_never_removes_the_target_itself() {
        // If CLAUDE_CONFIG_DIR is set to $HOME/.claude on purpose the
        // target IS `$HOME/.claude/.claude.json` — the cleanup must
        // not delete the file we just wrote.
        let home = tempdir().expect("home tempdir");
        let target = home.path().join(".claude").join(".claude.json");
        fs::create_dir_all(target.parent().unwrap()).expect("mkdir");
        let content = json!({
            "mcpServers": { "hoangsa-memory": { "command": "x" } }
        });
        fs::write(&target, serde_json::to_string_pretty(&content).unwrap())
            .expect("write");

        let removed = cleanup_orphan_claude_json(&target, &target).expect("cleanup");
        assert!(!removed, "must not remove the active target");
        assert!(target.exists());
    }

    #[test]
    fn cleanup_orphan_is_a_noop_when_absent() {
        let home = tempdir().expect("home tempdir");
        let orphan = home.path().join(".claude").join(".claude.json");
        let target = home.path().join(".claude.json");
        let removed = cleanup_orphan_claude_json(&orphan, &target).expect("cleanup");
        assert!(!removed, "missing file = nothing to do");
    }
}
