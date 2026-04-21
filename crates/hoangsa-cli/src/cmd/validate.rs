use crate::cmd::dag::{detect_cycles, detect_dangling};
use crate::helpers::{is_absolute, out, parse_frontmatter, read_file, read_json};
use serde_json::{Value, json};
use std::path::Path;

/// `validate plan <path>`
pub fn cmd_plan(file_path: &str) {
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

    let task_count = tasks.map(|a| a.len()).unwrap_or(0);
    out(&json!({
        "valid": errors.is_empty(),
        "errors": errors,
        "warnings": warnings,
        "task_count": task_count,
    }));
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

    let mut errors: Vec<String> = Vec::new();
    let warnings: Vec<String> = Vec::new();
    let fm = parse_frontmatter(&content);

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

    let has_unit = content.contains("## Unit Tests");
    let has_integration = content.contains("## Integration Tests");
    if !has_unit && !has_integration {
        errors.push("Must have at least one of: ## Unit Tests, ## Integration Tests".to_string());
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
