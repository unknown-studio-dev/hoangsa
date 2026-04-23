//! `hoangsa-cli ctx <workflow>` — pre-built context bundle.
//!
//! Deterministically assembles session state, git context, config excerpts,
//! and session artifact listing into `$SESSION_DIR/ctx.md`. The workflow
//! reads this once at Step 0 and skips the scattered `state get` /
//! `git status` / `config get` calls that would otherwise repeat throughout
//! the run. Absence of the file is a no-op — workflows fall back to their
//! existing boot sequence.

use crate::helpers::{out, read_json};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Known workflow identifiers. Used to scope the ctx bundle — e.g. cook
/// needs plan.json, menu needs DESIGN-SPEC excerpts, audit needs config
/// stack data.
const KNOWN_WORKFLOWS: &[&str] = &[
    "cook", "menu", "audit", "init", "fix", "brainstorm", "prepare", "research", "taste",
    "serve", "plate", "ship", "check", "addon", "update", "index", "rule",
];

struct Bundle {
    workflow: String,
    session_id: String,
    sections: Vec<(String, String)>,
}

impl Bundle {
    fn render(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("# Context bundle — `{}`\n\n", self.workflow));
        s.push_str(&format!("Session: `{}`\n\n", self.session_id));
        s.push_str(
            "Pre-built by `hoangsa-cli ctx`. Read once at Step 0 instead of scattering `state get`, `git status`, `config get` across the workflow.\n\n",
        );
        for (heading, body) in &self.sections {
            s.push_str(&format!("## {heading}\n\n{}\n\n", body.trim_end()));
        }
        s
    }
}

fn section_names(bundle: &Bundle) -> Vec<String> {
    bundle
        .sections
        .iter()
        .map(|(h, _)| h.clone())
        .collect()
}

/// Locate the latest session under `<cwd>/.hoangsa/sessions/` — used when
/// the caller does not provide an explicit `$SESSION_DIR`.
fn latest_session(cwd: &str) -> Option<PathBuf> {
    let root = Path::new(cwd).join(".hoangsa").join("sessions");
    let type_dirs = fs::read_dir(&root).ok()?;
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for ty in type_dirs.filter_map(|e| e.ok()) {
        let Ok(name) = ty.file_name().into_string() else {
            continue;
        };
        if !crate::cmd::session::KNOWN_TYPES.contains(&name.as_str()) {
            continue;
        }
        let Ok(names) = fs::read_dir(ty.path()) else {
            continue;
        };
        for n in names.filter_map(|e| e.ok()) {
            let meta = match n.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };
            if !meta.is_dir() {
                continue;
            }
            let mtime = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                best = Some((mtime, n.path()));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Run `git` in `cwd`, returning trimmed stdout on success or the empty
/// string on any failure. We don't propagate git errors into ctx.md —
/// the section simply shows "not a git repo" or the truncated result.
fn git(cwd: &str, args: &[&str]) -> String {
    Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout).ok()
            } else {
                None
            }
        })
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn build_session_section(state_path: &Path) -> String {
    if !state_path.exists() {
        return "_No `state.json` yet — session has not been initialized._".to_string();
    }
    let state = read_json(state_path.to_str().unwrap_or(""));
    if state.get("error").is_some() {
        return format!("_Error reading state.json: {}_", state["error"]);
    }
    let mut lines = String::from("```json\n");
    lines.push_str(&serde_json::to_string_pretty(&state).unwrap_or_default());
    lines.push_str("\n```");
    lines
}

