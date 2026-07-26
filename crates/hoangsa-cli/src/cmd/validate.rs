use crate::cmd::dag::{detect_cycles, detect_dangling};
use crate::helpers::{is_absolute, out, parse_frontmatter, read_file, read_json};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// `validate plan <path> [--tests <TEST-SPEC path>]`
pub fn cmd_plan(file_path: &str, tests_path: Option<&str>) {
    if !Path::new(file_path).exists() {
        out(&json!({ "valid": false, "errors": [format!("Plan file not found: {}", file_path)] }));
        return;
    }
    let plan = read_json(file_path);
    if plan.get("error").is_some() {
        out(&json!({ "valid": false, "errors": [plan["error"]] }));
        return;
    }

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for f in &["name", "workspace_dir", "budget_tokens", "tasks"] {
        if plan.get(f).is_none() {
            errors.push(format!("Missing field: {f}"));
        }
    }

    let tasks = plan.get("tasks").and_then(|v| v.as_array());
    match tasks {
        Some(arr) if arr.is_empty() => {
            errors.push("tasks must be a non-empty array".to_string());
        }
        None => {
            if plan.get("tasks").is_some() {
                errors.push("tasks must be a non-empty array".to_string());
            }
        }
        _ => {}
    }

    if let Some(wd) = plan.get("workspace_dir").and_then(|v| v.as_str())
        && !is_absolute(wd) {
            errors.push("workspace_dir must be an absolute path".to_string());
        }

    if let Some(task_arr) = tasks {
        for t in task_arr {
            let tid = t.get("id").and_then(|v| v.as_str()).unwrap_or("?");
            let required = [
                "id",
                "name",
                "complexity",
                "budget_tokens",
                "files",
                "depends_on",
                "context_pointers",
                "acceptance",
            ];
            for f in &required {
                if t.get(f).is_none() {
                    errors.push(format!("Task {tid}: missing {f}"));
                }
            }
            if let Some(complexity) = t.get("complexity").and_then(|v| v.as_str())
                && !["low", "medium", "high"].contains(&complexity) {
                    errors.push(format!("Task {tid}: complexity must be low|medium|high"));
                }
            // Two tiers: 45k is the planner's split target (prepare gate 3,
            // `budget.rs::work_tokens_max`); 80k is the hard ceiling. Warning
            // only above 80k left the 45k–80k band silent, so a task the
            // planner should have split passed `validate plan` clean.
            if let Some(budget) = t.get("budget_tokens").and_then(|v| v.as_u64()) {
                if budget > 80000 {
                    warnings.push(format!("Task {tid}: budget {budget} exceeds 80k limit"));
                } else if budget > 45000 {
                    warnings.push(format!(
                        "Task {tid}: budget {budget} exceeds 45k target — split the task (prepare gate 3)"
                    ));
                }
            }
            match t.get("files").and_then(|v| v.as_array()) {
                Some(files) if files.is_empty() => {
                    errors.push(format!("Task {tid}: files must be non-empty array"));
                }
                Some(files) => {
                    for f in files {
                        if let Some(fp) = f.as_str()
                            && !is_absolute(fp) {
                                errors
                                    .push(format!("Task {tid}: file path not absolute: {fp}"));
                            }
                    }
                }
                None => {
                    errors.push(format!("Task {tid}: files must be non-empty array"));
                }
            }
            if let Some(pointers) = t.get("context_pointers").and_then(|v| v.as_array()) {
                for p in pointers {
                    if let Some(ps) = p.as_str() {
                        // Expected format: /absolute/path/file:L1-L2
                        if !ps.is_empty() && !is_absolute(ps.split(':').next().unwrap_or("")) {
                            warnings.push(format!(
                                "Task {tid}: context_pointer not absolute: {ps}"
                            ));
                        }
                    }
                }
            }
            if let Some(acceptance) = t.get("acceptance").and_then(|v| v.as_str()) {
                let trimmed = acceptance.trim();
                if !trimmed.is_empty()
                    && let Some(first_char) = trimmed.chars().next()
                        && !first_char.is_ascii_lowercase() {
                            warnings.push(format!(
                                "Task {tid}: acceptance may not be a runnable command"
                            ));
                        }
            }
            check_task_spec_fields(t, tid, &mut errors, &mut warnings);
        }
    }

    // DAG checks
    if let Some(task_arr) = tasks {
        let cycles = detect_cycles(task_arr);
        let dangling = detect_dangling(task_arr);
        for c in cycles {
            errors.push(format!("Cycle: {c}"));
        }
        errors.extend(dangling);
    }

    // Budget sanity
    if let (Some(task_arr), Some(total_budget)) =
        (tasks, plan.get("budget_tokens").and_then(|v| v.as_f64()))
        && total_budget > 0.0 {
            let sum: f64 = task_arr
                .iter()
                .filter_map(|t| t.get("budget_tokens").and_then(|v| v.as_f64()))
                .sum();
            if ((sum - total_budget) / total_budget).abs() > 0.1 {
                warnings.push(format!(
                    "Budget mismatch: declared {}, tasks sum to {}",
                    total_budget as u64, sum as u64
                ));
            }
        }

    // Cross-check embeddings against the TEST-SPEC when --tests is given
    if let Some(tp) = tests_path {
        match read_file(tp) {
            Some(spec) => {
                let (e, w) = cross_check_plan_tests(&plan, &spec);
                errors.extend(e);
                warnings.extend(w);
            }
            None => errors.push(format!("TEST-SPEC file not found: {tp}")),
        }
    }

    let task_count = tasks.map(|a| a.len()).unwrap_or(0);
    out(&json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "task_count": task_count,
    }));
}

/// Shape-check the spec-embedding fields on a task: `test_cases` entries need
/// non-empty name/expected/verify, `edge_cases` entries need non-empty
/// case/input/expected, `ui` must be a boolean, and `type` should exist
/// (addon gates and the --tests cross-check rely on it).
fn check_task_spec_fields(
    t: &Value,
    tid: &str,
    errors: &mut Vec<String>,
    warnings: &mut Vec<String>,
) {
    let specs: [(&str, &[&str]); 2] = [
        ("test_cases", &["name", "expected", "verify"]),
        ("edge_cases", &["case", "input", "expected"]),
    ];
    for (field, keys) in specs {
        match t.get(field) {
            None => warnings.push(format!(
                "Task {tid}: missing {field} — use an empty array only when there is genuinely nothing to verify"
            )),
            Some(v) => match v.as_array() {
                None => errors.push(format!("Task {tid}: {field} must be an array")),
                Some(arr) => {
                    for (i, entry) in arr.iter().enumerate() {
                        for k in keys {
                            let ok = entry
                                .get(k)
                                .and_then(|x| x.as_str())
                                .is_some_and(|s| !s.trim().is_empty());
                            if !ok {
                                errors.push(format!(
                                    "Task {tid}: {field}[{i}] missing non-empty \"{k}\""
                                ));
                            }
                        }
                    }
                }
            },
        }
    }
    // Behavior contract: an impl worker handed a signature and a test name but no
    // logic invents the logic. `behavior` carries the DESIGN-SPEC steps down to it.
    let kind = t.get("type").and_then(|v| v.as_str());
    const BEHAVIOR_HINT: &str = "copy the DESIGN-SPEC ## Behavior / Logic steps for the REQs \
        this task covers, or state an explicit waiver entry (\"N/A — <reason>\")";
    match t.get("behavior") {
        Some(v) => match v.as_array() {
            None => errors.push(format!("Task {tid}: behavior must be an array of steps")),
            Some(arr) => {
                for (i, step) in arr.iter().enumerate() {
                    if step.as_str().is_none_or(|s| s.trim().is_empty()) {
                        errors.push(format!("Task {tid}: behavior[{i}] must be a non-empty string"));
                    }
                }
                if arr.is_empty() && kind == Some("impl") {
                    errors.push(format!("Task {tid}: behavior is empty — {BEHAVIOR_HINT}"));
                }
            }
        },
        None => match kind {
            Some("impl") => errors.push(format!("Task {tid}: missing behavior — {BEHAVIOR_HINT}")),
            None => warnings.push(format!(
                "Task {tid}: missing behavior (and no type to tell whether it implements anything) — {BEHAVIOR_HINT}"
            )),
            _ => {}
        },
    }

    if let Some(ui) = t.get("ui")
        && !ui.is_boolean() {
            errors.push(format!("Task {tid}: ui must be a boolean"));
        }
    if t.get("type").is_none() {
        warnings.push(format!(
            "Task {tid}: missing type (impl|test|e2e|research|analysis) — addon gates and --tests cross-check rely on it"
        ));
    }
}

