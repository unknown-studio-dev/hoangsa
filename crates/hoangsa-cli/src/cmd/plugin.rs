//! `hoangsa-cli plugin package [--out <dir>]` — assemble the HOANGSA
//! Claude plugin from `templates/`.
//!
//! The plugin is the instruction layer only — commands, skills, agents,
//! and the workflows they reference, with paths rewritten to
//! `${CLAUDE_PLUGIN_ROOT}`. Hooks and `.mcp.json` are deliberately NOT
//! bundled:
//!
//!   * hooks would double-fire on Claude Code (the installer already
//!     writes them to settings.json), and inside Claude Cowork's task VM
//!     the host `hoangsa-cli` binary is unreachable anyway;
//!   * the MCP server is registered by the installer (`.claude.json`,
//!     Codex `config.toml`, `--harness cowork` for Claude Desktop) —
//!     bundling it would register the same server twice.
//!
//! The generated tree is checked in at `plugin/` and referenced by the
//! repo-root `.claude-plugin/marketplace.json`, so
//! `/plugin marketplace add pirumu/hoangsa` works from git. Regenerate
//! with `make plugin` before a release.

use crate::helpers::out;
use serde_json::json;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Rewrite install-path references in template content to the plugin's
/// own tree. Workflows/skills live inside the plugin, so both the
/// project-local and global `.claude/` forms collapse to
/// `${CLAUDE_PLUGIN_ROOT}`.
fn plugin_adapt_content(text: &str) -> String {
    text.replace(
        "~/.claude/hoangsa/workflows/",
        "${CLAUDE_PLUGIN_ROOT}/workflows/",
    )
    .replace(
        "./.claude/hoangsa/workflows/",
        "${CLAUDE_PLUGIN_ROOT}/workflows/",
    )
    .replace(
        ".claude/hoangsa/workflows/",
        "${CLAUDE_PLUGIN_ROOT}/workflows/",
    )
    .replace("~/.claude/skills/hoangsa/", "${CLAUDE_PLUGIN_ROOT}/skills/")
    .replace(".claude/skills/hoangsa/", "${CLAUDE_PLUGIN_ROOT}/skills/")
}

/// Strip the `name:` line from a command's frontmatter — plugin commands
/// are namespaced by file path under the plugin name (`commands/fix.md`
/// in plugin `hoangsa` → `/hoangsa:fix`), so a literal `name: hoangsa:fix`
/// would fight the path-derived name.
fn strip_frontmatter_name(content: &str) -> String {
    let Some(rest) = content.strip_prefix("---\n") else {
        return content.to_string();
    };
    let Some(end) = rest.find("\n---") else {
        return content.to_string();
    };
    let (fm, tail) = rest.split_at(end);
    let kept: Vec<&str> = fm
        .lines()
        .filter(|l| !l.trim_start().starts_with("name:"))
        .collect();
    format!("---\n{}{}", kept.join("\n"), tail)
}

fn plugin_manifest() -> serde_json::Value {
    json!({
        "name": "hoangsa",
        "description": "HOANGSA context engineering — /hoangsa:* workflow commands, worker agents, and memory-discipline skills. Pair with the hoangsa installer for enforcement hooks and the hoangsa-memory MCP server.",
        "version": PLUGIN_VERSION,
        "author": { "name": "hoangsa" },
        "homepage": "https://github.com/unknown-studio-dev/hoangsa",
        "repository": "https://github.com/unknown-studio-dev/hoangsa",
        "license": "MIT",
        "keywords": ["workflow", "memory", "context-engineering"],
    })
}

/// Assemble the plugin tree from `templates_src` into `out_dir`.
///
/// Routing:
///   commands/hoangsa/<n>.md → commands/<n>.md (frontmatter name stripped)
///   skills/hoangsa/<s>/**   → skills/<s>/**
///   agents/**               → agents/**
///   workflows/**            → workflows/**
/// All text content goes through `plugin_adapt_content`.
pub fn package(templates_src: &Path, out_dir: &Path) -> io::Result<Vec<PathBuf>> {
    if out_dir.exists() {
        // Only wipe a directory we recognizably own — a previous package
        // output carries `.claude-plugin/plugin.json`.
        let manifest = out_dir.join(".claude-plugin").join("plugin.json");
        if manifest.exists() {
            fs::remove_dir_all(out_dir)?;
        } else if fs::read_dir(out_dir)?.next().is_some() {
            return Err(io::Error::other(format!(
                "refusing to overwrite non-plugin directory: {}",
                out_dir.display()
            )));
        }
    }

    let mut written = Vec::new();
    let mut write = |rel: PathBuf, content: &[u8]| -> io::Result<()> {
        let dst = out_dir.join(&rel);
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&dst, content)?;
        written.push(rel);
        Ok(())
    };

    write(
        Path::new(".claude-plugin").join("plugin.json"),
        format!("{}\n", serde_json::to_string_pretty(&plugin_manifest())?).as_bytes(),
    )?;

    for src_file in super::install::templates::walk_files(templates_src)? {
        let rel = src_file
            .strip_prefix(templates_src)
            .map_err(|_| io::Error::other("strip_prefix failed"))?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");

        let dst_rel = if let Some(tail) = rel_str.strip_prefix("commands/hoangsa/") {
            Path::new("commands").join(tail)
        } else if let Some(tail) = rel_str.strip_prefix("skills/hoangsa/") {
            Path::new("skills").join(tail)
        } else if rel_str.starts_with("agents/") || rel_str.starts_with("workflows/") {
            rel.to_path_buf()
        } else {
            continue;
        };

        let bytes = fs::read(&src_file)?;
        match String::from_utf8(bytes) {
            Ok(text) => {
                let mut adapted = plugin_adapt_content(&text);
                if rel_str.starts_with("commands/hoangsa/") {
                    adapted = strip_frontmatter_name(&adapted);
                }
                write(dst_rel, adapted.as_bytes())?;
            }
            // Binary asset (skill resources) — copy verbatim.
            Err(e) => write(dst_rel, e.as_bytes())?,
        }
    }

    Ok(written)
}

