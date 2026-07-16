// ───────────────────────── codex submodule ─────────────────────────
//
// `hoangsa-cli install --harness codex` — wire HOANGSA into OpenAI Codex
// CLI. Codex's integration surface maps onto the Claude one like so:
//
//   settings.json hooks      → ~/.codex/hooks.json (same entry shape,
//                              commands routed via `hook codex <handler>`)
//   ~/.claude.json mcpServers → [mcp_servers.hoangsa-memory] in
//                              ~/.codex/config.toml (TOML, comment-safe
//                              edits via toml_edit)
//   ~/.claude/skills/         → ~/.agents/skills/hoangsa/ (same SKILL.md
//                              standard; skills nest under a hoangsa/
//                              namespace dir)
//   ~/.claude/commands/       → wrapper skills that render the workflow
//                              through `hoangsa-cli codex render <name>`
//   ~/.claude/hoangsa/workflows → ~/.codex/hoangsa/workflows (contents
//                              adapted: .claude paths + MCP tool names)
//   statusline                → none (Codex `tui.status_line` only takes
//                              built-in ids); SessionEnd → none (archive
//                              rides PreCompact + Stop instead)
//
// IMPORTANT trust caveat: Codex refuses to run user hooks until they are
// approved via `/hooks` inside Codex. The install summary reminds the
// user; there is no supported way to pre-trust from outside.

use super::templates::{CopyReport, Manifest, compute_file_sha256, sha256_hex};
use super::*;
use crate::helpers::parse_frontmatter;

/// Resolve the Codex config dir — honors `CODEX_HOME` like Codex itself,
/// falls back to `~/.codex`. The env value is taken verbatim (no tilde
/// expansion), matching Codex's own resolution and the sibling resolvers
/// in hoangsa-proxy and hoangsa-memory-retrieve — diverging here would
/// send the three binaries to different directories.
pub fn codex_home() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os("CODEX_HOME") {
        let s = raw.to_string_lossy().into_owned();
        if !s.is_empty() {
            return Ok(PathBuf::from(s));
        }
    }
    Ok(home_path()?.join(".codex"))
}

/// `global` → `$CODEX_HOME/hooks.json`; `local` → `<cwd>/.codex/hooks.json`
/// (the project layer only loads when the project is trusted in Codex).
pub fn hooks_json_path(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
    match mode {
        "global" => Ok(codex_home()?.join("hooks.json")),
        _ => Ok(cwd.join(".codex").join("hooks.json")),
    }
}

/// `global` → `$CODEX_HOME/config.toml`; `local` → `<cwd>/.codex/config.toml`.
pub fn config_toml_path(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
    match mode {
        "global" => Ok(codex_home()?.join("config.toml")),
        _ => Ok(cwd.join(".codex").join("config.toml")),
    }
}

/// Agent Skills discovery root. `global` → `~/.agents/skills`;
/// `local` → `<cwd>/.agents/skills`.
pub fn agents_skills_dir(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
    match mode {
        "global" => Ok(home_path()?.join(".agents").join("skills")),
        _ => Ok(cwd.join(".agents").join("skills")),
    }
}

/// Root for the hoangsa-internal tree (`workflows/`, patch backups).
/// `global` → `$CODEX_HOME`; `local` → `<cwd>/.codex`.
pub fn codex_dst_dir(mode: &str, cwd: &Path) -> Result<PathBuf, String> {
    match mode {
        "global" => codex_home(),
        _ => Ok(cwd.join(".codex")),
    }
}

/// Where workflows live under a Codex root — shared by the installer
/// (write side) and `codex render` (read side) so they can't diverge.
pub fn workflows_dir(codex_root: &Path) -> PathBuf {
    codex_root.join("hoangsa").join("workflows")
}

/// Codex install manifest — separate file so Claude and Codex installs
/// never fight over hash state.
pub fn codex_manifest_path() -> Result<PathBuf, String> {
    Ok(super::memory_install_dir()?.join("manifest-codex.json"))
}