/// Cross-check plan.json embeddings against the TEST-SPEC that produced them:
/// every non-waiver Edge Cases row must be carried by ≥1 implementation task
/// AND ≥1 test/e2e task, every spec test must appear in some task's
/// test_cases, ## E2E Tests requires an e2e task, and `surface: ui` requires
/// at least one task flagged `"ui": true`.
fn cross_check_plan_tests(plan: &Value, spec: &str) -> (Vec<String>, Vec<String>) {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    struct TaskInfo {
        id: String,
        name: String,
        kind: Option<String>,
        edge: Vec<String>,
        tests: Vec<String>,
        ui: bool,
    }

    let empty = Vec::new();
    let infos: Vec<TaskInfo> = plan
        .get("tasks")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty)
        .iter()
        .map(|t| {
            let strings_of = |field: &str, key: &str| -> Vec<String> {
                t.get(field)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|e| e.get(key).and_then(|c| c.as_str()))
                            .map(normalize)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            TaskInfo {
                id: t.get("id").and_then(|v| v.as_str()).unwrap_or("?").to_string(),
                name: t.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                kind: t.get("type").and_then(|v| v.as_str()).map(str::to_string),
                edge: strings_of("edge_cases", "case"),
                tests: strings_of("test_cases", "name"),
                ui: t.get("ui").and_then(|v| v.as_bool()).unwrap_or(false),
            }
        })
        .collect();

    let typed = infos.iter().any(|t| t.kind.is_some());
    let is_test_kind = |k: &Option<String>| matches!(k.as_deref(), Some("test") | Some("e2e"));

    for row in spec_table_rows(spec, "Edge Cases") {
        let norm = normalize(&row);
        if norm.starts_with("none") {
            continue; // waiver row: | None for REQ-xx | — | — | <reason> |
        }
        // A task may append detail to the case text, but never summarize it
        let carriers: Vec<&TaskInfo> = infos
            .iter()
            .filter(|t| t.edge.iter().any(|c| c == &norm || c.contains(&norm)))
            .collect();
        if carriers.is_empty() {
            errors.push(format!(
                "Edge case \"{row}\" from TEST-SPEC is not embedded in any task's edge_cases — copy the row into the implementing task AND its test task"
            ));
        } else if typed {
            if !carriers.iter().any(|t| is_test_kind(&t.kind)) {
                errors.push(format!(
                    "Edge case \"{row}\" is not embedded in any test/e2e task's edge_cases"
                ));
            }
            if !carriers.iter().any(|t| !is_test_kind(&t.kind)) {
                errors.push(format!(
                    "Edge case \"{row}\" is not embedded in any implementation task's edge_cases"
                ));
            }
        } else if carriers.len() == 1 {
            warnings.push(format!(
                "Edge case \"{row}\" is embedded in only one task ({}) — it needs an implementation task AND a test task (tasks carry no type field, cannot verify)",
                carriers[0].id
            ));
        }
    }

    for heading in ["Unit Tests", "Integration Tests", "E2E Tests"] {
        for name in spec_test_names(spec, heading) {
            let norm = normalize(&name);
            if !infos.iter().any(|t| t.tests.contains(&norm)) {
                errors.push(format!(
                    "{heading} test \"{name}\" from TEST-SPEC is not embedded in any task's test_cases"
                ));
            }
        }
    }

    if !spec_test_names(spec, "E2E Tests").is_empty() {
        let has_e2e_task = if typed {
            infos.iter().any(|t| t.kind.as_deref() == Some("e2e"))
        } else {
            infos
                .iter()
                .any(|t| normalize(&t.id).contains("e2e") || normalize(&t.name).contains("e2e"))
        };
        if !has_e2e_task {
            errors.push(
                "TEST-SPEC has ## E2E Tests but plan has no e2e task (type: \"e2e\")".to_string(),
            );
        }
    }

    if parse_frontmatter(spec)
        .as_ref()
        .and_then(|m| m.get("surface"))
        .map(|s| s.as_str())
        == Some("ui")
        && !infos.iter().any(|t| t.ui)
    {
        errors.push(
            "TEST-SPEC has surface: ui but no task is flagged \"ui\": true — visual verification will never trigger"
                .to_string(),
        );
    }

    (errors, warnings)
}

/// Trimmed lines of the first `## ` section whose heading satisfies `matches`.
fn section_lines_by(content: &str, matches: impl Fn(&str) -> bool) -> Vec<&str> {
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(h) = trimmed.strip_prefix("## ") {
            if in_section {
                break;
            }
            in_section = matches(h.trim());
            continue;
        }
        if in_section {
            lines.push(trimmed);
        }
    }
    lines
}

/// Trimmed lines belonging to the (case-insensitive) `## <heading>` section.
fn section_lines<'a>(content: &'a str, heading: &str) -> Vec<&'a str> {
    section_lines_by(content, |h| h.eq_ignore_ascii_case(heading))
}

/// Full table data-row lines under `## <heading>` (header + `|---|` excluded).
fn section_table_lines(content: &str, heading: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut seen_header = false;
    for line in section_lines(content, heading) {
        if !line.starts_with('|') {
            continue;
        }
        let inner: String = line
            .chars()
            .filter(|c| !matches!(c, '|' | '-' | ':' | ' '))
            .collect();
        if inner.is_empty() {
            continue; // separator row
        }
        if !seen_header {
            seen_header = true;
            continue;
        }
        rows.push(line.to_string());
    }
    rows
}

/// First-column values of the table data rows under `## <heading>`
/// (header row and |---| separator rows excluded).
fn spec_table_rows(content: &str, heading: &str) -> Vec<String> {
    let mut rows = Vec::new();
    let mut seen_header = false;
    for line in section_lines(content, heading) {
        if !line.starts_with('|') {
            continue;
        }
        let inner: String = line
            .chars()
            .filter(|c| !matches!(c, '|' | '-' | ':' | ' '))
            .collect();
        if inner.is_empty() {
            continue; // separator row
        }
        if !seen_header {
            seen_header = true;
            continue;
        }
        if let Some(cell) = line.trim_matches('|').split('|').next() {
            let cell = cell.trim();
            if !cell.is_empty() {
                rows.push(cell.to_string());
            }
        }
    }
    rows
}

/// `### Test: <name>` names under `## <heading>`.
fn spec_test_names(content: &str, heading: &str) -> Vec<String> {
    section_lines(content, heading)
        .into_iter()
        .filter_map(|l| l.strip_prefix("### Test:"))
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect()
}

/// Lowercase + collapse internal whitespace, for forgiving text matching.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

