//! `hoangsa-cli codex render <command> [--arguments "…"]`
//!
//! Runtime half of the Codex command skills: each generated
//! `hoangsa-<command>` skill (see `install::codex`) tells the agent to run
//! this and follow the output. Rendering resolves the workflow the same
//! way the Claude slash commands do — project copy first, then the global
//! install — but against the Codex tree, and emits plain text (this IS the
//! prompt, not a machine-readable result).

use std::fs;
use std::path::{Path, PathBuf};

/// Resolve `<name>.md` under the project then global Codex workflow
/// trees — derived from the installer's own path helpers so the read
/// side can't drift from where install actually writes.
fn workflow_candidates(name: &str, cwd: &str) -> Vec<PathBuf> {
    use super::install::codex::{codex_dst_dir, workflows_dir};
    let file = format!("{name}.md");
    let mut candidates = Vec::new();
    if let Ok(root) = codex_dst_dir("local", Path::new(cwd)) {
        candidates.push(workflows_dir(&root).join(&file));
    }
    if let Ok(root) = codex_dst_dir("global", Path::new(cwd)) {
        candidates.push(workflows_dir(&root).join(&file));
    }
    candidates
}

/// Reject names that could escape the workflows dir (`../`, absolute,
/// separators). Workflow names are single path segments like `fix`.
fn valid_workflow_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn cmd_codex_render(rest: &[&str], cwd: &str) {
    let Some(&name) = rest.first() else {
        eprintln!("usage: hoangsa-cli codex render <command> [--arguments \"…\"]");
        std::process::exit(2);
    };
    if !valid_workflow_name(name) {
        eprintln!("codex render: invalid command name: {name}");
        std::process::exit(2);
    }
    let arguments = rest
        .iter()
        .position(|&a| a == "--arguments")
        .and_then(|i| rest.get(i + 1))
        .copied()
        .unwrap_or("");

    let candidates = workflow_candidates(name, cwd);
    let Some(found) = candidates.iter().find(|p| p.is_file()) else {
        eprintln!(
            "codex render: workflow '{name}' not found. Looked in:\n{}\nRun `hoangsa-cli install --harness codex` first.",
            candidates
                .iter()
                .map(|p| format!("  {}", p.display()))
                .collect::<Vec<_>>()
                .join("\n")
        );
        std::process::exit(1);
    };

    let body = match fs::read_to_string(found) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("codex render: read {}: {e}", found.display());
            std::process::exit(1);
        }
    };

    println!("# HOANGSA /{name} — rendered for Codex");
    println!("# source: {}", found.display());
    if !arguments.trim().is_empty() {
        println!("\nARGUMENTS: {}", arguments.trim());
    }
    println!("\n{body}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workflow_name_validation() {
        assert!(valid_workflow_name("fix"));
        assert!(valid_workflow_name("task-link"));
        assert!(valid_workflow_name("init_detect"));
        assert!(!valid_workflow_name(""));
        assert!(!valid_workflow_name("../etc/passwd"));
        assert!(!valid_workflow_name("a/b"));
        assert!(!valid_workflow_name("fix.md"));
    }

    #[test]
    fn candidates_prefer_project_tree() {
        let c = workflow_candidates("fix", "/tmp/proj");
        assert!(c[0].ends_with(".codex/hoangsa/workflows/fix.md"));
        assert!(c[0].starts_with("/tmp/proj"));
    }
}