/// Build the HOANGSA-managed hook tree for Codex's `hooks.json`. Entry
/// shape is identical to Claude's (`merge_hoangsa_hooks` is reused as-is);
/// differences live in the event/matcher table:
///
///   * commands go through `hook codex <handler>` for output translation
///   * file edits match `apply_patch` next to the Claude-compat
///     `Edit|Write` aliases Codex also accepts
///   * MCP tool matchers use the sanitized server id
///     (`hoangsa-memory` → `mcp__hoangsa_memory__…`) — a hyphen would
///     flip Codex's matcher from exact to regex mode
///   * no SessionEnd (Codex has none — PreCompact + Stop cover archive)
///   * no graph-affordance (Codex has no Grep/Glob tools; searches go
///     through the shell)
pub fn build_codex_hooks_inner(install_root: Option<&Path>) -> Value {
    let cli = install_root
        .map(|d| d.join("bin").join("hoangsa-cli"))
        .unwrap_or_else(|| PathBuf::from("hoangsa-cli"))
        .display()
        .to_string();

    let managed_entry = hooks::managed_entry;

    let mut pre_tool_use = vec![
        managed_entry(
            format!("{cli} hook codex lesson-guard"),
            10,
            Some("Edit|Write|apply_patch"),
        ),
        managed_entry(
            format!("{cli} hook codex enforce"),
            10,
            Some("Edit|Write|apply_patch|Bash"),
        ),
    ];

    if let Some(hsp) = install_root
        .map(|d| d.join("bin").join("hsp"))
        .filter(|p| p.exists())
    {
        pre_tool_use.push(managed_entry(
            format!("{} hook rewrite --codex {}", hsp.display(), hooks::HSP_MARKER),
            10,
            Some("Bash"),
        ));
    }

    json!({
        "SessionStart": [
            managed_entry(format!("{cli} hook codex state-clear"), 5, None),
            managed_entry(format!("{cli} hook codex session-start"), 5, None),
        ],
        "Stop": [
            managed_entry(format!("{cli} hook codex stop-check"), 5, None),
            managed_entry(format!("{cli} hook codex session-usage"), 5, None),
            // Codex has no SessionEnd — Stop carries the archive trigger
            // instead. spawn_archive_ingest's cooldown stamp keeps the
            // per-turn firing from spawning an ingest storm.
            managed_entry(format!("{cli} hook codex session-archive"), 5, None),
        ],
        "PostToolUse": [managed_entry(
            format!("{cli} hook codex post-enforce"),
            5,
            Some("mcp__hoangsa_memory__memory_impact|mcp__hoangsa_memory__memory_detect_changes|mcp__hoangsa_memory__memory_recall|mcp__hoangsa_memory__memory_remember_lesson|Edit|Write|apply_patch"),
        )],
        "PreToolUse": pre_tool_use,
        "UserPromptSubmit": [managed_entry(format!("{cli} hook codex prompt-guard"), 5, None)],
        "PreCompact": [managed_entry(format!("{cli} hook codex session-archive"), 5, None)],
    })
}

pub fn build_codex_hooks() -> Value {
    build_codex_hooks_inner(super::memory_install_dir().ok().as_deref())
}

/// Register `hoangsa-memory-mcp` in Codex's `config.toml` under
/// `[mcp_servers.hoangsa-memory]`. Edits via `toml_edit` so user comments
/// and formatting survive. Conservative merge: `command` is always ours;
/// `args` / `env.RUST_LOG` / timeouts only fill in when absent so a
/// user-tuned value is never clobbered. Returns true when the file changed.
pub fn register_mcp_codex(config_path: &Path, mcp_bin: &Path) -> Result<bool, String> {
    let raw = match fs::read_to_string(config_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read {}: {e}", config_path.display())),
    };
    let mut doc: toml_edit::DocumentMut = raw
        .parse()
        .map_err(|e| format!("parse {}: {e}", config_path.display()))?;

    let servers = doc["mcp_servers"].or_insert(toml_edit::table());
    if let Some(t) = servers.as_table_mut() {
        // Container tables render as headers only ([mcp_servers.<id>]),
        // matching what Codex writes itself.
        t.set_implicit(true);
    }
    let entry = servers["hoangsa-memory"].or_insert(toml_edit::table());
    let Some(table) = entry.as_table_mut() else {
        return Err("mcp_servers.hoangsa-memory is not a table".into());
    };

    let mut changed = false;
    let bin = mcp_bin.display().to_string();
    if table.get("command").and_then(|v| v.as_str()) != Some(bin.as_str()) {
        table["command"] = toml_edit::value(bin);
        changed = true;
    }
    if table.get("args").is_none() {
        table["args"] = toml_edit::value(toml_edit::Array::new());
        changed = true;
    }
    if table.get("startup_timeout_sec").is_none() {
        table["startup_timeout_sec"] = toml_edit::value(20);
        changed = true;
    }
    if table.get("tool_timeout_sec").is_none() {
        table["tool_timeout_sec"] = toml_edit::value(120);
        changed = true;
    }
    let env = table["env"].or_insert(toml_edit::table());
    if let Some(env_table) = env.as_table_mut()
        && env_table.get("RUST_LOG").is_none()
    {
        env_table["RUST_LOG"] = toml_edit::value("info");
        changed = true;
    }

    if changed {
        if let Some(parent) = config_path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("mkdir: {e}"))?;
        }
        fs::write(config_path, doc.to_string()).map_err(|e| format!("write: {e}"))?;
    }
    Ok(changed)
}