/// `plan resolve <plan_path>` — resolve and normalize all paths in plan.json.
///
/// For each task's `files` and `context_pointers`:
///   - Relative paths → joined with workspace_dir to make absolute
///   - Non-existent absolute paths → fuzzy-matched against actual workspace files
///   - Writes corrected plan.json back + reports what changed
pub fn cmd_resolve(file_path: &str) {
    if !Path::new(file_path).exists() {
        out(&json!({ "error": format!("Plan file not found: {}", file_path) }));
        return;
    }
    let mut plan = read_json(file_path);
    if plan.get("error").is_some() {
        out(&json!({ "error": plan["error"] }));
        return;
    }

    let workspace_dir = match plan.get("workspace_dir").and_then(|v| v.as_str()) {
        Some(wd) if !wd.is_empty() && is_absolute(wd) => wd.to_string(),
        _ => {
            out(&json!({ "error": "plan.json missing or invalid workspace_dir" }));
            return;
        }
    };

    // Build file index of workspace for fuzzy matching
    let workspace_path = Path::new(&workspace_dir);
    let file_index = build_file_index(workspace_path);

    let mut fixes: Vec<Value> = Vec::new();

    if let Some(tasks) = plan.get_mut("tasks").and_then(|v| v.as_array_mut()) {
        for task in tasks.iter_mut() {
            let tid = task
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("?")
                .to_string();

            // Resolve files[]
            if let Some(files) = task.get_mut("files").and_then(|v| v.as_array_mut()) {
                for file_val in files.iter_mut() {
                    if let Some(fp) = file_val.as_str().map(|s| s.to_string())
                        && let Some((resolved, reason)) =
                            resolve_path(&fp, &workspace_dir, workspace_path, &file_index)
                        {
                            fixes.push(json!({
                                "task": tid,
                                "field": "files",
                                "old": fp,
                                "new": resolved,
                                "reason": reason,
                            }));
                            *file_val = Value::String(resolved);
                        }
                }
            }

            // Resolve context_pointers[]
            if let Some(pointers) = task
                .get_mut("context_pointers")
                .and_then(|v| v.as_array_mut())
            {
                for ptr_val in pointers.iter_mut() {
                    if let Some(ps) = ptr_val.as_str().map(|s| s.to_string()) {
                        // Split off :L1-L2 suffix
                        let (path_part, line_suffix) = match ps.rfind(':') {
                            Some(i) if ps[i + 1..].contains('-') => {
                                (&ps[..i], Some(&ps[i..]))
                            }
                            _ => (ps.as_str(), None),
                        };
                        if let Some((resolved, reason)) =
                            resolve_path(path_part, &workspace_dir, workspace_path, &file_index)
                        {
                            let new_val = match line_suffix {
                                Some(suffix) => format!("{resolved}{suffix}"),
                                None => resolved.clone(),
                            };
                            fixes.push(json!({
                                "task": tid,
                                "field": "context_pointers",
                                "old": ps,
                                "new": new_val,
                                "reason": reason,
                            }));
                            *ptr_val = Value::String(new_val);
                        }
                    }
                }
            }
        }
    }

    if fixes.is_empty() {
        out(&json!({ "resolved": true, "fixes": [], "message": "All paths already valid" }));
        return;
    }

    // Write back
    match std::fs::write(file_path, serde_json::to_string_pretty(&plan).unwrap()) {
        Ok(_) => out(&json!({
            "resolved": true,
            "fixes": fixes,
            "fix_count": fixes.len(),
        })),
        Err(e) => out(&json!({ "error": format!("Failed to write plan: {}", e) })),
    }
}

/// Build a flat index of all files in workspace (relative to workspace root).
fn build_file_index(workspace: &Path) -> Vec<String> {
    let mut files = Vec::new();
    fn walk(dir: &Path, root: &Path, out: &mut Vec<String>) {
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            // Skip hidden dirs and common non-source dirs
            if name.starts_with('.')
                || name == "node_modules"
                || name == "target"
                || name == "__pycache__"
                || name == "dist"
                || name == "build"
            {
                continue;
            }
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().to_string());
            }
        }
    }
    walk(workspace, workspace, &mut files);
    files
}

/// Try to resolve a single path. Returns Some((resolved, reason)) if changed, None if already ok.
fn resolve_path(
    path: &str,
    workspace_dir: &str,
    workspace_path: &Path,
    file_index: &[String],
) -> Option<(String, String)> {
    // Case 1: relative path → make absolute
    if !is_absolute(path) {
        let joined = workspace_path.join(path);
        let abs = joined.to_string_lossy().to_string();
        if joined.exists() {
            return Some((abs, "relative→absolute".to_string()));
        }
        // Relative and doesn't exist — try fuzzy match
        if let Some(matched) = fuzzy_match(path, file_index) {
            let resolved = workspace_path.join(&matched).to_string_lossy().to_string();
            return Some((resolved, format!("relative+fuzzy:{path}→{matched}")));
        }
        // Still make it absolute even if file doesn't exist (CREATE case)
        return Some((abs, "relative→absolute(new file)".to_string()));
    }

    // Case 2: absolute path that doesn't exist — try fuzzy match
    let abs_path = Path::new(path);
    if !abs_path.exists() {
        // Extract relative part from workspace_dir
        if let Ok(rel) = abs_path.strip_prefix(workspace_dir) {
            let rel_str = rel.to_string_lossy().to_string();
            if let Some(matched) = fuzzy_match(&rel_str, file_index) {
                let resolved = workspace_path.join(&matched).to_string_lossy().to_string();
                return Some((resolved, format!("fuzzy:{rel_str}→{matched}")));
            }
        }
        // Try matching just the filename
        if let Some(fname) = abs_path.file_name().and_then(|f| f.to_str())
            && let Some(matched) = fuzzy_match(fname, file_index) {
                let resolved = workspace_path.join(&matched).to_string_lossy().to_string();
                return Some((resolved, format!("fuzzy_filename:{fname}→{matched}")));
            }
    }

    None // path is absolute and exists — no change needed
}

/// Fuzzy match a path fragment against the file index.
/// Tries: exact match → suffix match → filename match.
fn fuzzy_match(query: &str, file_index: &[String]) -> Option<String> {
    // Exact relative match
    if file_index.contains(&query.to_string()) {
        return None; // exact match means no fix needed (caller handles)
    }

    // Suffix match — find files ending with the query
    let suffix_matches: Vec<&String> = file_index
        .iter()
        .filter(|f| f.ends_with(query) || f.ends_with(&format!("/{query}")))
        .collect();
    if suffix_matches.len() == 1 {
        return Some(suffix_matches[0].clone());
    }

    // Filename match
    let fname = Path::new(query)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(query);
    let name_matches: Vec<&String> = file_index
        .iter()
        .filter(|f| {
            Path::new(f.as_str())
                .file_name()
                .and_then(|n| n.to_str())
                == Some(fname)
        })
        .collect();
    if name_matches.len() == 1 {
        return Some(name_matches[0].clone());
    }

    None // ambiguous or no match
}

/// `plan task-ids <path>` — extract task IDs from a plan.json file.
pub fn cmd_task_ids(file_path: &str) {
    if !Path::new(file_path).exists() {
        out(&json!({ "error": format!("Plan file not found: {}", file_path) }));
        return;
    }
    let plan = read_json(file_path);
    if plan.get("error").is_some() {
        out(&json!({ "error": plan["error"] }));
        return;
    }
    let task_ids: Vec<&str> = plan
        .get("tasks")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
                .collect()
        })
        .unwrap_or_default();
    out(&json!({ "task_ids": task_ids }));
}

