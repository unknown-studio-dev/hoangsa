use crate::cmd::addon::resolve_hoangsa_root;
use crate::helpers::{out, parse_frontmatter, read_file, read_json};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

/// One addon loaded with metadata + body, ready for composition.
struct Addon {
    name: String,
    frameworks: Vec<String>,
    test_frameworks: Vec<String>,
    priority: i64,
    inject_position: String,
    allowed_tools: Vec<String>,
    pre_invoke_gate: Option<String>,
    exclude_task_types: Vec<String>,
    include_task_types: Vec<String>,
    exclude_worker_roles: Vec<String>,
    include_worker_roles: Vec<String>,
    body: String,
}

fn fm_list(fm: &std::collections::BTreeMap<String, String>, key: &str) -> Vec<String> {
    fm.get(key)
        .and_then(|f| serde_json::from_str::<Vec<String>>(f).ok())
        .unwrap_or_default()
}

/// Content after the closing `---` of YAML frontmatter (or the whole file).
fn strip_frontmatter(content: &str) -> String {
    let mut lines = content.lines();
    if lines.next().map(str::trim) != Some("---") {
        return content.to_string();
    }
    let mut rest = String::new();
    let mut in_fm = true;
    for line in lines {
        if in_fm {
            if line.trim() == "---" {
                in_fm = false;
            }
            continue;
        }
        rest.push_str(line);
        rest.push('\n');
    }
    rest.trim().to_string()
}

/// Load every addon under `dir` (non-recursive `*.md`), parsing frontmatter.
fn load_addons(dir: &Path) -> Vec<Addon> {
    let mut result = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Some(fm) = parse_frontmatter(&content) else {
            continue;
        };
        let Some(name) = fm.get("name").cloned() else {
            continue;
        };
        result.push(Addon {
            name,
            frameworks: fm_list(&fm, "frameworks"),
            test_frameworks: fm_list(&fm, "test_frameworks"),
            priority: fm.get("priority").and_then(|p| p.parse().ok()).unwrap_or(50),
            inject_position: fm
                .get("inject_position")
                .cloned()
                .unwrap_or_else(|| "after_base".to_string()),
            allowed_tools: fm_list(&fm, "allowed_tools"),
            pre_invoke_gate: fm.get("pre_invoke_gate").filter(|v| *v != "null").cloned(),
            exclude_task_types: fm_list(&fm, "exclude_task_types"),
            include_task_types: fm_list(&fm, "include_task_types"),
            exclude_worker_roles: fm_list(&fm, "exclude_worker_roles"),
            include_worker_roles: fm_list(&fm, "include_worker_roles"),
            body: strip_frontmatter(&content),
        });
    }
    result
}

/// All stack/framework terms declared in config.json, lowercased:
/// preferences.tech_stack, codebase.frameworks, codebase.packages[].stack,
/// codebase.packages[].frameworks, codebase.testing.frameworks, active_addons.
fn project_terms(config: &Value) -> BTreeSet<String> {
    fn add_arr(terms: &mut BTreeSet<String>, v: Option<&Value>) {
        if let Some(arr) = v.and_then(|x| x.as_array()) {
            for s in arr.iter().filter_map(|s| s.as_str()) {
                terms.insert(s.to_lowercase());
            }
        }
    }
    let mut terms = BTreeSet::new();
    add_arr(&mut terms, config.pointer("/preferences/tech_stack"));
    add_arr(&mut terms, config.pointer("/codebase/frameworks"));
    add_arr(&mut terms, config.pointer("/codebase/testing/frameworks"));
    add_arr(&mut terms, config.pointer("/codebase/active_addons"));
    if let Some(pkgs) = config.pointer("/codebase/packages").and_then(|p| p.as_array()) {
        for pkg in pkgs {
            if let Some(s) = pkg.get("stack").and_then(|s| s.as_str()) {
                terms.insert(s.to_lowercase());
            }
            add_arr(&mut terms, pkg.get("frameworks"));
        }
    }
    terms
}

struct Composed {
    rules: String,
    allowed_tools: Vec<String>,
    applied: Vec<String>,
    skipped: Vec<(String, String)>,
}