/// Rewrite Claude-flavoured references in template content for Codex:
/// install paths and the MCP server tool prefix (Codex sanitizes the
/// server id's hyphen to an underscore). The `skills/hoangsa/` form must
/// be mapped before the bare `skills/` fallback — skills install to
/// `<skills_root>/hoangsa/<skill>`, so rewriting the prefix alone would
/// double the namespace segment.
pub fn codex_adapt_content(text: &str) -> String {
    text.replace("~/.claude/hoangsa/", "~/.codex/hoangsa/")
        .replace(".claude/hoangsa/", ".codex/hoangsa/")
        .replace("~/.claude/skills/hoangsa/", "~/.agents/skills/hoangsa/")
        .replace(".claude/skills/hoangsa/", ".agents/skills/hoangsa/")
        .replace("~/.claude/skills/", "~/.agents/skills/")
        .replace(".claude/skills/", ".agents/skills/")
        .replace("mcp__hoangsa-memory__", "mcp__hoangsa_memory__")
}

/// SKILL.md body for the shared command-player skill — the adapter every
/// generated command skill defers to. Owned by the installer (not a
/// template) because it is the contract of `hoangsa-cli codex render`.
pub const COMMAND_PLAYER_SKILL: &str = r#"---
name: hoangsa-command-player
description: >
  Shared runtime rules for Codex-native HOANGSA commands. Use before any
  `hoangsa-*` command skill, `/prompts:hoangsa-*` shortcut, or typed
  `/hoangsa:*` compatibility request.
---

Use this skill as the adapter between Claude-shaped HOANGSA workflows and Codex.

1. Resolve Hoangsa with `command -v hoangsa-cli`; if missing, try `$HOME/.hoangsa/bin/hoangsa-cli`.
2. Render the requested command with `hoangsa-cli codex render <command> --arguments "$ARGUMENTS"`.
3. Never read `.claude/hoangsa` or `~/.claude/hoangsa` in Codex mode.
4. Use available `memory_*` MCP tools before non-trivial edits or factual codebase claims.
5. Convert Claude `AskUserQuestion` steps into concise Codex user questions.
6. Convert Claude `Task` orchestration into explicit Codex subagent instructions; only spawn subagents when appropriate for the active session.
7. `MODEL:` lines in worker envelopes name Claude tiers — ignore them on Codex and use the session model.
8. Respect Codex sandboxing, approvals, hooks, skills, and AGENTS.md instructions.
9. Treat custom prompts as shortcuts only. The skill workflow is canonical.
"#;

/// Generate the wrapper SKILL.md for one Claude command template.
/// `name` is the bare command (`fix`), `description` the first sentence of
/// the command's own description (fallback: a generic line).
pub fn command_wrapper_skill(name: &str, description: &str) -> String {
    let summary = description
        .split_once('.')
        .map(|(head, _)| head.trim())
        .filter(|s| !s.is_empty())
        .map(|s| format!("{s}."))
        .unwrap_or_else(|| format!("Run the HOANGSA {name} workflow."));
    format!(
        r#"---
name: hoangsa-{name}
description: >
  HOANGSA Codex command for `/hoangsa:{name}`. {summary}
  Trigger when the user types `/hoangsa:{name}`, asks for `hoangsa {name}`,
  selects `/prompts:hoangsa-{name}`, or explicitly invokes `$hoangsa-{name}`.
---

First read and follow the shared `$hoangsa-command-player` skill.

Render the command prompt with:

```sh
hoangsa-cli codex render {name} --arguments "$ARGUMENTS"
```

If `$ARGUMENTS` is unavailable, pass an empty string. Follow the rendered
workflow exactly, using Codex-native questions, subagents, MCP tools, sandbox,
approvals, and hooks.
"#
    )
}