/// `validate scope <sessionDir> <taskId> [--rev <sha>]` — did the worker stay
/// inside `task.files`? Worker rules say only listed files may be touched; this
/// is the check that makes that a gate instead of an honor system.
pub fn cmd_scope(session_dir: &str, task_id: &str, rev: &str) {
    let plan_path = Path::new(session_dir).join("plan.json");
    let plan = read_json(plan_path.to_str().unwrap_or(""));
    if plan.get("error").is_some() {
        out(&json!({ "valid": false, "errors": [format!("plan.json not found or invalid in {session_dir}")] }));
        return;
    }
    let workspace = plan
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(".");
    let empty = Vec::new();
    let task = plan
        .get("tasks")
        .and_then(|v| v.as_array())
        .unwrap_or(&empty)
        .iter()
        .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(task_id));
    let Some(task) = task else {
        out(&json!({ "valid": false, "errors": [format!("Task not found in plan: {task_id}")] }));
        return;
    };
    let task_files: Vec<String> = task
        .get("files")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|f| f.as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let git = |args: &[&str]| -> Option<String> {
        let o = std::process::Command::new("git")
            .args(args)
            .current_dir(workspace)
            .output()
            .ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };

    let repo_root = git(&["rev-parse", "--show-toplevel"]).unwrap_or_else(|| workspace.to_string());

    // A merge commit prints NO file list for `git show --name-only`, which the
    // diff below would read as "touched nothing" and pass. The one commit
    // shape that can carry arbitrary files must not be the one shape the gate
    // waves through.
    if let Some(parents) = git(&["rev-list", "--parents", "-n", "1", rev])
        && parents.split_whitespace().count() > 2
    {
        out(&json!({
            "valid": false,
            "errors": [format!(
                "{rev} is a merge commit — scope cannot be verified from it. \
                 Point --rev at the task's own commit."
            )],
        }));
        return;
    }

    let output = std::process::Command::new("git")
        .args(["show", "--name-only", "--pretty=format:", rev])
        .current_dir(workspace)
        .output();
    let changed: Vec<String> = match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr).trim().to_string();
            out(&json!({ "valid": false, "errors": [format!("git show {rev} failed: {err}")] }));
            return;
        }
        Err(e) => {
            out(&json!({ "valid": false, "errors": [format!("git not runnable in {workspace}: {e}")] }));
            return;
        }
    };

    let (unexpected, untouched) = scope_diff(&task_files, &changed, workspace, &repo_root);
    let errors: Vec<String> = unexpected
        .iter()
        .map(|f| {
            format!(
                "Task {task_id} touched {f}, which is not in its files — a worker may only modify files the plan gave it"
            )
        })
        .collect();
    let warnings: Vec<String> = untouched
        .iter()
        .map(|f| format!("Task {task_id}: {f} is in files but unchanged by {rev}"))
        .collect();
    out(&json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "rev": rev,
        "changed": changed,
    }));
}

/// Split a commit's changed paths against a task's declared `files`, returning
/// (touched-but-undeclared, declared-but-untouched). `changed` entries are
/// repo-relative; declared files are absolute (`plan resolve` guarantees it),
/// so compare on the workspace-joined form of each.
fn scope_diff(
    task_files: &[String],
    changed: &[String],
    workspace: &str,
    repo_root: &str,
) -> (Vec<String>, Vec<String>) {
    // The two lists are relative to DIFFERENT roots. `git show --name-only`
    // always prints repo-root-relative paths regardless of the directory it
    // runs in, while `task.files` are workspace-relative (or absolute).
    // Resolving both against `workspace` made every task in a monorepo look
    // like it had touched an undeclared file AND left its declared files
    // untouched — the same file, reported twice, contradicting itself.
    // Canonicalize so the two sides agree through symlinks: `git rev-parse
    // --show-toplevel` resolves them (/tmp -> /private/tmp on macOS) while
    // `task.files` carry whatever `workspace_dir` was written as. Falls back
    // to the literal path when the file no longer exists (a deleted file is
    // still in scope).
    let against = |base: &str, p: &str| -> String {
        let joined = if is_absolute(p) {
            PathBuf::from(p)
        } else {
            Path::new(base).join(p)
        };
        std::fs::canonicalize(&joined)
            .unwrap_or(joined)
            .to_string_lossy()
            .to_string()
    };
    let declared: Vec<String> = task_files.iter().map(|f| against(workspace, f)).collect();
    let touched: Vec<String> = changed.iter().map(|c| against(repo_root, c)).collect();
    let unexpected = changed
        .iter()
        .zip(touched.iter())
        .filter(|(_, t)| !declared.contains(t))
        .map(|(orig, _)| orig.clone())
        .collect();
    let untouched = task_files
        .iter()
        .zip(declared.iter())
        .filter(|(_, d)| !touched.contains(d))
        .map(|(orig, _)| orig.clone())
        .collect();
    (unexpected, untouched)
}

/// `validate spec <path>`
pub fn cmd_spec(file_path: &str) {
    let content = match read_file(file_path) {
        Some(c) => c,
        None => {
            out(&json!({ "valid": false, "errors": ["File not found"] }));
            return;
        }
    };

    let (errors, warnings, component) = validate_spec_content(&content);

    out(&json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "component": component.map(Value::String).unwrap_or(Value::Null),
    }));
}

/// The risk classes a code DESIGN-SPEC must answer, with the keywords that
/// identify each row. Concurrency/TOCTOU, idempotency and partial failure are
/// the ones that get silently skipped — the sweep exists to make a spec say
/// "N/A because …" out loud instead of never considering them at all.
const RISK_CLASSES: [(&str, &[&str]); 8] = [
    ("Boundary / empty input", &["boundary", "empty input"]),
    ("Invalid / malformed input", &["invalid", "malformed"]),
    ("Concurrency & TOCTOU", &["concurren", "toctou", "race"]),
    ("Idempotency & retry", &["idempot", "retry"]),
    ("Partial failure & rollback", &["partial failure", "rollback"]),
    ("Auth & permission", &["auth", "permission"]),
    ("Limits (size / timeout / rate)", &["limit", "timeout"]),
    ("Backward compat / migration", &["compat", "migration"]),
];

/// Validate DESIGN-SPEC content. Category-aware: a code spec (anything not
/// explicitly `ops`/`content`) additionally demands a non-empty
/// `## Behavior / Logic` — the logic a fresh-context worker implements from —
/// a `## Risk Sweep` covering every class in [`RISK_CLASSES`], and an explicit
/// `RESOLVED`/`DEFERRED` status on every `## Open Questions` entry, so a
/// question cannot reach the plan without having been put to the user.
fn validate_spec_content(content: &str) -> (Vec<String>, Vec<String>, Option<String>) {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let fm = parse_frontmatter(content);

    match &fm {
        None => {
            errors.push("Missing YAML frontmatter (--- delimiters)".to_string());
        }
        Some(map) => {
            for f in &["spec_version", "project", "component", "language", "status"] {
                if !map.contains_key(*f) {
                    errors.push(format!("Frontmatter missing: {f}"));
                }
            }
        }
    }

    if !content.contains("## Types") {
        warnings.push("Missing ## Types section".to_string());
    }
    if !content.contains("## Interfaces") {
        warnings.push("Missing ## Interfaces section".to_string());
    }
    // `## Implementations` is checked inside the `is_code` branch below —
    // menu.md tells ops/content authors to delete the code-only sections, so
    // demanding it from every category made Gate 1 unpassable for exactly the
    // specs written to spec.
    if !content.contains("## Acceptance") {
        errors.push("Missing ## Acceptance Criteria section".to_string());
    }

    let code_block_count = content.matches("```").count() / 2;
    if code_block_count < 2 {
        warnings.push("Expected code blocks in Types and Interfaces sections".to_string());
    }

    // Only an explicit ops/content spec opts out of the code gates — a missing or
    // unrecognized category (including an unfilled template placeholder) is treated
    // as code, so the strictest path is the one you get by forgetting.
    let category = fm.as_ref().and_then(|m| m.get("category")).map(String::as_str);
    let is_code = !matches!(category, Some("ops") | Some("content"));
    match category {
        None => warnings.push("Frontmatter missing: category (assuming code)".to_string()),
        Some(c) if is_code && c != "code" => warnings.push(format!(
            "Unknown category: {c} (expected code|ops|content — assuming code)"
        )),
        _ => {}
    }

    if is_code {
        if !content.contains("## Implementations") {
            errors.push(
                "Missing ## Implementations section (code specs only — an ops or content \
                 spec should set `category` in the frontmatter)"
                    .to_string(),
            );
        }
        if !section_has_entries_prefixed(content, "Behavior") {
            errors.push(
                "Missing or empty ## Behavior / Logic — a code spec must state the logic per REQ \
                 (trigger, steps, branch conditions, error paths), or the worker invents it. \
                 A REQ that is pure data gets one waiver line: `[REQ-xx] N/A — <reason>`"
                    .to_string(),
            );
        }
        errors.extend(risk_sweep_errors(content));
        errors.extend(open_question_errors(content));
    }

    let component = fm.as_ref().and_then(|m| m.get("component")).cloned();
    (errors, warnings, component)
}