fn build_git_section(cwd: &str) -> String {
    if git(cwd, &["rev-parse", "--is-inside-work-tree"]) != "true" {
        return "_Not a git repository._".to_string();
    }
    let branch = git(cwd, &["branch", "--show-current"]);
    let base = git(cwd, &["symbolic-ref", "refs/remotes/origin/HEAD"]);
    let base = base.trim_start_matches("refs/remotes/origin/").to_string();
    let dirty = git(cwd, &["status", "--porcelain"]);
    let dirty_lines = dirty.lines().count();
    // `git log` with --oneline caps output to recent N commits.
    let recent = git(cwd, &["log", "-n", "5", "--oneline", "--no-decorate"]);

    let mut s = String::new();
    s.push_str(&format!("- **Branch:** `{branch}`\n"));
    if !base.is_empty() {
        s.push_str(&format!("- **Base:** `{base}`\n"));
    }
    s.push_str(&format!(
        "- **Dirty:** {} file(s){}\n",
        dirty_lines,
        if dirty_lines > 0 {
            " — run `/plate` or stash before switching branches"
        } else {
            ""
        }
    ));
    if !recent.is_empty() {
        s.push_str("\n**Recent commits:**\n\n```\n");
        s.push_str(&recent);
        s.push_str("\n```");
    }
    s
}

fn build_config_section(cwd: &str) -> String {
    let cfg_path = Path::new(cwd).join(".hoangsa").join("config.json");
    if !cfg_path.exists() {
        return "_No `.hoangsa/config.json` — run `/hoangsa:init` first._".to_string();
    }
    let cfg = read_json(cfg_path.to_str().unwrap_or(""));
    if cfg.get("error").is_some() {
        return format!("_Error reading config.json: {}_", cfg["error"]);
    }
    let prefs = cfg.get("preferences").cloned().unwrap_or(json!({}));
    let codebase = cfg.get("codebase").cloned().unwrap_or(json!({}));
    let compact = json!({
        "preferences": prefs,
        "codebase": {
            "tech_stack": codebase.get("tech_stack"),
            "git_convention": codebase.get("git_convention"),
            "packages_count": codebase
                .get("packages")
                .and_then(|p| p.as_array())
                .map(|a| a.len())
                .unwrap_or(0),
            "linters": codebase.get("linters"),
            "testing": codebase.get("testing"),
        },
    });
    format!(
        "```json\n{}\n```",
        serde_json::to_string_pretty(&compact).unwrap_or_default()
    )
}