/// Copy + adapt the template tree for Codex. Routing:
///
///   skills/hoangsa/<skill>/<f> → <skills_root>/hoangsa/<skill>/<f> (adapted)
///   commands/hoangsa/<n>.md    → <skills_root>/hoangsa/hoangsa-<n>/SKILL.md (generated wrapper)
///   workflows/<f>              → <codex_root>/hoangsa/workflows/<f> (adapted)
///   agents/<f>                 → skipped (Claude Task-agent definitions;
///                                subagent conversion is a command-player rule)
///
/// Also writes the shared `hoangsa-command-player` skill. Patch backups
/// land under `<codex_root>/hoangsa-patches/` with the same modified-file
/// gate as the Claude flow; the manifest keys stay template-relative so
/// re-runs skip clean files.
pub fn install_codex_templates(
    src: &Path,
    codex_root: &Path,
    skills_root: &Path,
    prev_manifest: &Option<Manifest>,
) -> io::Result<(CopyReport, Manifest)> {
    if !src.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("template source not found: {}", src.display()),
        ));
    }

    let patch_root = codex_root.join("hoangsa-patches");
    let stamp = super::backup_timestamp();
    let mut report = CopyReport::default();
    let mut new_manifest = Manifest::new(CLI_VERSION);

    let mut jobs: Vec<(String, PathBuf, String)> = Vec::new(); // (manifest key, dst, content)

    for src_file in super::templates::walk_files(src)? {
        let rel = src_file
            .strip_prefix(src)
            .map_err(|_| io::Error::other("strip_prefix failed"))?;
        let rel_str = super::templates::rel_key(rel);

        let raw = fs::read_to_string(&src_file);
        let (dst, content) = if let Some(tail) = rel_str.strip_prefix("skills/hoangsa/") {
            let Ok(raw) = raw else {
                // Binary skill asset — copy verbatim.
                let dst = skills_root.join("hoangsa").join(tail);
                let key = format!("codex/{rel_str}");
                let src_hash = compute_file_sha256(&src_file)?;
                new_manifest.files.insert(key.clone(), src_hash.clone());
                backup_if_user_modified(&dst, &key, prev_manifest, &patch_root, &stamp, &mut report)?;
                if !dst.exists() || compute_file_sha256(&dst)? != src_hash {
                    if let Some(parent) = dst.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::copy(&src_file, &dst)?;
                    report.copied.push(dst);
                } else {
                    report.skipped.push(dst);
                }
                continue;
            };
            (
                skills_root.join("hoangsa").join(tail),
                codex_adapt_content(&raw),
            )
        } else if let Some(tail) = rel_str.strip_prefix("commands/hoangsa/") {
            let Some(name) = tail.strip_suffix(".md") else {
                continue;
            };
            let Ok(raw) = raw else { continue };
            let description = parse_frontmatter(&raw)
                .and_then(|fm| fm.get("description").cloned())
                .unwrap_or_default();
            (
                skills_root
                    .join("hoangsa")
                    .join(format!("hoangsa-{name}"))
                    .join("SKILL.md"),
                command_wrapper_skill(name, &description),
            )
        } else if let Some(tail) = rel_str.strip_prefix("workflows/") {
            let Ok(raw) = raw else { continue };
            (workflows_dir(codex_root).join(tail), codex_adapt_content(&raw))
        } else {
            // agents/ and anything unrecognized: not installed for Codex.
            continue;
        };

        jobs.push((format!("codex/{rel_str}"), dst, content));
    }

    jobs.push((
        "codex/generated/command-player".into(),
        skills_root
            .join("hoangsa")
            .join("hoangsa-command-player")
            .join("SKILL.md"),
        COMMAND_PLAYER_SKILL.to_string(),
    ));

    for (key, dst, content) in jobs {
        let content_hash = sha256_hex(content.as_bytes());
        new_manifest.files.insert(key.clone(), content_hash.clone());

        backup_if_user_modified(&dst, &key, prev_manifest, &patch_root, &stamp, &mut report)?;

        let needs_copy = !dst.exists() || compute_file_sha256(&dst)? != content_hash;
        if needs_copy {
            if let Some(parent) = dst.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&dst, &content)?;
            report.copied.push(dst);
        } else {
            report.skipped.push(dst);
        }
    }

    Ok((report, new_manifest))
}