/// Every [`RISK_CLASSES`] entry must appear in the `## Risk Sweep` table.
/// A row may be `N/A` — with a reason — but a class may not be absent.
fn risk_sweep_errors(content: &str) -> Vec<String> {
    let rows: Vec<String> = section_table_lines(content, "Risk Sweep")
        .iter()
        .map(|l| normalize(l))
        .collect();
    if rows.is_empty() {
        return vec![
            "Missing or empty ## Risk Sweep — a code spec must answer all 8 risk classes \
             (boundary, invalid input, concurrency & TOCTOU, idempotency & retry, partial \
             failure & rollback, auth & permission, limits, backward compat), each APPLIES \
             with concrete handling or N/A with a reason"
                .to_string(),
        ];
    }
    let missing: Vec<&str> = RISK_CLASSES
        .iter()
        .filter(|(_, keys)| !rows.iter().any(|row| keys.iter().any(|k| row.contains(*k))))
        .map(|(label, _)| *label)
        .collect();
    if missing.is_empty() {
        return Vec::new();
    }
    vec![format!(
        "## Risk Sweep is missing risk class(es): {} — answer each one (APPLIES + handling, or \
         N/A + reason); dropping a row is not the same as ruling it out",
        missing.join(", ")
    )]
}

/// Every `## Open Questions` entry must carry `RESOLVED` or `DEFERRED`.
/// A statusless entry is a question that was never put to the user.
fn open_question_errors(content: &str) -> Vec<String> {
    let mut entries: Vec<String> = section_table_lines(content, "Open Questions");
    entries.extend(
        section_lines(content, "Open Questions")
            .iter()
            .filter(|l| l.starts_with("- "))
            .map(|l| l.to_string()),
    );
    entries
        .iter()
        .filter(|line| {
            let n = normalize(line);
            // `| None | … |` / `- None` waive the section outright
            let first = n.trim_matches('|').split('|').next().unwrap_or("").trim();
            let waived = first.is_empty()
                || first.starts_with("none")
                || n.trim_start_matches("- ").starts_with("none");
            !waived && !n.contains("resolved") && !n.contains("deferred")
        })
        .map(|line| {
            format!(
                "## Open Questions entry has no status: {} — every question must be surfaced to \
                 the user and marked RESOLVED (with the answer) or DEFERRED (with the assumption \
                 we proceed under and its impact)",
                line.trim()
            )
        })
        .collect()
}

/// `validate tests <path>`
pub fn cmd_tests(file_path: &str) {
    let content = match read_file(file_path) {
        Some(c) => c,
        None => {
            out(&json!({ "valid": false, "errors": ["File not found"] }));
            return;
        }
    };

    let (errors, warnings, component) = validate_tests_content(&content);

    out(&json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "component": component.map(Value::String).unwrap_or(Value::Null),
    }));
}

/// Validate TEST-SPEC content. Category-aware: `code` (default) demands
/// unit/integration sections, a non-empty Edge Cases table, E2E tests when
/// `surface` is ui/api/cli, and a Visual Verification table when `surface: ui`;
/// `ops` demands Smoke Tests; `content` demands a Deliverable Checklist.
fn validate_tests_content(content: &str) -> (Vec<String>, Vec<String>, Option<String>) {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let fm = parse_frontmatter(content);

    match &fm {
        None => {
            errors.push("Missing YAML frontmatter".to_string());
        }
        Some(map) => {
            for f in &["tests_version", "spec_ref", "component"] {
                if !map.contains_key(*f) {
                    errors.push(format!("Frontmatter missing: {f}"));
                }
            }
        }
    }

    let category = match fm.as_ref().and_then(|m| m.get("category")) {
        Some(c) => c.clone(),
        None => {
            warnings.push("Frontmatter missing: category (assuming code)".to_string());
            "code".to_string()
        }
    };

    match category.as_str() {
        "ops" => {
            if !content.contains("## Smoke Tests") {
                errors.push("ops TEST-SPEC must have a ## Smoke Tests section".to_string());
            }
            if !section_has_entries(content, "Edge Cases") {
                warnings.push("## Edge Cases section is missing or empty".to_string());
            }
        }
        "content" => {
            if !content.contains("## Deliverable Checklist") {
                errors.push(
                    "content TEST-SPEC must have a ## Deliverable Checklist section".to_string(),
                );
            }
        }
        _ => {
            let has_unit = content.contains("## Unit Tests");
            let has_integration = content.contains("## Integration Tests");
            if !has_unit && !has_integration {
                errors.push(
                    "Must have at least one of: ## Unit Tests, ## Integration Tests".to_string(),
                );
            }
            if !section_has_entries(content, "Edge Cases") {
                errors.push(
                    "## Edge Cases must exist with at least one entry — if a spec truly has \
                     no edge cases, add an explicit waiver row: | None for REQ-xx | — | — | <reason> |"
                        .to_string(),
                );
            }
            match fm.as_ref().and_then(|m| m.get("surface")).map(|s| s.as_str()) {
                Some(s @ ("ui" | "api" | "cli" | "user-facing")) => {
                    if !section_has_entries(content, "E2E Tests") {
                        errors.push(format!(
                            "surface: {s} requires an ## E2E Tests section with at least one test"
                        ));
                    }
                    if s == "ui" && !section_has_entries(content, "Visual Verification") {
                        errors.push(
                            "surface: ui requires a ## Visual Verification table with at least one row"
                                .to_string(),
                        );
                    }
                    if s == "user-facing" {
                        warnings.push(
                            "surface: user-facing is deprecated — use ui|api|cli (ui additionally requires ## Visual Verification)"
                                .to_string(),
                        );
                    }
                }
                Some("internal") => {}
                Some(other) => {
                    warnings.push(format!(
                        "Unknown surface value: {other} (expected ui|api|cli|internal)"
                    ));
                }
                None => {
                    warnings.push(
                        "Frontmatter missing: surface (ui|api|cli|internal) — E2E requirement cannot be enforced"
                            .to_string(),
                    );
                }
            }
        }
    }

    let component = fm.as_ref().and_then(|m| m.get("component")).cloned();
    (errors, warnings, component)
}

/// True when the `## <heading>` section contains at least one entry:
/// a markdown table with ≥1 data row (header + separator don't count),
/// a bullet item, or a `###` sub-block.
fn section_has_entries(content: &str, heading: &str) -> bool {
    has_entries(&section_lines(content, heading))
}

/// Same, for a section whose heading merely *starts with* `prefix` — so
/// `## Behavior`, `## Behavior / Logic` and `## Behavior & Flow` all count.
fn section_has_entries_prefixed(content: &str, prefix: &str) -> bool {
    let prefix = prefix.to_lowercase();
    has_entries(&section_lines_by(content, |h| h.to_lowercase().starts_with(&prefix)))
}