pub fn cmd_plugin_package(rest: &[&str], cwd: &str) {
    let out_dir = rest
        .iter()
        .position(|&a| a == "--out")
        .and_then(|i| rest.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(cwd).join("plugin"));

    let src = match super::install::templates::templates_source_dir("local", Path::new(cwd)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("plugin package: {e}");
            std::process::exit(1);
        }
    };

    match package(&src, &out_dir) {
        Ok(written) => out(&json!({
            "status": "ok",
            "out": out_dir,
            "files": written.len(),
        })),
        Err(e) => {
            eprintln!("plugin package: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn seed_templates(root: &Path) {
        fs::create_dir_all(root.join("commands/hoangsa")).unwrap();
        fs::write(
            root.join("commands/hoangsa/fix.md"),
            "---\nname: hoangsa:fix\ndescription: Hotfix flow\n---\nRead `./.claude/hoangsa/workflows/fix.md` or `~/.claude/hoangsa/workflows/fix.md`.",
        )
        .unwrap();
        fs::create_dir_all(root.join("skills/hoangsa/git-flow")).unwrap();
        fs::write(
            root.join("skills/hoangsa/git-flow/SKILL.md"),
            "---\nname: git-flow\n---\nSee .claude/skills/hoangsa/visual-debug/SKILL.md",
        )
        .unwrap();
        fs::create_dir_all(root.join("workflows")).unwrap();
        fs::write(root.join("workflows/fix.md"), "workflow body").unwrap();
        fs::create_dir_all(root.join("agents")).unwrap();
        fs::write(
            root.join("agents/hoangsa-worker-impl.md"),
            "---\nmodel: sonnet\n---\nworker",
        )
        .unwrap();
    }

    #[test]
    fn package_routes_and_adapts() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("templates");
        seed_templates(&src);
        let out_dir = tmp.path().join("plugin");

        package(&src, &out_dir).unwrap();

        let manifest = fs::read_to_string(out_dir.join(".claude-plugin/plugin.json")).unwrap();
        assert!(manifest.contains("\"name\": \"hoangsa\""));

        let cmd = fs::read_to_string(out_dir.join("commands/fix.md")).unwrap();
        assert!(
            !cmd.contains("name: hoangsa:fix"),
            "frontmatter name stripped: {cmd}"
        );
        assert!(cmd.contains("description: Hotfix flow"));
        assert!(cmd.contains("${CLAUDE_PLUGIN_ROOT}/workflows/fix.md"));
        assert!(!cmd.contains(".claude/"));

        let skill = fs::read_to_string(out_dir.join("skills/git-flow/SKILL.md")).unwrap();
        assert!(skill.contains("${CLAUDE_PLUGIN_ROOT}/skills/visual-debug/SKILL.md"));

        assert!(out_dir.join("workflows/fix.md").exists());
        assert!(out_dir.join("agents/hoangsa-worker-impl.md").exists());
    }

    #[test]
    fn package_is_rerunnable_but_refuses_foreign_dirs() {
        let tmp = tempdir().unwrap();
        let src = tmp.path().join("templates");
        seed_templates(&src);

        let out_dir = tmp.path().join("plugin");
        package(&src, &out_dir).unwrap();
        // Stale file from a previous layout must not survive a repackage.
        fs::write(out_dir.join("commands/stale.md"), "old").unwrap();
        package(&src, &out_dir).unwrap();
        assert!(!out_dir.join("commands/stale.md").exists());

        let foreign = tmp.path().join("not-a-plugin");
        fs::create_dir_all(&foreign).unwrap();
        fs::write(foreign.join("keep.txt"), "user data").unwrap();
        assert!(package(&src, &foreign).is_err());
        assert!(foreign.join("keep.txt").exists());
    }

    #[test]
    fn strip_frontmatter_name_only_touches_name() {
        let s = strip_frontmatter_name("---\nname: x\ndescription: d\n---\nbody name: y");
        assert!(!s.contains("name: x"));
        assert!(s.contains("description: d"));
        assert!(s.contains("body name: y"));
        // No frontmatter → untouched.
        assert_eq!(strip_frontmatter_name("plain"), "plain");
    }
}
