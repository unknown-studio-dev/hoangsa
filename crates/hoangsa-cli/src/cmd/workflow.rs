//! `hoangsa-cli workflow show <name>` — resolve and print a workflow file.
//!
//! Every `/hoangsa:*` command used to carry an eight-line block explaining
//! where to look for its workflow: project-local, then `CLAUDE_CONFIG_DIR`,
//! then `~/.claude`. Nineteen copies of the same prose, differing only in a
//! filename — so when the `CLAUDE_CONFIG_DIR` case turned out to be missing,
//! it was missing nineteen times and had to be fixed nineteen times.
//!
//! Resolution belongs in one place that can be tested. The commands now say
//! "run this and follow the output", which is also exactly what the Codex
//! command-player already does via `codex render` — the two harnesses now
//! share a shape instead of each carrying their own copy of the rules.

use crate::cmd::addon::claude_config_dirs;
use std::path::{Path, PathBuf};

/// Reject anything that is not a single path segment. Workflow names come
/// from a slash command (`fix`, `cook`), so `../` or an absolute path means
/// something has gone wrong, not that the caller wants a different directory.
fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Candidate paths for `<name>.md`, most specific first.
///
/// `HOANGSA_ROOT` wins when set (used by tests and by the local installer),
/// then a project-local install, then each Claude config dir.
pub fn candidates(name: &str, cwd: &str) -> Vec<PathBuf> {
    let file = format!("{name}.md");
    let mut out = Vec::new();
    if let Ok(root) = std::env::var("HOANGSA_ROOT")
        && !root.is_empty()
    {
        out.push(Path::new(&root).join("workflows").join(&file));
    }
    out.push(Path::new(cwd).join(".claude/hoangsa/workflows").join(&file));
    for base in claude_config_dirs() {
        out.push(base.join("hoangsa/workflows").join(&file));
    }
    out
}

/// `workflow show <name>` — print the first workflow file that exists.
///
/// Emits the file verbatim on stdout: the output IS the prompt the agent
/// follows, so it must not be wrapped in JSON or commentary. Exits non-zero
/// with the searched paths when nothing matches, so a broken install says
/// where it looked instead of leaving the agent to guess.
pub fn cmd_show(rest: &[&str], cwd: &str) {
    let Some(&name) = rest.first() else {
        eprintln!("usage: hoangsa-cli workflow show <name>");
        std::process::exit(2);
    };
    if !valid_name(name) {
        eprintln!("workflow: invalid name {name:?} — expected a single segment like `cook`");
        std::process::exit(2);
    }

    let tried = candidates(name, cwd);
    for path in &tried {
        if let Ok(body) = std::fs::read_to_string(path) {
            print!("{body}");
            return;
        }
    }

    eprintln!("workflow: {name}.md not found. Looked in:");
    for p in &tried {
        eprintln!("  {}", p.display());
    }
    eprintln!(
        "Run `hoangsa-cli install` to (re)install the templates, or set \
         CLAUDE_CONFIG_DIR if Claude runs under an alternate profile."
    );
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_names_that_escape_the_workflows_dir() {
        for bad in ["", "../secrets", "a/b", "/etc/passwd", "..", "x.md"] {
            assert!(!valid_name(bad), "{bad:?} must be rejected");
        }
        for ok in ["cook", "worker-rules", "task_link"] {
            assert!(valid_name(ok), "{ok:?} must be accepted");
        }
    }

    /// The alternate-profile directory must be searched. Omitting it is the
    /// bug this command was extracted to stop repeating.
    #[test]
    fn candidates_include_the_configured_profile_dir() {
        // SAFETY: single-threaded assertion window inside this test.
        unsafe { std::env::set_var("CLAUDE_CONFIG_DIR", "/tmp/hoangsa-alt-profile") };
        let got: Vec<String> = candidates("cook", "/proj")
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect();
        unsafe { std::env::remove_var("CLAUDE_CONFIG_DIR") };

        assert!(
            got.iter().any(|p| p.contains("/proj/.claude/hoangsa")),
            "project-local install must be searched first: {got:?}"
        );
        assert!(
            got.iter()
                .any(|p| p.starts_with("/tmp/hoangsa-alt-profile/hoangsa/workflows")),
            "CLAUDE_CONFIG_DIR must be searched: {got:?}"
        );
        assert!(
            got.iter().any(|p| p.contains("/.claude/hoangsa/workflows")),
            "the default profile must remain a fallback: {got:?}"
        );
    }
}