fn has_entries(lines: &[&str]) -> bool {
    let mut pipe_rows = 0usize;
    let mut items = 0usize;
    for trimmed in lines {
        if trimmed.starts_with('|') {
            // Separator rows (|---|:---:|) reduce to nothing once structural chars are removed
            let inner: String = trimmed
                .chars()
                .filter(|c| !matches!(c, '|' | '-' | ':' | ' '))
                .collect();
            if !inner.is_empty() {
                pipe_rows += 1;
            }
        } else if trimmed.starts_with("- ") || trimmed.starts_with("### ") {
            items += 1;
        }
    }
    // A table needs header + ≥1 data row; bullets/sub-blocks count directly
    pipe_rows >= 2 || items >= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    const FM_CODE: &str = "---\ntests_version: \"1.0\"\nspec_ref: \"x-spec-v1.0\"\ncomponent: \"x\"\ncategory: \"code\"\n";

    fn code_spec(surface: &str, body: &str) -> String {
        format!("{FM_CODE}surface: \"{surface}\"\n---\n\n{body}")
    }

    const EDGE_TABLE: &str = "## Edge Cases\n| Case | Input | Expected | Covers |\n|------|-------|----------|--------|\n| empty list | [] | returns [] | REQ-01 |\n";
    const E2E_BLOCK: &str = "## E2E Tests\n\n### Test: full_flow\n- **Verify**: `npx playwright test e2e/flow.spec.ts`\n";

    #[test]
    fn code_spec_with_edge_cases_and_e2e_is_valid() {
        let spec = code_spec("api", &format!("## Unit Tests\n\n### Test: a\n\n{EDGE_TABLE}\n{E2E_BLOCK}"));
        let (errors, warnings, component) = validate_tests_content(&spec);
        assert!(errors.is_empty(), "unexpected errors: {errors:?}");
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert_eq!(component.as_deref(), Some("x"));
    }

    #[test]
    fn code_spec_missing_edge_cases_errors() {
        let spec = code_spec("internal", "## Unit Tests\n\n### Test: a\n");
        let (errors, _, _) = validate_tests_content(&spec);
        assert!(errors.iter().any(|e| e.contains("Edge Cases")), "{errors:?}");
    }

    #[test]
    fn code_spec_empty_edge_case_table_errors() {
        let spec = code_spec(
            "internal",
            "## Unit Tests\n\n### Test: a\n\n## Edge Cases\n| Case | Input | Expected | Covers |\n|------|-------|----------|--------|\n",
        );
        let (errors, _, _) = validate_tests_content(&spec);
        assert!(errors.iter().any(|e| e.contains("Edge Cases")), "{errors:?}");
    }

    #[test]
    fn edge_case_waiver_row_counts_as_entry() {
        let spec = code_spec(
            "internal",
            "## Unit Tests\n\n### Test: a\n\n## Edge Cases\n| Case | Input | Expected | Covers |\n|------|-------|----------|--------|\n| None for REQ-01 | — | — | pure constant lookup |\n",
        );
        let (errors, _, _) = validate_tests_content(&spec);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn user_facing_without_e2e_errors() {
        let spec = code_spec("user-facing", &format!("## Unit Tests\n\n### Test: a\n\n{EDGE_TABLE}"));
        let (errors, warnings, _) = validate_tests_content(&spec);
        assert!(errors.iter().any(|e| e.contains("E2E")), "{errors:?}");
        assert!(warnings.iter().any(|w| w.contains("deprecated")), "{warnings:?}");
    }

    #[test]
    fn cli_surface_requires_e2e() {
        let spec = code_spec("cli", &format!("## Unit Tests\n\n### Test: a\n\n{EDGE_TABLE}"));
        let (errors, _, _) = validate_tests_content(&spec);
        assert!(errors.iter().any(|e| e.contains("E2E")), "{errors:?}");
    }

    #[test]
    fn ui_surface_requires_visual_verification() {
        let spec = code_spec("ui", &format!("## Unit Tests\n\n### Test: a\n\n{EDGE_TABLE}\n{E2E_BLOCK}"));
        let (errors, _, _) = validate_tests_content(&spec);
        assert!(
            errors.iter().any(|e| e.contains("Visual Verification")),
            "{errors:?}"
        );

        let with_vv = code_spec(
            "ui",
            &format!(
                "## Unit Tests\n\n### Test: a\n\n{EDGE_TABLE}\n{E2E_BLOCK}\n## Visual Verification\n| Screen / Component | States to verify | How |\n|---|---|---|\n| Profile form | empty / loading / error | run app + screenshot |\n"
            ),
        );
        let (errors, warnings, _) = validate_tests_content(&with_vv);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn section_heading_match_is_case_insensitive() {
        let spec = code_spec(
            "internal",
            "## Unit Tests\n\n### Test: a\n\n## Edge cases\n| Case | Input | Expected | Covers |\n|------|-------|----------|--------|\n| empty list | [] | returns [] | REQ-01 |\n",
        );
        let (errors, _, _) = validate_tests_content(&spec);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn internal_surface_does_not_require_e2e() {
        let spec = code_spec("internal", &format!("## Unit Tests\n\n### Test: a\n\n{EDGE_TABLE}"));
        let (errors, _, _) = validate_tests_content(&spec);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn missing_surface_warns_but_does_not_error() {
        let spec = format!("{FM_CODE}---\n\n## Unit Tests\n\n### Test: a\n\n{EDGE_TABLE}");
        let (errors, warnings, _) = validate_tests_content(&spec);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(warnings.iter().any(|w| w.contains("surface")), "{warnings:?}");
    }

    #[test]
    fn missing_category_assumes_code_with_warning() {
        let spec = "---\ntests_version: \"1.0\"\nspec_ref: \"x-spec-v1.0\"\ncomponent: \"x\"\n---\n\n## Unit Tests\n\n### Test: a\n";
        let (errors, warnings, _) = validate_tests_content(spec);
        assert!(warnings.iter().any(|w| w.contains("category")), "{warnings:?}");
        assert!(errors.iter().any(|e| e.contains("Edge Cases")), "{errors:?}");
    }

    #[test]
    fn ops_spec_requires_smoke_tests_not_unit_tests() {
        let spec = "---\ntests_version: \"1.0\"\nspec_ref: \"x-spec-v1.0\"\ncomponent: \"x\"\ncategory: \"ops\"\n---\n\n## Smoke Tests\n\n### Check: container_up\n- **Command**: `docker compose ps`\n";
        let (errors, _, _) = validate_tests_content(spec);
        assert!(errors.is_empty(), "{errors:?}");

        let bad = "---\ntests_version: \"1.0\"\nspec_ref: \"x-spec-v1.0\"\ncomponent: \"x\"\ncategory: \"ops\"\n---\n\n## Pre-flight Checks\n- [ ] daemon running\n";
        let (errors, _, _) = validate_tests_content(bad);
        assert!(errors.iter().any(|e| e.contains("Smoke Tests")), "{errors:?}");
    }

    #[test]
    fn content_spec_requires_deliverable_checklist() {
        let spec = "---\ntests_version: \"1.0\"\nspec_ref: \"x-spec-v1.0\"\ncomponent: \"x\"\ncategory: \"content\"\n---\n\n## Deliverable Checklist\n\n### [REQ-01] README\n- [ ] File exists\n";
        let (errors, _, _) = validate_tests_content(spec);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn missing_frontmatter_errors() {
        let (errors, _, _) = validate_tests_content("## Unit Tests\n");
        assert!(errors.iter().any(|e| e.contains("frontmatter")), "{errors:?}");
    }

    // --- plan ↔ TEST-SPEC cross-check ---

    fn ui_spec() -> String {
        code_spec(
            "ui",
            "## Unit Tests\n\n### Test: rejects_invalid_email\n\n## Edge Cases\n| Case | Input | Expected | Covers |\n|------|-------|----------|--------|\n| empty list | [] | returns [] | REQ-01 |\n| None for REQ-02 | — | — | pure lookup |\n\n## E2E Tests\n\n### Test: full_signup_flow\n",
        )
    }

    fn task(id: &str, kind: &str, edge_case: Option<&str>, test_name: Option<&str>, ui: bool) -> Value {
        json!({
            "id": id,
            "name": id,
            "type": kind,
            "ui": ui,
            "edge_cases": edge_case.map(|c| vec![json!({"case": c, "input": "[]", "expected": "returns []", "covers": "REQ-01"})]).unwrap_or_default(),
            "test_cases": test_name.map(|n| vec![json!({"name": n, "covers": "REQ-01", "expected": "ok", "verify": "npm test"})]).unwrap_or_default(),
        })
    }

    #[test]
    fn cross_check_passes_when_everything_is_embedded() {
        let plan = json!({ "tasks": [
            task("T-01", "impl", Some("empty list"), None, true),
            task("T-02", "test", Some("empty list"), Some("rejects_invalid_email"), false),
            task("T-03", "e2e", None, Some("full_signup_flow"), false),
        ]});
        let (errors, warnings) = cross_check_plan_tests(&plan, &ui_spec());
        assert!(errors.is_empty(), "{errors:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn cross_check_flags_dropped_edge_case() {
        let plan = json!({ "tasks": [
            task("T-01", "impl", None, None, true),
            task("T-02", "test", None, Some("rejects_invalid_email"), false),
            task("T-03", "e2e", None, Some("full_signup_flow"), false),
        ]});
        let (errors, _) = cross_check_plan_tests(&plan, &ui_spec());
        assert!(
            errors.iter().any(|e| e.contains("empty list") && e.contains("not embedded in any task")),
            "{errors:?}"
        );
    }

    #[test]
    fn cross_check_requires_edge_case_in_both_impl_and_test_tasks() {
        let plan = json!({ "tasks": [
            task("T-01", "impl", Some("empty list"), None, true),
            task("T-02", "test", None, Some("rejects_invalid_email"), false),
            task("T-03", "e2e", None, Some("full_signup_flow"), false),
        ]});
        let (errors, _) = cross_check_plan_tests(&plan, &ui_spec());
        assert!(
            errors.iter().any(|e| e.contains("empty list") && e.contains("test/e2e task")),
            "{errors:?}"
        );
    }

    #[test]
    fn cross_check_flags_missing_e2e_task_and_test() {
        let plan = json!({ "tasks": [
            task("T-01", "impl", Some("empty list"), None, true),
            task("T-02", "test", Some("empty list"), Some("rejects_invalid_email"), false),
        ]});
        let (errors, _) = cross_check_plan_tests(&plan, &ui_spec());
        assert!(
            errors.iter().any(|e| e.contains("full_signup_flow")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("no e2e task")),
            "{errors:?}"
        );
    }

    #[test]
    fn cross_check_flags_ui_spec_without_ui_task() {
        let plan = json!({ "tasks": [
            task("T-01", "impl", Some("empty list"), None, false),
            task("T-02", "test", Some("empty list"), Some("rejects_invalid_email"), false),
            task("T-03", "e2e", None, Some("full_signup_flow"), false),
        ]});
        let (errors, _) = cross_check_plan_tests(&plan, &ui_spec());
        assert!(
            errors.iter().any(|e| e.contains("surface: ui") && e.contains("\"ui\": true")),
            "{errors:?}"
        );
    }

    #[test]
    fn cross_check_untyped_plan_downgrades_to_warning() {
        let plan = json!({ "tasks": [
            {"id": "T-01", "name": "Implement list", "edge_cases": [{"case": "empty list"}], "test_cases": [{"name": "rejects_invalid_email"}]},
            {"id": "T-02", "name": "E2E signup", "test_cases": [{"name": "full_signup_flow"}], "ui": true},
        ]});
        let (errors, warnings) = cross_check_plan_tests(&plan, &ui_spec());
        assert!(
            !errors.iter().any(|e| e.contains("empty list")),
            "{errors:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("empty list") && w.contains("only one task")),
            "{warnings:?}"
        );
    }

    #[test]
    fn cross_check_waiver_rows_are_skipped() {
        let plan = json!({ "tasks": [
            task("T-01", "impl", Some("empty list"), None, true),
            task("T-02", "test", Some("empty list"), Some("rejects_invalid_email"), false),
            task("T-03", "e2e", None, Some("full_signup_flow"), false),
        ]});
        let (errors, _) = cross_check_plan_tests(&plan, &ui_spec());
        assert!(
            !errors.iter().any(|e| e.contains("None for REQ-02")),
            "{errors:?}"
        );
    }

    // --- scope: commit files vs task.files ---

    #[test]
    fn scope_diff_flags_undeclared_files_and_reports_untouched() {
        let declared = vec![
            "/ws/src/a.rs".to_string(),
            "/ws/src/b.rs".to_string(),
        ];
        let changed = vec!["src/a.rs".to_string(), "src/sneaky.rs".to_string()];
        let (unexpected, untouched) = scope_diff(&declared, &changed, "/ws", "/ws");
        assert_eq!(unexpected, vec!["src/sneaky.rs"]);
        assert_eq!(untouched, vec!["/ws/src/b.rs"]);
    }

    #[test]
    fn scope_diff_clean_when_commit_matches_declared_files() {
        let declared = vec!["/ws/src/a.rs".to_string()];
        let changed = vec!["src/a.rs".to_string()];
        let (unexpected, untouched) = scope_diff(&declared, &changed, "/ws", "/ws");
        assert!(unexpected.is_empty(), "{unexpected:?}");
        assert!(untouched.is_empty(), "{untouched:?}");
    }

    #[test]
    fn scope_diff_accepts_relative_task_files() {
        let declared = vec!["src/a.rs".to_string()];
        let changed = vec!["src/a.rs".to_string()];
        let (unexpected, untouched) = scope_diff(&declared, &changed, "/ws", "/ws");
        assert!(unexpected.is_empty() && untouched.is_empty());
    }

    // --- DESIGN-SPEC: behavior, risk sweep, open questions ---

    const RISK_TABLE: &str = "## Risk Sweep\n| Risk class | Applies? | Handling | Edge case ref |\n|---|---|---|---|\n| Boundary / empty input | APPLIES | zero rows returns [] | empty list |\n| Invalid / malformed input | N/A | input is a typed enum | — |\n| Concurrency & TOCTOU | APPLIES | single-writer lock on the session dir | two writers |\n| Idempotency & retry | APPLIES | upsert on (id) | replayed request |\n| Partial failure & rollback | N/A | single atomic write | — |\n| Auth & permission | N/A | internal library, no caller identity | — |\n| Limits (size / timeout / rate) | APPLIES | 1 MiB payload cap | oversized body |\n| Backward compat / migration | N/A | new file, no old readers | — |\n";

    fn design_spec(extra: &str) -> String {
        format!(
            "---\nspec_version: \"1.0\"\nproject: \"p\"\ncomponent: \"x\"\nlanguage: \"rust\"\ncategory: \"code\"\nstatus: \"draft\"\n---\n\n\
             ## Types / Data Models\n```rust\nstruct A;\n```\n\n## Interfaces / APIs\n```rust\nfn a();\n```\n\n\
             ## Behavior / Logic\n\n### [REQ-01] create user\n**Steps:**\n1. reject when email fails RFC5322 → 400\n\n\
             {RISK_TABLE}\n## Implementations\n\n## Acceptance Criteria\n\n{extra}"
        )
    }

    #[test]
    fn code_design_spec_with_behavior_and_risk_sweep_is_valid() {
        let spec = design_spec("## Open Questions\n| Question | Status | Answer | Impact |\n|---|---|---|---|\n| soft delete? | RESOLVED | hard delete, per user | data loss |\n");
        let (errors, warnings, component) = validate_spec_content(&spec);
        assert!(errors.is_empty(), "{errors:?}");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(component.as_deref(), Some("x"));
    }

    #[test]
    fn code_design_spec_without_behavior_errors() {
        let spec = design_spec("").replace("## Behavior / Logic", "## Something Else");
        let (errors, _, _) = validate_spec_content(&spec);
        assert!(errors.iter().any(|e| e.contains("Behavior / Logic")), "{errors:?}");
    }

    #[test]
    fn empty_behavior_section_errors() {
        let spec = design_spec("").replace(
            "### [REQ-01] create user\n**Steps:**\n1. reject when email fails RFC5322 → 400\n",
            "",
        );
        let (errors, _, _) = validate_spec_content(&spec);
        assert!(errors.iter().any(|e| e.contains("Behavior / Logic")), "{errors:?}");
    }

    #[test]
    fn risk_sweep_missing_class_names_it() {
        let spec = design_spec("").replace(
            "| Concurrency & TOCTOU | APPLIES | single-writer lock on the session dir | two writers |\n",
            "",
        );
        let (errors, _, _) = validate_spec_content(&spec);
        assert!(
            errors.iter().any(|e| e.contains("Risk Sweep") && e.contains("Concurrency")),
            "{errors:?}"
        );
    }

    #[test]
    fn missing_risk_sweep_errors() {
        let spec = design_spec("").replace(RISK_TABLE, "");
        let (errors, _, _) = validate_spec_content(&spec);
        assert!(
            errors.iter().any(|e| e.contains("Risk Sweep") && e.contains("8 risk classes")),
            "{errors:?}"
        );
    }

    #[test]
    fn statusless_open_question_errors() {
        let table = design_spec("## Open Questions\n| Question | Status | Answer | Impact |\n|---|---|---|---|\n| soft delete? |  |  | data loss |\n");
        let (errors, _, _) = validate_spec_content(&table);
        assert!(
            errors.iter().any(|e| e.contains("no status") && e.contains("soft delete")),
            "{errors:?}"
        );

        let bullets = design_spec("## Open Questions\n- do we soft delete?\n");
        let (errors, _, _) = validate_spec_content(&bullets);
        assert!(errors.iter().any(|e| e.contains("no status")), "{errors:?}");
    }

    #[test]
    fn deferred_and_none_open_questions_pass() {
        let deferred = design_spec("## Open Questions\n| Question | Status | Answer | Impact |\n|---|---|---|---|\n| soft delete? | DEFERRED | assume hard delete | data loss on rollout |\n");
        let (errors, _, _) = validate_spec_content(&deferred);
        assert!(errors.is_empty(), "{errors:?}");

        let none = design_spec("## Open Questions\n| Question | Status | Answer | Impact |\n|---|---|---|---|\n| None | — | — | — |\n");
        let (errors, _, _) = validate_spec_content(&none);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn ops_design_spec_skips_code_only_gates() {
        let spec = "---\nspec_version: \"1.0\"\nproject: \"p\"\ncomponent: \"x\"\nlanguage: \"docker\"\ncategory: \"ops\"\nstatus: \"draft\"\n---\n\n## Steps / Runbook\n1. build\n\n## Implementations\n\n## Acceptance Criteria\n";
        let (errors, _, _) = validate_spec_content(spec);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn design_spec_without_category_assumes_code() {
        let spec = design_spec("").replace("category: \"code\"\n", "");
        let (errors, warnings, _) = validate_spec_content(&spec);
        assert!(warnings.iter().any(|w| w.contains("category")), "{warnings:?}");
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn impl_task_without_behavior_errors() {
        let mut errors = Vec::new();
        let t = json!({ "type": "impl", "test_cases": [], "edge_cases": [] });
        check_task_spec_fields(&t, "T-01", &mut errors, &mut Vec::new());
        assert!(errors.iter().any(|e| e.contains("missing behavior")), "{errors:?}");

        let mut errors = Vec::new();
        let t = json!({ "type": "impl", "behavior": [], "test_cases": [], "edge_cases": [] });
        check_task_spec_fields(&t, "T-01", &mut errors, &mut Vec::new());
        assert!(errors.iter().any(|e| e.contains("behavior is empty")), "{errors:?}");

        let mut errors = Vec::new();
        let t = json!({ "type": "impl", "behavior": ["N/A — pure type definitions"], "test_cases": [], "edge_cases": [] });
        check_task_spec_fields(&t, "T-01", &mut errors, &mut Vec::new());
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn test_task_needs_no_behavior_but_bad_shape_still_errors() {
        let mut errors = Vec::new();
        let t = json!({ "type": "test", "test_cases": [], "edge_cases": [] });
        check_task_spec_fields(&t, "T-02", &mut errors, &mut Vec::new());
        assert!(errors.is_empty(), "{errors:?}");

        let mut errors = Vec::new();
        let t = json!({ "type": "impl", "behavior": "step one", "test_cases": [], "edge_cases": [] });
        check_task_spec_fields(&t, "T-03", &mut errors, &mut Vec::new());
        assert!(
            errors.iter().any(|e| e.contains("behavior must be an array")),
            "{errors:?}"
        );
    }

    #[test]
    fn task_spec_fields_shape_checks() {
        let mut errors = Vec::new();
        let mut warnings = Vec::new();
        let t = json!({
            "test_cases": [{"name": "a", "expected": "", "verify": "npm test"}],
            "edge_cases": "not-an-array",
            "ui": "yes",
        });
        check_task_spec_fields(&t, "T-01", &mut errors, &mut warnings);
        assert!(
            errors.iter().any(|e| e.contains("test_cases[0]") && e.contains("expected")),
            "{errors:?}"
        );
        assert!(
            errors.iter().any(|e| e.contains("edge_cases must be an array")),
            "{errors:?}"
        );
        assert!(errors.iter().any(|e| e.contains("ui must be a boolean")), "{errors:?}");
        assert!(warnings.iter().any(|w| w.contains("missing type")), "{warnings:?}");
    }

    #[test]
    fn spec_parsers_extract_rows_and_names() {
        let spec = ui_spec();
        assert_eq!(spec_table_rows(&spec, "Edge Cases"), vec!["empty list", "None for REQ-02"]);
        assert_eq!(spec_test_names(&spec, "Unit Tests"), vec!["rejects_invalid_email"]);
        assert_eq!(spec_test_names(&spec, "E2E Tests"), vec!["full_signup_flow"]);
        assert!(spec_test_names(&spec, "Integration Tests").is_empty());
    }

    /// `git show --name-only` prints repo-root-relative paths; `task.files`
    /// are workspace-relative. Resolving both against the workspace made every
    /// task in a monorepo report the SAME file as both undeclared and
    /// untouched.
    #[test]
    fn scope_diff_resolves_each_list_against_its_own_root() {
        let declared = vec!["a.txt".to_string()];       // workspace-relative
        let changed = vec!["sub/a.txt".to_string()];    // repo-root-relative
        let (unexpected, untouched) = scope_diff(&declared, &changed, "/repo/sub", "/repo");
        assert!(unexpected.is_empty(), "same file, got {unexpected:?}");
        assert!(untouched.is_empty(), "same file, got {untouched:?}");

        // A genuinely undeclared file is still caught.
        let changed = vec!["sub/a.txt".to_string(), "sub/b.txt".to_string()];
        let (unexpected, _) = scope_diff(&declared, &changed, "/repo/sub", "/repo");
        assert_eq!(unexpected, vec!["sub/b.txt".to_string()]);
    }
}