/// Middleware-chain composition (cook.md §Worker rules — Middleware Chain):
/// before_base addons → base → after_base addons → project overrides → tail
/// addons, each group sorted by priority then name; task-type / worker-role /
/// pre-invoke gates applied per addon; allowed_tools = union.
fn compose_rules(project_dir: &str, task_type: &str, role: &str) -> Result<Composed, String> {
    let root = resolve_hoangsa_root(project_dir)
        .ok_or_else(|| "HOANGSA_ROOT not found (checked env, .claude/hoangsa, ~/.claude/hoangsa)".to_string())?;
    compose_rules_at(&root, project_dir, task_type, role)
}

fn compose_rules_at(
    root: &str,
    project_dir: &str,
    task_type: &str,
    role: &str,
) -> Result<Composed, String> {
    let base = read_file(
        Path::new(&root)
            .join("workflows/worker-rules/base.md")
            .to_str()
            .unwrap_or(""),
    )
    .map(|c| strip_frontmatter(&c))
    .ok_or_else(|| "worker-rules/base.md not found".to_string())?;

    // Project-level addon files override root addons of the same name
    let mut addons = load_addons(&Path::new(root).join("workflows/worker-rules/addons"));
    let overrides = load_addons(&Path::new(project_dir).join(".hoangsa/worker-rules/addons"));
    for ov in overrides {
        addons.retain(|a| a.name != ov.name);
        addons.push(ov);
    }

    let config = read_json(
        Path::new(project_dir)
            .join(".hoangsa/config.json")
            .to_str()
            .unwrap_or(""),
    );
    let terms = project_terms(&config);

    let mut applied: Vec<&Addon> = Vec::new();
    let mut skipped: Vec<(String, String)> = Vec::new();
    for addon in &addons {
        let stack_match = addon.frameworks.iter().any(|f| f == "*" || terms.contains(&f.to_lowercase()))
            || addon.test_frameworks.iter().any(|f| terms.contains(&f.to_lowercase()))
            || terms.contains(&addon.name.to_lowercase());
        if !stack_match {
            skipped.push((addon.name.clone(), "no stack match".to_string()));
            continue;
        }
        if addon.exclude_task_types.iter().any(|t| t == task_type) {
            skipped.push((addon.name.clone(), format!("excluded for task type {task_type}")));
            continue;
        }
        if !addon.include_task_types.is_empty()
            && !addon.include_task_types.iter().any(|t| t == task_type)
        {
            skipped.push((addon.name.clone(), format!("not included for task type {task_type}")));
            continue;
        }
        if addon.exclude_worker_roles.iter().any(|r| r == role) {
            skipped.push((addon.name.clone(), format!("excluded for role {role}")));
            continue;
        }
        if !addon.include_worker_roles.is_empty()
            && !addon.include_worker_roles.iter().any(|r| r == role)
        {
            skipped.push((addon.name.clone(), format!("not included for role {role}")));
            continue;
        }
        if let Some(gate) = &addon.pre_invoke_gate {
            let ok = Command::new("sh")
                .arg("-c")
                .arg(gate)
                .current_dir(project_dir)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            if !ok {
                skipped.push((addon.name.clone(), "pre_invoke_gate failed".to_string()));
                continue;
            }
        }
        applied.push(addon);
    }

    let group = |pos: &str| -> Vec<&Addon> {
        let mut g: Vec<&Addon> = applied
            .iter()
            .copied()
            .filter(|a| a.inject_position == pos)
            .collect();
        g.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.name.cmp(&b.name)));
        g
    };

    let project_overrides = read_file(
        Path::new(project_dir)
            .join(".hoangsa/worker-rules.md")
            .to_str()
            .unwrap_or(""),
    )
    .map(|c| strip_frontmatter(&c));

    let (before, after, tail) = (group("before_base"), group("after_base"), group("tail"));
    let applied_names: Vec<String> = before
        .iter()
        .chain(after.iter())
        .chain(tail.iter())
        .map(|a| a.name.clone())
        .collect();

    let mut parts: Vec<String> = Vec::new();
    for a in before {
        parts.push(a.body.clone());
    }
    parts.push(base);
    for a in after {
        parts.push(a.body.clone());
    }
    if let Some(po) = project_overrides {
        parts.push(po);
    }
    for a in tail {
        parts.push(a.body.clone());
    }

    let mut allowed: BTreeSet<String> = BTreeSet::new();
    for a in &applied {
        for t in &a.allowed_tools {
            allowed.insert(t.clone());
        }
    }

    Ok(Composed {
        rules: parts.join("\n\n---\n\n"),
        allowed_tools: allowed.into_iter().collect(),
        applied: applied_names,
        skipped,
    })
}

