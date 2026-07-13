use crate::cmd::dag::{detect_cycles, detect_dangling};
use crate::helpers::{is_absolute, out, parse_frontmatter, read_file, read_json};
use serde_json::{Value, json};
use std::path::Path;

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
            if let Some(budget) = t.get("budget_tokens").and_then(|v| v.as_u64())
                && budget > 80000 {
                    warnings.push(format!("Task {tid}: budget {budget} exceeds 80k limit"));
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

/// Trimmed lines belonging to the (case-insensitive) `## <heading>` section.
fn section_lines<'a>(content: &'a str, heading: &str) -> Vec<&'a str> {
    let mut in_section = false;
    let mut lines = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(h) = trimmed.strip_prefix("## ") {
            if in_section {
                break;
            }
            in_section = h.trim().eq_ignore_ascii_case(heading);
            continue;
        }
        if in_section {
            lines.push(trimmed);
        }
    }
    lines
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

/// `validate spec <path>`
pub fn cmd_spec(file_path: &str) {
    let content = match read_file(file_path) {
        Some(c) => c,
        None => {
            out(&json!({ "valid": false, "errors": ["File not found"] }));
            return;
        }
    };

    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let fm = parse_frontmatter(&content);

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
    if !content.contains("## Implementations") {
        errors.push("Missing ## Implementations section".to_string());
    }
    if !content.contains("## Acceptance") {
        errors.push("Missing ## Acceptance Criteria section".to_string());
    }

    let code_block_count = content.matches("```").count() / 2;
    if code_block_count < 2 {
        warnings.push("Expected code blocks in Types and Interfaces sections".to_string());
    }

    let component = fm
        .as_ref()
        .and_then(|m| m.get("component"))
        .map(|s| Value::String(s.clone()))
        .unwrap_or(Value::Null);

    out(&json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "component": component,
    }));
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
    let mut in_section = false;
    let mut pipe_rows = 0usize;
    let mut items = 0usize;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(h) = trimmed.strip_prefix("## ") {
            if in_section {
                break;
            }
            in_section = h.trim().eq_ignore_ascii_case(heading);
            continue;
        }
        if !in_section {
            continue;
        }
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
}
