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

/// Derive an install root from a binary path. Returns `Some` only when
/// the binary lives in an installed layout `<root>/bin/<name>` — i.e.
/// the immediate parent is literally named `bin`. This guard prevents
/// `cargo run -- install` (binary at `target/debug/hoangsa-cli`) from
/// accidentally reporting `target/debug` as the install root.
///
/// Pure function on paths so it's unit-testable without touching
/// `std::env::current_exe`.
fn derive_install_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let parent = exe.parent()?;
    if parent.file_name()?.to_str()? != "bin" {
        return None;
    }
    parent.parent().map(Path::to_path_buf)
}

/// Root directory for the installed `hoangsa-memory` tree (bins + manifest).
///
/// Resolution order (first match wins):
///   1. `HOANGSA_INSTALL_DIR` env var — explicit override, honored verbatim.
///      Users who want per-Claude-profile installs (e.g. alongside
///      `zclaude='CLAUDE_CONFIG_DIR=~/.zclaude claude'`) set this inline
///      in their alias, NOT in `.zshrc` (the installer deliberately does
///      not persist this to rc — a global env would collide across
///      profiles).
///   2. Derive from `current_exe()` — canonicalize to resolve PATH
///      shim symlinks (e.g. `/usr/local/bin/hoangsa-cli` →
///      `~/.hoangsa/bin/hoangsa-cli`), then accept only when the parent
///      is literally `bin` (see `derive_install_root_from_exe`).
///      Makes non-default installs work in fresh shells without any env.
///   3. `$HOME/.hoangsa` — last-resort default for dev runs
///      (`cargo run`), exotic layouts, or when `current_exe` fails.
fn memory_install_dir() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("HOANGSA_INSTALL_DIR") {
        let s = raw.to_string_lossy().into_owned();
        if !s.is_empty() {
            return Ok(PathBuf::from(s));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // canonicalize follows symlinks so PATH shims resolve to the
        // real installed binary; fall back to the raw path if
        // canonicalize fails (e.g. deleted file races).
        let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
        if let Some(root) = derive_install_root_from_exe(&resolved) {
            return Ok(root);
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
    /// Value of `--harness[=<claude|codex>]`; defaults to claude.
    harness: Option<String>,
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
            "--harness" => {
                i += 1;
                if i >= args.len() {
                    return Err("--harness requires a value (claude|codex)".into());
                }
                f.harness = Some(args[i].to_string());
            }
            s if s.starts_with("--harness=") => {
                f.harness = Some(s["--harness=".len()..].to_string());
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
    if let Some(h) = f.harness.as_deref()
        && !matches!(h, "claude" | "codex" | "cowork")
    {
        return Err(format!("--harness must be claude, codex or cowork, got: {h}"));
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

pub mod codex;
pub mod hooks;
pub mod mode;
pub mod relocate;
pub mod templates;

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

    if flags.harness.as_deref() == Some("codex") {
        cmd_install_codex(&flags, mode, &cwd);
        return;
    }
    if flags.harness.as_deref() == Some("cowork") {
        cmd_install_cowork(&flags);
        return;
    }

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

/// `--harness cowork` — Claude Cowork runs tasks in a sandboxed VM, but
/// Claude Desktop bridges stdio MCP servers from
/// `claude_desktop_config.json` into it. Registering `hoangsa-memory`
/// there is the whole integration: hooks / skills / commands have no
/// Cowork surface outside plugin bundles (not shipped yet), and the
/// hoangsa CLI on the host is unreachable from inside the VM.
fn cmd_install_cowork(flags: &InstallFlags) {
    // Claude Desktop's config is per-user; there is no project scope, and
    // MCP registration is the flow's only action — reject flags that
    // would otherwise be silently ignored.
    if flags.local {
        eprintln!("install: --harness cowork is global-only (claude_desktop_config.json has no project scope)");
        std::process::exit(2);
    }
    if flags.no_memory {
        eprintln!("install: --harness cowork only registers the memory MCP server — nothing to do with --no-memory");
        std::process::exit(2);
    }
    let config = match mode::claude_desktop_config_path() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let mcp_bin = match mode::memory_mcp_bin() {
        Ok(b) => b,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };

    if flags.dry_run {
        helpers::out(&json!({
            "harness": "cowork",
            "actions": [{ "action": "register_mcp_desktop", "target": config }],
        }));
        return;
    }

    if !mcp_bin.exists() {
        eprintln!(
            "install: hoangsa-memory-mcp missing at {} — run the release installer first",
            mcp_bin.display()
        );
        std::process::exit(3);
    }
    if let Err(e) = mode::register_mcp_global_to(&config, &mcp_bin) {
        eprintln!("install: register desktop MCP failed: {e}");
        std::process::exit(1);
    }
    helpers::out(&json!({
        "status": "ok",
        "harness": "cowork",
        "mcp_target": config,
        "next_step": "Restart Claude Desktop so Cowork picks up the hoangsa-memory MCP server.",
    }));
}

/// Live + dry-run install flow for `--harness codex`. Deliberately a
/// separate function from the Claude flow — the two share the template
/// source and MCP binary but nothing about destinations, so interleaving
/// them behind `mode` matches would obscure both.
fn cmd_install_codex(flags: &InstallFlags, mode: &str, cwd: &Path) {
    let mut warnings: Vec<String> = Vec::new();

    let src = match templates::templates_source_dir(mode, cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let (codex_root, skills_root, hooks_file, config_file, manifest_path) = match (
        codex::codex_dst_dir(mode, cwd),
        codex::agents_skills_dir(mode, cwd),
        codex::hooks_json_path(mode, cwd),
        codex::config_toml_path(mode, cwd),
        codex::codex_manifest_path(),
    ) {
        (Ok(a), Ok(b), Ok(c), Ok(d), Ok(e)) => (a, b, c, d, e),
        (a, b, c, d, e) => {
            for err in [a.err(), b.err(), c.err(), d.err(), e.err()].into_iter().flatten() {
                eprintln!("install: {err}");
            }
            std::process::exit(1);
        }
    };

    if flags.dry_run {
        helpers::out(&json!({
            "mode": mode,
            "harness": "codex",
            "actions": [
                { "action": "copy_templates_codex", "src": src, "codex_root": codex_root, "skills_root": skills_root },
                { "action": "merge_hooks_json", "target": hooks_file },
                { "action": "register_mcp_codex", "target": config_file },
                { "action": "memory_guidance_sync", "target": cwd.join("AGENTS.md") },
            ],
            "note": "Codex requires approving the installed hooks once via /hooks inside Codex.",
        }));
        return;
    }

    let prev = match templates::load_manifest(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("install: {e}");
            std::process::exit(1);
        }
    };
    let (report, new_manifest) =
        match codex::install_codex_templates(&src, &codex_root, &skills_root, &prev) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("install: install_codex_templates failed: {e}");
                std::process::exit(1);
            }
        };
    if let Err(e) = templates::save_manifest(&manifest_path, &new_manifest) {
        eprintln!("install: save_manifest failed: {e}");
        std::process::exit(1);
    }

    // hooks.json shares the Claude entry shape, so the whole document is
    // treated as a settings object with a single `hooks` key and merged
    // with the same idempotent machinery (stale hoangsa entries — even
    // hand-written ones referencing `hoangsa-cli` — get replaced;
    // user-authored entries survive).
    let mut hooks_doc = match hooks::load_settings(&hooks_file) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("install: load hooks.json failed: {e}");
            std::process::exit(1);
        }
    };
    let hooks_added = hooks::merge_hoangsa_hooks(&mut hooks_doc, &codex::build_codex_hooks());
    if let Err(e) = hooks::save_settings(&hooks_file, &hooks_doc) {
        eprintln!("install: save hooks.json failed: {e}");
        std::process::exit(1);
    }

    // MCP registration is fatal like the Claude flow — memory is the
    // point — unless the user opted out with --no-memory.
    let mut mcp_changed = false;
    if !flags.no_memory {
        let mcp_bin = match mode::memory_mcp_bin() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("install: {e}");
                std::process::exit(1);
            }
        };
        if !mcp_bin.exists() {
            eprintln!(
                "install: hoangsa-memory-mcp missing at {} — run the release installer first",
                mcp_bin.display()
            );
            std::process::exit(3);
        }
        mcp_changed = match codex::register_mcp_codex(&config_file, &mcp_bin) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("install: register_mcp_codex failed: {e}");
                std::process::exit(1);
            }
        };
    }

    // Local-mode seeds are harness-agnostic (.hoangsa/rules.json, .memoryignore).
    let mut rules_seeded = false;
    let mut memory_ignore_seeded = false;
    if mode == "local" {
        match mode::seed_local_rules(cwd) {
            Ok(wrote) => rules_seeded = wrote,
            Err(e) => warnings.push(format!("seed_local_rules: {e}")),
        }
        match mode::seed_memory_ignore(cwd) {
            Ok(wrote) => memory_ignore_seeded = wrote,
            Err(e) => warnings.push(format!("seed_memory_ignore: {e}")),
        }
    }

    let (guidance_synced, guidance_report) = match super::guidance::sync(cwd) {
        Ok(r) => (true, Some(r)),
        Err(e) => {
            warnings.push(format!("memory-guidance sync failed: {e}"));
            (false, None)
        }
    };

    let status = if warnings.is_empty() { "ok" } else { "partial" };
    helpers::out(&json!({
        "status": status,
        "harness": "codex",
        "warnings": warnings,
        "mode": mode,
        "src": src,
        "codex_root": codex_root,
        "skills_root": skills_root,
        "manifest": manifest_path,
        "copied": report.copied.len(),
        "backups": report.patched_backups.len(),
        "skipped": report.skipped.len(),
        "backups_paths": report.patched_backups,
        "hooks_file": hooks_file,
        "hooks_added": hooks_added,
        "mcp_target": (!flags.no_memory).then_some(&config_file),
        "mcp_changed": mcp_changed,
        "rules_seeded": rules_seeded,
        "memory_ignore_seeded": memory_ignore_seeded,
        "memory_guidance_synced": guidance_synced,
        "memory_guidance_agents_updated": guidance_report.as_ref().map(|r| r.agents_md_updated),
        "next_step": "Open Codex and run /hooks once to approve the hoangsa hooks — Codex does not execute untrusted hooks.",
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_install_root_accepts_standard_bin_layout() {
        let exe = PathBuf::from("/opt/hoangsa/bin/hoangsa-cli");
        assert_eq!(
            derive_install_root_from_exe(&exe),
            Some(PathBuf::from("/opt/hoangsa"))
        );
    }

    #[test]
    fn derive_install_root_rejects_cargo_target_layout() {
        // `cargo run -- install` binary lives at target/debug/hoangsa-cli
        // (no `bin/` dir) — must NOT derive target/debug as install root.
        let exe = PathBuf::from("/workspace/target/debug/hoangsa-cli");
        assert_eq!(derive_install_root_from_exe(&exe), None);
    }

    #[test]
    fn derive_install_root_rejects_wrong_parent_name() {
        // Anything not literally named `bin` must be rejected (e.g. a
        // user's `~/scripts/hoangsa-cli` wrapper).
        let exe = PathBuf::from("/home/u/scripts/hoangsa-cli");
        assert_eq!(derive_install_root_from_exe(&exe), None);
    }

    #[test]
    fn derive_install_root_handles_nested_install_root() {
        let exe = PathBuf::from("/tmp/profile-a/.hoangsa/bin/hoangsa-cli");
        assert_eq!(
            derive_install_root_from_exe(&exe),
            Some(PathBuf::from("/tmp/profile-a/.hoangsa"))
        );
    }

    #[test]
    fn derive_install_root_rejects_root_level_binary() {
        // `/hoangsa-cli` with no parent can never be in a <root>/bin layout.
        let exe = PathBuf::from("/hoangsa-cli");
        assert_eq!(derive_install_root_from_exe(&exe), None);
    }

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