/// `rules compose <projectDir> [--task-type t] [--role r]`
pub fn cmd_compose(project_dir: &str, task_type: &str, role: &str) {
    match compose_rules(project_dir, task_type, role) {
        Ok(c) => out(&json!({
            "rules": c.rules,
            "allowed_tools": c.allowed_tools,
            "applied": c.applied,
            "skipped": c.skipped.iter().map(|(n, r)| json!({"name": n, "reason": r})).collect::<Vec<_>>(),
        })),
        Err(e) => out(&json!({ "error": e })),
    }
}

/// Lesson paragraphs from `.hoangsa/memory/LESSONS.md` whose text contains any
/// keyword derived from the task (name words >3 chars, file stems, task type).
/// Format-agnostic: entries are blank-line-separated paragraphs. Max 5.
fn matching_lessons(workspace: &str, task: &Value) -> Vec<String> {
    let Some(content) = read_file(
        Path::new(workspace)
            .join(".hoangsa/memory/LESSONS.md")
            .to_str()
            .unwrap_or(""),
    ) else {
        return Vec::new();
    };

    let mut keywords: BTreeSet<String> = BTreeSet::new();
    if let Some(name) = task.get("name").and_then(|v| v.as_str()) {
        for w in name.split(|c: char| !c.is_alphanumeric()) {
            if w.len() > 3 {
                keywords.insert(w.to_lowercase());
            }
        }
    }
    if let Some(t) = task.get("type").and_then(|v| v.as_str()) {
        keywords.insert(t.to_lowercase());
    }
    if let Some(files) = task.get("files").and_then(|v| v.as_array()) {
        for f in files.iter().filter_map(|f| f.as_str()) {
            if let Some(stem) = Path::new(f).file_stem().and_then(|s| s.to_str())
                && stem.len() > 3
            {
                keywords.insert(stem.to_lowercase());
            }
        }
    }
    if keywords.is_empty() {
        return Vec::new();
    }

    content
        .split("\n\n")
        .filter(|p| {
            let lower = p.to_lowercase();
            !p.trim().is_empty()
                && !p.trim_start().starts_with('#')
                && keywords.iter().any(|k| lower.contains(k.as_str()))
        })
        .take(5)
        .map(|p| p.trim().to_string())
        .collect()
}

/// The skill registry block from common.md §Worker skill registry (first fenced
/// block in that section). Falls back to a minimal built-in list.
fn skill_registry(root: &str) -> String {
    let fallback = "Available skills — read the full SKILL.md only if relevant to your task:\n\
        - git-flow: Git branching, task switching, PR creation → .claude/skills/hoangsa/git-flow/SKILL.md\n\
        - visual-debug: Screenshot/video analysis for visual bugs → .claude/skills/hoangsa/visual-debug/SKILL.md\n\
        - fe-testing: FE verification loop — criteria, test layers, run-and-observe, mutation check → .claude/skills/hoangsa/fe-testing/SKILL.md\n\n\
        To use a skill: read_file(\"<path>\") to get full instructions, then follow them.\n\
        Do NOT read skills unless your task specifically requires them.";
    let Some(common) = read_file(
        Path::new(root)
            .join("workflows/common.md")
            .to_str()
            .unwrap_or(""),
    ) else {
        return fallback.to_string();
    };
    let mut in_section = false;
    let mut in_fence = false;
    let mut block = String::new();
    for line in common.lines() {
        if let Some(h) = line.strip_prefix("## ") {
            if in_section {
                break;
            }
            in_section = h.trim().eq_ignore_ascii_case("Worker skill registry");
            continue;
        }
        if !in_section {
            continue;
        }
        if line.trim_start().starts_with("```") {
            if in_fence {
                break;
            }
            in_fence = true;
            continue;
        }
        if in_fence {
            block.push_str(line);
            block.push('\n');
        }
    }
    let block = block.trim();
    if block.is_empty() { fallback.to_string() } else { block.to_string() }
}