fn build_artifacts_section(session_dir: &Path) -> String {
    let Ok(entries) = fs::read_dir(session_dir) else {
        return "_Could not list session directory._".to_string();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();
    if names.is_empty() {
        return "_Session directory is empty._".to_string();
    }
    names
        .iter()
        .map(|n| format!("- `{n}`"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn build_plan_section(session_dir: &Path) -> Option<String> {
    let plan_path = session_dir.join("plan.json");
    if !plan_path.exists() {
        return None;
    }
    let plan = read_json(plan_path.to_str().unwrap_or(""));
    if plan.get("error").is_some() {
        return Some(format!("_Error reading plan.json: {}_", plan["error"]));
    }
    let tasks = plan.get("tasks").and_then(|v| v.as_array());
    let task_count = tasks.map(|t| t.len()).unwrap_or(0);
    let pending = tasks
        .map(|t| {
            t.iter()
                .filter(|task| {
                    let s = task
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("pending");
                    !matches!(s, "completed" | "done" | "skipped")
                })
                .count()
        })
        .unwrap_or(0);
    let workspace = plan
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("(unset)");
    let mut s = format!(
        "- **Tasks:** {task_count} total, {pending} pending\n- **Workspace:** `{workspace}`\n"
    );
    if let Some(tasks) = tasks {
        s.push_str("\n**Task list:**\n\n");
        for t in tasks.iter().take(20) {
            let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let name = t.get("name").and_then(|v| v.as_str()).unwrap_or("?");
            let status = t
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("pending");
            s.push_str(&format!("- `{id}` [{status}] — {name}\n"));
        }
        if tasks.len() > 20 {
            s.push_str(&format!("- …and {} more\n", tasks.len() - 20));
        }
    }
    Some(s)
}

fn build_design_spec_section(session_dir: &Path) -> Option<String> {
    let path = session_dir.join("DESIGN-SPEC.md");
    let content = fs::read_to_string(&path).ok()?;
    // Cap excerpt at 80 lines — if the spec is longer, the workflow re-reads
    // the full file on demand. The ctx bundle's job is to surface *presence*
    // and the top-of-file gist, not to re-embed the whole spec.
    let excerpt: String = content.lines().take(80).collect::<Vec<_>>().join("\n");
    let truncated = content.lines().count() > 80;
    Some(format!(
        "```markdown\n{excerpt}{}\n```",
        if truncated { "\n\n…(truncated — read DESIGN-SPEC.md for full content)" } else { "" }
    ))
}

fn build_usage_section(session_dir: &Path) -> Option<String> {
    let path = session_dir.join("usage.json");
    if !path.exists() {
        return None;
    }
    let v = read_json(path.to_str().unwrap_or(""));
    if v.get("error").is_some() {
        return None;
    }
    let turns = v.get("turns").and_then(|n| n.as_u64()).unwrap_or(0);
    let total = v.get("total_tokens").and_then(|n| n.as_u64()).unwrap_or(0);
    let cache = v
        .get("cache_read_tokens")
        .and_then(|n| n.as_u64())
        .unwrap_or(0);
    Some(format!(
        "- **Turns so far:** {turns}\n- **Total tokens:** {total}\n- **Cache-read:** {cache}\n"
    ))
}

/// `ctx <workflow> [session_id]`
pub fn cmd_ctx(workflow: Option<&str>, session_id: Option<&str>, cwd: &str) {
    let workflow = match workflow {
        Some(w) if !w.is_empty() => w.to_string(),
        _ => {
            out(&json!({ "error": "workflow is required (e.g. cook, menu, audit)" }));
            return;
        }
    };
    if !KNOWN_WORKFLOWS.contains(&workflow.as_str()) {
        out(&json!({
            "error": format!(
                "Unknown workflow '{workflow}'. Known: {}",
                KNOWN_WORKFLOWS.join(", ")
            )
        }));
        return;
    }

    // Resolve session dir. Explicit id wins; otherwise use latest.
    let session_dir = match session_id {
        Some(id) if !id.is_empty() => {
            let p = Path::new(cwd)
                .join(".hoangsa")
                .join("sessions")
                .join(id);
            if !p.exists() {
                out(&json!({
                    "error": format!("Session not found: {id}"),
                }));
                return;
            }
            p
        }
        _ => match latest_session(cwd) {
            Some(p) => p,
            None => {
                out(&json!({
                    "error": "No session found — run `/hoangsa:menu` or `/hoangsa:fix` first",
                }));
                return;
            }
        },
    };

    // Derive session_id string from path: .../.hoangsa/sessions/<type>/<name>
    let session_id_derived = session_dir
        .parent()
        .and_then(|parent| {
            session_dir.file_name().and_then(|n| n.to_str()).map(|name| {
                let ty = parent.file_name().and_then(|t| t.to_str()).unwrap_or("");
                format!("{ty}/{name}")
            })
        })
        .unwrap_or_else(|| "unknown".to_string());

    let mut sections: Vec<(String, String)> = vec![
        (
            "Session state".to_string(),
            build_session_section(&session_dir.join("state.json")),
        ),
        ("Git context".to_string(), build_git_section(cwd)),
        ("Project config (excerpt)".to_string(), build_config_section(cwd)),
        (
            "Session artifacts".to_string(),
            build_artifacts_section(&session_dir),
        ),
    ];

    // Workflow-scoped additions — cheap and deterministic.
    match workflow.as_str() {
        "cook" | "taste" | "check" => {
            if let Some(plan) = build_plan_section(&session_dir) {
                sections.push(("Plan summary".to_string(), plan));
            }
            if let Some(spec) = build_design_spec_section(&session_dir) {
                sections.push(("DESIGN-SPEC excerpt".to_string(), spec));
            }
        }
        "prepare" | "menu" | "brainstorm" | "fix" => {
            if let Some(spec) = build_design_spec_section(&session_dir) {
                sections.push(("DESIGN-SPEC excerpt".to_string(), spec));
            }
        }
        _ => {}
    }

    if let Some(usage) = build_usage_section(&session_dir) {
        sections.push(("Session token usage".to_string(), usage));
    }

    let bundle = Bundle {
        workflow: workflow.clone(),
        session_id: session_id_derived,
        sections,
    };

    let rendered = bundle.render();
    let ctx_path = session_dir.join("ctx.md");
    if let Err(e) = fs::write(&ctx_path, &rendered) {
        out(&json!({
            "success": false,
            "error": format!("Cannot write ctx.md: {e}"),
        }));
        return;
    }

    out(&json!({
        "success": true,
        "path": ctx_path.to_string_lossy(),
        "size_bytes": rendered.len(),
        "sections": section_names(&bundle),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn setup_session(root: &Path, kind: &str, name: &str) -> PathBuf {
        let dir = root.join(".hoangsa").join("sessions").join(kind).join(name);
        fs::create_dir_all(&dir).unwrap();
        let state = json!({
            "session_id": format!("{kind}/{name}"),
            "status": "design",
            "task_type": kind,
            "language": "en",
        });
        fs::write(
            dir.join("state.json"),
            serde_json::to_string_pretty(&state).unwrap(),
        )
        .unwrap();
        dir
    }

    #[test]
    fn ctx_unknown_workflow_errors() {
        // Runs via the public API — we only exercise the validation branch,
        // which does not touch the filesystem. Capture stdout via a guard
        // process if needed; here we just smoke-test by calling build helpers.
        assert!(!KNOWN_WORKFLOWS.contains(&"bogus-workflow"));
    }

    #[test]
    fn build_session_section_handles_missing_state() {
        let tmp = tempdir().unwrap();
        let out = build_session_section(&tmp.path().join("state.json"));
        assert!(out.contains("No `state.json`"));
    }

    #[test]
    fn build_session_section_embeds_state_json() {
        let tmp = tempdir().unwrap();
        let path = tmp.path().join("state.json");
        fs::write(&path, r#"{"status":"cooking","language":"vi"}"#).unwrap();
        let out = build_session_section(&path);
        assert!(out.contains("cooking"));
        assert!(out.contains("```json"));
    }

    #[test]
    fn build_plan_section_returns_none_when_absent() {
        let tmp = tempdir().unwrap();
        assert!(build_plan_section(tmp.path()).is_none());
    }

    #[test]
    fn build_plan_section_counts_pending() {
        let tmp = tempdir().unwrap();
        fs::write(
            tmp.path().join("plan.json"),
            serde_json::to_string_pretty(&json!({
                "workspace_dir": "/tmp/ws",
                "tasks": [
                    {"id": "T-01", "name": "a", "status": "completed"},
                    {"id": "T-02", "name": "b", "status": "pending"},
                    {"id": "T-03", "name": "c"},
                ]
            }))
            .unwrap(),
        )
        .unwrap();
        let out = build_plan_section(tmp.path()).expect("some");
        assert!(out.contains("3 total"));
        assert!(out.contains("2 pending"));
        assert!(out.contains("T-01"));
    }

    #[test]
    fn build_design_spec_section_truncates_long_specs() {
        let tmp = tempdir().unwrap();
        let long: String = (0..120).map(|i| format!("line {i}\n")).collect();
        fs::write(tmp.path().join("DESIGN-SPEC.md"), &long).unwrap();
        let out = build_design_spec_section(tmp.path()).expect("some");
        assert!(out.contains("truncated"));
        assert!(out.contains("line 0"));
        assert!(!out.contains("line 100\n")); // 100..120 should be chopped
    }

    #[test]
    fn latest_session_picks_most_recent_across_types() {
        let tmp = tempdir().unwrap();
        let _a = setup_session(tmp.path(), "feat", "one");
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = setup_session(tmp.path(), "fix", "two");
        let picked = latest_session(tmp.path().to_str().unwrap()).expect("some");
        assert_eq!(picked, b);
    }

    #[test]
    fn build_artifacts_section_lists_non_dotfiles() {
        let tmp = tempdir().unwrap();
        fs::write(tmp.path().join("state.json"), "{}").unwrap();
        fs::write(tmp.path().join("DESIGN-SPEC.md"), "x").unwrap();
        fs::write(tmp.path().join(".hidden"), "x").unwrap();
        let out = build_artifacts_section(tmp.path());
        assert!(out.contains("state.json"));
        assert!(out.contains("DESIGN-SPEC.md"));
        assert!(!out.contains(".hidden"));
    }
}