/// Same patch-backup gate as `copy_templates`: only when the file exists,
/// the previous manifest tracked it, and the on-disk hash drifted from
/// what we last wrote.
fn backup_if_user_modified(
    dst: &Path,
    key: &str,
    prev_manifest: &Option<Manifest>,
    patch_root: &Path,
    stamp: &str,
    report: &mut CopyReport,
) -> io::Result<()> {
    if dst.exists()
        && let Some(prev) = prev_manifest
        && let Some(prev_hash) = prev.files.get(key)
    {
        let current = compute_file_sha256(dst)?;
        if &current != prev_hash {
            let backup_path = patch_root.join(format!("{key}.bak-{stamp}"));
            if let Some(parent) = backup_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(dst, &backup_path)?;
            report.patched_backups.push(backup_path);
        }
    }
    Ok(())
}


#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn codex_hooks_use_codex_dispatcher_and_sanitized_mcp_names() {
        let hooks = build_codex_hooks_inner(None);
        let post = hooks["PostToolUse"][0]["matcher"].as_str().unwrap();
        assert!(post.contains("mcp__hoangsa_memory__memory_impact"));
        assert!(!post.contains("hoangsa-memory__"), "hyphenated id would flip the matcher to regex mode");
        let cmd = hooks["SessionStart"][1]["hooks"][0]["command"].as_str().unwrap();
        assert!(cmd.ends_with("hook codex session-start"));
        assert!(hooks.get("SessionEnd").is_none(), "Codex has no SessionEnd event");
        let pre = hooks["PreToolUse"].as_array().unwrap();
        assert!(pre.iter().any(|e| e["matcher"].as_str() == Some("Edit|Write|apply_patch")));
    }

    #[test]
    fn register_mcp_creates_and_is_idempotent() {
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join("config.toml");
        let bin = Path::new("/opt/hoangsa/bin/hoangsa-memory-mcp");

        assert!(register_mcp_codex(&cfg, bin).unwrap());
        let raw = fs::read_to_string(&cfg).unwrap();
        assert!(raw.contains("[mcp_servers.hoangsa-memory]"), "{raw}");
        assert!(raw.contains("command = \"/opt/hoangsa/bin/hoangsa-memory-mcp\""));
        assert!(raw.contains("RUST_LOG"));

        // Second run: no change.
        assert!(!register_mcp_codex(&cfg, bin).unwrap());
    }

    #[test]
    fn register_mcp_preserves_user_config() {
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join("config.toml");
        fs::write(
            &cfg,
            "# my codex config\nmodel = \"gpt-5.5\"\n\n[mcp_servers.other]\ncommand = \"x\"\n",
        )
        .unwrap();
        register_mcp_codex(&cfg, Path::new("/bin/mcp")).unwrap();
        let raw = fs::read_to_string(&cfg).unwrap();
        assert!(raw.contains("# my codex config"), "comments survive: {raw}");
        assert!(raw.contains("model = \"gpt-5.5\""));
        assert!(raw.contains("[mcp_servers.other]"));
        assert!(raw.contains("[mcp_servers.hoangsa-memory]"));
    }

    #[test]
    fn register_mcp_respects_user_tuned_fields() {
        let tmp = tempdir().unwrap();
        let cfg = tmp.path().join("config.toml");
        fs::write(
            &cfg,
            "[mcp_servers.hoangsa-memory]\ncommand = \"/old/bin\"\nstartup_timeout_sec = 99\n",
        )
        .unwrap();
        register_mcp_codex(&cfg, Path::new("/new/bin")).unwrap();
        let raw = fs::read_to_string(&cfg).unwrap();
        assert!(raw.contains("command = \"/new/bin\""), "command is ours: {raw}");
        assert!(raw.contains("startup_timeout_sec = 99"), "user timeout kept: {raw}");
    }

    #[test]
    fn adapt_content_rewrites_paths_and_mcp_prefix() {
        let text = "Read `./.claude/hoangsa/workflows/fix.md` or `~/.claude/hoangsa/workflows/fix.md`.\nSkill: .claude/skills/hoangsa/git-flow/SKILL.md\nTool: mcp__hoangsa-memory__memory_recall";
        let adapted = codex_adapt_content(text);
        assert!(adapted.contains("./.codex/hoangsa/workflows/fix.md"));
        assert!(adapted.contains("~/.codex/hoangsa/workflows/fix.md"));
        // Single namespace segment — skills land at <skills_root>/hoangsa/<skill>.
        assert!(adapted.contains(".agents/skills/hoangsa/git-flow"));
        assert!(!adapted.contains("hoangsa/hoangsa/"));
        assert!(adapted.contains("mcp__hoangsa_memory__memory_recall"));
        assert!(!adapted.contains(".claude/"));
    }

    #[test]
    fn wrapper_skill_takes_first_sentence() {
        let s = command_wrapper_skill("fix", "Hotfix — analyze bug → fix. More detail here.");
        assert!(s.contains("name: hoangsa-fix"));
        assert!(s.contains("Hotfix — analyze bug → fix."));
        assert!(!s.contains("More detail here"));
        assert!(s.contains("codex render fix"));
    }

    #[test]
    fn wrapper_skill_handles_empty_description() {
        let s = command_wrapper_skill("qc", "");
        assert!(s.contains("Run the HOANGSA qc workflow."));
    }

    #[test]
    fn templates_route_and_adapt() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("templates");
        fs::create_dir_all(src.join("skills/hoangsa/git-flow")).unwrap();
        fs::write(
            src.join("skills/hoangsa/git-flow/SKILL.md"),
            "---\nname: git-flow\n---\nsee ~/.claude/hoangsa/workflows/x.md",
        )
        .unwrap();
        fs::create_dir_all(src.join("commands/hoangsa")).unwrap();
        fs::write(
            src.join("commands/hoangsa/fix.md"),
            "---\nname: hoangsa:fix\ndescription: Hotfix flow. Long tail.\n---\nbody",
        )
        .unwrap();
        fs::create_dir_all(src.join("workflows")).unwrap();
        fs::write(src.join("workflows/fix.md"), "use mcp__hoangsa-memory__memory_recall").unwrap();
        fs::create_dir_all(src.join("agents")).unwrap();
        fs::write(src.join("agents/worker.md"), "claude agent").unwrap();

        let codex_root = tmp.path().join("codex");
        let skills_root = tmp.path().join("agents-skills");
        let (report, manifest) =
            install_codex_templates(&src, &codex_root, &skills_root, &None).unwrap();

        let skill = fs::read_to_string(skills_root.join("hoangsa/git-flow/SKILL.md")).unwrap();
        assert!(skill.contains("~/.codex/hoangsa/workflows/x.md"));

        let wrapper = fs::read_to_string(skills_root.join("hoangsa/hoangsa-fix/SKILL.md")).unwrap();
        assert!(wrapper.contains("codex render fix"));
        assert!(wrapper.contains("Hotfix flow."));

        let wf = fs::read_to_string(codex_root.join("hoangsa/workflows/fix.md")).unwrap();
        assert!(wf.contains("mcp__hoangsa_memory__memory_recall"));

        let player = skills_root.join("hoangsa/hoangsa-command-player/SKILL.md");
        assert!(player.exists());

        assert!(!codex_root.join("hoangsa/agents").exists());
        assert!(!skills_root.join("hoangsa/worker.md").exists());
        assert!(report.copied.len() >= 4);
        assert!(manifest.files.keys().all(|k| k.starts_with("codex/")));

        // Re-run: everything skips.
        let (report2, _) =
            install_codex_templates(&src, &codex_root, &skills_root, &Some(manifest)).unwrap();
        assert!(report2.copied.is_empty(), "copied: {:?}", report2.copied);
        assert!(report2.patched_backups.is_empty());
    }

    #[test]
    fn user_modified_file_gets_backed_up() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("templates");
        fs::create_dir_all(src.join("workflows")).unwrap();
        fs::write(src.join("workflows/fix.md"), "v1").unwrap();

        let codex_root = tmp.path().join("codex");
        let skills_root = tmp.path().join("skills");
        let (_, manifest) =
            install_codex_templates(&src, &codex_root, &skills_root, &None).unwrap();

        // User edits the installed copy; template also changes.
        fs::write(codex_root.join("hoangsa/workflows/fix.md"), "user edit").unwrap();
        fs::write(src.join("workflows/fix.md"), "v2").unwrap();

        let (report, _) =
            install_codex_templates(&src, &codex_root, &skills_root, &Some(manifest)).unwrap();
        assert_eq!(report.patched_backups.len(), 1, "{:?}", report.patched_backups);
        let restored = fs::read_to_string(&report.patched_backups[0]).unwrap();
        assert_eq!(restored, "user edit");
    }
}