/// Map a plan task type onto a model-routing role (`resolve-model` roles).
/// Research-flavored tasks read + summarize; everything else writes code.
fn model_role_for(task_type: &str) -> &'static str {
    match task_type {
        "research" | "analysis" => "researcher",
        _ => "worker",
    }
}

fn list_section(title: &str, items: Option<&Vec<Value>>, render: impl Fn(&Value) -> String) -> String {
    let Some(items) = items.filter(|a| !a.is_empty()) else {
        return String::new();
    };
    let lines: Vec<String> = items.iter().map(render).collect();
    format!("\n{title}\n{}\n", lines.join("\n"))
}

/// `envelope <sessionDir> <taskId> [--kind cook|fix] [--memory-status s]`
/// Prints the complete worker prompt (markdown) to stdout.
pub fn cmd_envelope(session_dir: &str, task_id: &str, kind: &str, memory_status: &str) {
    let plan_path = Path::new(session_dir).join("plan.json");
    let plan = read_json(plan_path.to_str().unwrap_or(""));
    if plan.get("error").is_some() {
        out(&json!({ "error": format!("plan.json not found or invalid in {session_dir}") }));
        return;
    }
    let empty = Vec::new();
    let Some(task) = plan
        .get("tasks")
        .and_then(|t| t.as_array())
        .unwrap_or(&empty)
        .iter()
        .find(|t| t.get("id").and_then(|i| i.as_str()) == Some(task_id))
    else {
        out(&json!({ "error": format!("task {task_id} not found in plan.json") }));
        return;
    };

    let workspace = plan
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let task_type = task
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| if kind == "fix" { "fix" } else { "impl" });
    let role = match task_type {
        "research" | "analysis" => "readonly",
        _ => "impl",
    };
    let (model, _profile, model_source) =
        crate::cmd::model::resolve_model_parts(model_role_for(task_type), workspace);

    let composed = match compose_rules(workspace, task_type, role) {
        Ok(c) => c,
        Err(e) => {
            out(&json!({ "error": e }));
            return;
        }
    };
    let root = resolve_hoangsa_root(workspace).unwrap_or_default();

    let s = |key: &str| task.get(key).and_then(|v| v.as_str()).unwrap_or("");
    let name = s("name");
    let acceptance = s("acceptance");
    let is_ui = task.get("ui").and_then(|v| v.as_bool()).unwrap_or(false);
    let evidence_dir = format!("{session_dir}/evidence/{task_id}");
    if is_ui {
        let _ = fs::create_dir_all(&evidence_dir);
    }

    let files = list_section("Files to modify:", task.get("files").and_then(|v| v.as_array()), |f| {
        format!("- {}", f.as_str().unwrap_or("?"))
    });
    let pointers = list_section(
        "Context to read first:",
        task.get("context_pointers").and_then(|v| v.as_array()),
        |f| format!("- {}", f.as_str().unwrap_or("?")),
    );
    let covers = list_section("Requirements covered:", task.get("covers").and_then(|v| v.as_array()), |f| {
        format!("- {}", f.as_str().unwrap_or("?"))
    });
    let test_cases = list_section(
        "Test cases to satisfy (from TEST-SPEC):",
        task.get("test_cases").and_then(|v| v.as_array()),
        |t| {
            format!(
                "- {} — expected: {} — verify: `{}`",
                t.get("name").and_then(|v| v.as_str()).unwrap_or("?"),
                t.get("expected").and_then(|v| v.as_str()).unwrap_or("?"),
                t.get("verify").and_then(|v| v.as_str()).unwrap_or("?"),
            )
        },
    );
    let edge_cases = list_section(
        "Edge cases you MUST handle — non-negotiable:",
        task.get("edge_cases").and_then(|v| v.as_array()),
        |e| {
            format!(
                "- {} — input: {} — expected: {}",
                e.get("case").and_then(|v| v.as_str()).unwrap_or("?"),
                e.get("input").and_then(|v| v.as_str()).unwrap_or("?"),
                e.get("expected").and_then(|v| v.as_str()).unwrap_or("?"),
            )
        },
    );

    let lessons = matching_lessons(workspace, task);
    let lessons_section = if lessons.is_empty() {
        String::new()
    } else {
        format!(
            "\nLessons from past sessions in this area — follow them or report why one doesn't apply:\n{}\n",
            lessons.join("\n\n")
        )
    };

    let context_pack = read_file(
        Path::new(session_dir)
            .join(format!("task-{task_id}.context.json"))
            .to_str()
            .unwrap_or(""),
    )
    .map(|c| format!("\n## Context Pack\n\n{c}\n"))
    .unwrap_or_default();

    let bug_context = if kind == "fix" {
        "\nBug context:\n<BUG_CONTEXT — orchestrator replaces this line with the root-cause summary and cross-layer notes>\n"
    } else {
        ""
    };

    let ui_line = if is_ui { "yes" } else { "no" };
    let ui_step = if kind == "fix" {
        "5b. If \"UI task: yes\": read the fe-testing skill, follow flow 5 (run-and-observe) — re-render every state taste flagged plus the standard Visual Verification states, screenshot each into the evidence dir, list the paths in your report. A UI fix that was never rendered is NOT fixed"
    } else {
        "5b. If \"UI task: yes\": read the fe-testing skill, follow flow 5 (run-and-observe) — walk every state in the TEST-SPEC Visual Verification table, screenshot each into your evidence dir, list the paths in your report. UI that compiles but was never rendered is NOT done"
    };
    // Conventional-commit prefix: task.type is a worker role, not a commit type
    let commit_type = match task_type {
        "impl" => plan
            .get("name")
            .and_then(|n| n.as_str())
            .and_then(|n| n.split(':').next())
            .filter(|p| !p.trim().is_empty() && !p.contains(' '))
            .unwrap_or("feat")
            .to_string(),
        "test" | "e2e" => "test".to_string(),
        other => other.to_string(),
    };
    let commit_step = if kind == "fix" {
        format!("7. Commit with message: \"fix(<scope>): {name}\" — <scope> = primary module/package from the files above, NOT session_id/branch name\n8. After committing: memory_detect_changes({{diff: \"<git diff of your commit>\"}}) — confirm only expected symbols changed; a fix must be minimal")
    } else {
        format!("7. Commit with message: \"{commit_type}(<scope>): {name}\" — <scope> = primary module/package from the files above, NOT session_id/branch name\n8. After committing: memory_turn_save({{role: \"assistant\", text: \"Task {task_id}: <one-line summary>\"}}) and memory_detect_changes({{diff: \"<git diff of this task's commit>\"}}) — report unexpected symbol changes to the orchestrator")
    };

    let tool_restrictions = if composed.allowed_tools.is_empty() {
        String::new()
    } else {
        format!(
            "\n## Tool Restrictions\n\nYou are ONLY allowed to use these tools: {}\nDo NOT use any tool not on this list. If you need a restricted tool, report it as a blocker.\n",
            composed.allowed_tools.join(", ")
        )
    };

    let prompt = format!(
        "MODEL: {model}   (config routing, source: {model_source} — spawn this worker with exactly this model)\n\n\
You are a HOANGSA worker. Execute this task precisely.\n\n\
## Worker Rules\n\n{rules}\n{tool_restrictions}\n---\n\n\
## Task Envelope\n\n\
Task: {name}\nID: {task_id}\nWorkspace: {workspace}\nhoangsa-memory: {memory_status}\n\
{files}{pointers}{covers}{test_cases}{edge_cases}{lessons_section}{bug_context}\
\nUI task: {ui_line}\nEvidence dir (UI tasks — screenshots go here):\n{evidence_dir}/\n\
{context_pack}\
\n## Skill Registry (load on demand)\n\n{registry}\n\n\
## Instructions\n\n\
1. Read all context_pointers files first\n\
2. Before modifying any function/class/method, run memory_impact({{target: \"symbolName\", direction: \"upstream\"}}) to check blast radius (if hoangsa-memory is available); search past work with memory_archive_search({{query: \"{name} <primary module>\"}})\n\
3. If impact returns HIGH or CRITICAL risk — report it, do not proceed without orchestrator acknowledgment\n\
4. Implement the task — handling EVERY listed edge case. If an edge case is impossible or out of scope, STOP and report it as a blocker; never silently skip one\n\
5. Run the acceptance command to verify: {acceptance}\n\
{ui_step}\n\
6. If acceptance fails, fix and retry (max 3 attempts)\n\
{commit_step}\n\n\
Acceptance command: {acceptance}\n",
        rules = composed.rules,
        registry = skill_registry(&root),
    );

    println!("{prompt}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hoangsa-envelope-test-{suffix}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("root/workflows/worker-rules/addons")).unwrap();
        fs::create_dir_all(dir.join("project/.hoangsa")).unwrap();
        fs::write(
            dir.join("root/workflows/worker-rules/base.md"),
            "# Base Rules\n\n- Only modify task files.\n",
        )
        .unwrap();
        fs::write(
            dir.join("project/.hoangsa/config.json"),
            r#"{"preferences":{"tech_stack":["rust"]},"codebase":{}}"#,
        )
        .unwrap();
        dir
    }

    fn write_addon(dir: &Path, name: &str, frontmatter_extra: &str, body: &str) {
        fs::write(
            dir.join(format!("root/workflows/worker-rules/addons/{name}.md")),
            format!(
                "---\nname: {name}\nframeworks: [\"rust\"]\ntest_frameworks: []\npriority: 50\ninject_position: after_base\nallowed_tools: []\npre_invoke_gate: null\n{frontmatter_extra}---\n\n{body}\n"
            ),
        )
        .unwrap();
    }

    fn compose_in(dir: &Path, task_type: &str, role: &str) -> Composed {
        compose_rules_at(
            dir.join("root").to_str().unwrap(),
            dir.join("project").to_str().unwrap(),
            task_type,
            role,
        )
        .unwrap()
    }

    #[test]
    fn compose_includes_base_and_matching_addon_in_order() {
        let dir = temp_root("order");
        write_addon(&dir, "rust", "", "# Rust Addon");
        write_addon(&dir, "aaa-first", "", "# AAA Addon");
        let c = compose_in(&dir, "impl", "impl");
        let rules = &c.rules;
        let base_pos = rules.find("# Base Rules").unwrap();
        let aaa = rules.find("# AAA Addon").unwrap();
        let rust = rules.find("# Rust Addon").unwrap();
        assert!(base_pos < aaa && aaa < rust, "order wrong: {rules}");
        assert_eq!(c.applied, vec!["aaa-first", "rust"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_respects_inject_position_and_priority() {
        let dir = temp_root("position");
        write_addon(&dir, "early", "", "# Early");
        fs::write(
            dir.join("root/workflows/worker-rules/addons/early.md"),
            "---\nname: early\nframeworks: [\"*\"]\npriority: 10\ninject_position: before_base\n---\n\n# Early\n",
        )
        .unwrap();
        write_addon(&dir, "guard", "", "# Guard");
        fs::write(
            dir.join("root/workflows/worker-rules/addons/guard.md"),
            "---\nname: guard\nframeworks: [\"*\"]\npriority: 90\ninject_position: tail\n---\n\n# Guard\n",
        )
        .unwrap();
        let c = compose_in(&dir, "impl", "impl");
        let early = c.rules.find("# Early").unwrap();
        let base = c.rules.find("# Base Rules").unwrap();
        let guard = c.rules.find("# Guard").unwrap();
        assert!(early < base && base < guard, "{}", c.rules);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_applies_task_type_and_role_gates() {
        let dir = temp_root("gates");
        write_addon(
            &dir,
            "quality",
            "exclude_task_types: [\"research\"]\nexclude_worker_roles: [\"reviewer\"]\n",
            "# Quality",
        );
        let c = compose_in(&dir, "research", "impl");
        assert!(c.applied.is_empty(), "{:?}", c.applied);
        assert!(c.skipped.iter().any(|(n, r)| n == "quality" && r.contains("research")));

        let c = compose_in(&dir, "impl", "reviewer");
        assert!(c.applied.is_empty());

        let c = compose_in(&dir, "impl", "impl");
        assert_eq!(c.applied, vec!["quality"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_runs_pre_invoke_gate() {
        let dir = temp_root("gate-cmd");
        fs::write(
            dir.join("root/workflows/worker-rules/addons/gated.md"),
            "---\nname: gated\nframeworks: [\"*\"]\npre_invoke_gate: \"false\"\n---\n\n# Gated\n",
        )
        .unwrap();
        let c = compose_in(&dir, "impl", "impl");
        assert!(c.skipped.iter().any(|(n, r)| n == "gated" && r.contains("pre_invoke_gate")));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_skips_non_matching_stack_and_unions_allowed_tools() {
        let dir = temp_root("stack");
        fs::write(
            dir.join("root/workflows/worker-rules/addons/python.md"),
            "---\nname: python\nframeworks: [\"python\"]\n---\n\n# Python\n",
        )
        .unwrap();
        fs::write(
            dir.join("root/workflows/worker-rules/addons/locked.md"),
            "---\nname: locked\nframeworks: [\"rust\"]\nallowed_tools: [\"Read\", \"Bash\"]\n---\n\n# Locked\n",
        )
        .unwrap();
        let c = compose_in(&dir, "impl", "impl");
        assert!(c.skipped.iter().any(|(n, _)| n == "python"));
        assert_eq!(c.applied, vec!["locked"]);
        assert_eq!(c.allowed_tools, vec!["Bash", "Read"]);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compose_inserts_project_overrides_before_tail() {
        let dir = temp_root("overrides");
        fs::write(
            dir.join("project/.hoangsa/worker-rules.md"),
            "# Project Overrides\n",
        )
        .unwrap();
        fs::write(
            dir.join("root/workflows/worker-rules/addons/guard.md"),
            "---\nname: guard\nframeworks: [\"*\"]\ninject_position: tail\n---\n\n# Guard\n",
        )
        .unwrap();
        let c = compose_in(&dir, "impl", "impl");
        let po = c.rules.find("# Project Overrides").unwrap();
        let guard = c.rules.find("# Guard").unwrap();
        assert!(c.rules.find("# Base Rules").unwrap() < po && po < guard);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn strip_frontmatter_handles_both_shapes() {
        assert_eq!(strip_frontmatter("---\na: b\n---\n\nBody"), "Body");
        assert_eq!(strip_frontmatter("No FM"), "No FM");
    }

    #[test]
    fn model_role_maps_research_to_researcher_else_worker() {
        assert_eq!(model_role_for("research"), "researcher");
        assert_eq!(model_role_for("analysis"), "researcher");
        for t in ["impl", "fix", "test", "e2e"] {
            assert_eq!(model_role_for(t), "worker");
        }
    }

    #[test]
    fn lessons_match_by_task_keywords() {
        let dir = temp_root("lessons");
        fs::create_dir_all(dir.join("project/.hoangsa/memory")).unwrap();
        fs::write(
            dir.join("project/.hoangsa/memory/LESSONS.md"),
            "# Lessons\n\nL01 | when editing migrations | run sqlx prepare after changing SQL\n\nL02 | when touching validation | empty strings must 422\n",
        )
        .unwrap();
        let task = json!({
            "name": "Implement validation middleware",
            "type": "impl",
            "files": ["/x/src/validation.rs"],
        });
        let found = matching_lessons(dir.join("project").to_str().unwrap(), &task);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("validation"));
        let _ = fs::remove_dir_all(&dir);
    }
}
