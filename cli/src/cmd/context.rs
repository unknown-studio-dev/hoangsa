use crate::helpers::{out, read_json};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

/// Parse file spec with optional line range.
/// "src/cmd/pref.rs:56-71" → ("src/cmd/pref.rs", Some((56, 71)))
/// "src/cmd/pref.rs"       → ("src/cmd/pref.rs", None)
///
/// Only treats the colon-suffix as a range if it matches `\d+-\d+` to avoid
/// misinterpreting Windows paths like `C:\foo`.
fn parse_file_spec(spec: &str) -> (&str, Option<(usize, usize)>) {
    if let Some(colon_pos) = spec.rfind(':') {
        let suffix = &spec[colon_pos + 1..];
        if let Some(dash_pos) = suffix.find('-') {
            let start_str = &suffix[..dash_pos];
            let end_str = &suffix[dash_pos + 1..];
            if let (Ok(start), Ok(end)) = (start_str.parse::<usize>(), end_str.parse::<usize>()) {
                if start > 0 && end >= start {
                    return (&spec[..colon_pos], Some((start, end)));
                }
            }
        }
    }
    (spec, None)
}

fn build_context_pack(session_dir: &str, task_id: &str) -> Result<Value, Value> {
    let plan_file = Path::new(session_dir).join("plan.json");
    if !plan_file.exists() {
        return Err(json!({ "error": format!("plan.json not found at {}", plan_file.display()) }));
    }

    let plan = read_json(plan_file.to_str().unwrap_or(""));
    if plan.get("error").is_some() {
        return Err(json!({ "error": plan["error"] }));
    }

    let tasks = plan.get("tasks").and_then(|v| v.as_array());
    let task = match tasks.and_then(|arr| {
        arr.iter()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(task_id))
    }) {
        Some(t) => t,
        None => {
            return Err(json!({ "error": format!("Task {} not found in plan.json", task_id) }));
        }
    };

    let workspace_dir = match plan.get("workspace_dir").and_then(|v| v.as_str()) {
        Some(wd) if !wd.is_empty() => wd,
        _ => {
            return Err(json!({ "error": "plan.json missing or empty workspace_dir" }));
        }
    };
    let workspace_canonical = match std::fs::canonicalize(workspace_dir) {
        Ok(p) => p,
        Err(_) => {
            return Err(
                json!({ "error": format!("workspace_dir does not exist: {}", workspace_dir) }),
            );
        }
    };

    // Read context_mode from workspace config.json preferences.
    let config_file = workspace_canonical.join(".hoangsa").join("config.json");
    let context_mode = if config_file.exists() {
        let cfg = read_json(config_file.to_str().unwrap_or(""));
        cfg.get("preferences")
            .and_then(|p| p.get("context_mode"))
            .and_then(|v| v.as_str())
            .unwrap_or("full")
            .to_owned()
    } else {
        "full".to_owned()
    };
    let selective = context_mode == "selective";

    let mut file_segments: Vec<Value> = Vec::new();
    if let Some(files) = task.get("files").and_then(|v| v.as_array()) {
        for file_val in files {
            if let Some(file_spec) = file_val.as_str() {
                let (file_path, line_range) = parse_file_spec(file_spec);
                let resolved = if Path::new(file_path).is_absolute() {
                    match std::fs::canonicalize(file_path) {
                        Ok(p) => p,
                        Err(_) => {
                            let mut normalized = std::path::PathBuf::new();
                            for component in Path::new(file_path).components() {
                                normalized.push(component);
                            }
                            normalized
                        }
                    }
                } else {
                    match std::fs::canonicalize(workspace_canonical.join(file_path)) {
                        Ok(p) => p,
                        Err(_) => {
                            let mut normalized = std::path::PathBuf::new();
                            for component in workspace_canonical.join(file_path).components() {
                                normalized.push(component);
                            }
                            normalized
                        }
                    }
                };
                if !resolved.starts_with(&workspace_canonical) {
                    return Err(
                        json!({ "error": format!("Path traversal rejected: {} is outside workspace {}", file_path, workspace_dir) }),
                    );
                }

                let full_path = if Path::new(file_path).is_absolute() {
                    std::path::PathBuf::from(file_path)
                } else {
                    workspace_canonical.join(file_path)
                };
                let exists = full_path.exists();
                let action = if exists { "MODIFY" } else { "CREATE" };
                let (lines, start_line, end_line) = if exists {
                    match fs::read_to_string(&full_path) {
                        Ok(content) => {
                            if selective {
                                if let Some((start, end)) = line_range {
                                    let selected: Vec<&str> = content
                                        .lines()
                                        .skip(start.saturating_sub(1))
                                        .take(end.saturating_sub(start.saturating_sub(1)))
                                        .collect();
                                    let actual_end = start.saturating_sub(1) + selected.len();
                                    (selected.join("\n"), start, actual_end)
                                } else {
                                    let line_count = content.lines().count();
                                    (content, 1, line_count)
                                }
                            } else {
                                let line_count = content.lines().count();
                                (content, 1, line_count)
                            }
                        }
                        Err(_) => (String::new(), 1, 0),
                    }
                } else {
                    (String::new(), 1, 0)
                };
                file_segments.push(json!({
                    "path": file_path,
                    "action": action,
                    "lines": lines,
                    "start_line": start_line,
                    "end_line": end_line,
                }));
            }
        }
    }

    // Apply line-range parsing to context_pointers.
    let mut context_pointer_segments: Vec<Value> = Vec::new();
    if let Some(pointers) = task.get("context_pointers").and_then(|v| v.as_array()) {
        for ptr_val in pointers {
            if let Some(ptr_spec) = ptr_val.as_str() {
                let (file_path, line_range) = parse_file_spec(ptr_spec);
                let full_path = if Path::new(file_path).is_absolute() {
                    std::path::PathBuf::from(file_path)
                } else {
                    workspace_canonical.join(file_path)
                };
                if full_path.exists() {
                    if let Ok(content) = fs::read_to_string(&full_path) {
                        let (lines, start_line, end_line) = if selective {
                            if let Some((start, end)) = line_range {
                                let selected: Vec<&str> = content
                                    .lines()
                                    .skip(start.saturating_sub(1))
                                    .take(end.saturating_sub(start.saturating_sub(1)))
                                    .collect();
                                let actual_end = start.saturating_sub(1) + selected.len();
                                (selected.join("\n"), start, actual_end)
                            } else {
                                let line_count = content.lines().count();
                                (content, 1, line_count)
                            }
                        } else {
                            let line_count = content.lines().count();
                            (content, 1, line_count)
                        };
                        context_pointer_segments.push(json!({
                            "path": file_path,
                            "lines": lines,
                            "start_line": start_line,
                            "end_line": end_line,
                        }));
                    }
                }
            }
        }
    }

    let acceptance: Vec<Value> =
        if let Some(arr) = task.get("acceptance").and_then(|v| v.as_array()) {
            arr.clone()
        } else if let Some(s) = task.get("acceptance") {
            if s.is_null() { vec![] } else { vec![s.clone()] }
        } else {
            vec![]
        };

    let mut context_data = json!({
        "task_id": task_id,
        "task_name": task.get("name"),
        "description": task.get("name"),
        "acceptance": acceptance,
        "file_segments": file_segments,
        "context_pointer_segments": context_pointer_segments,
        "dependency_signatures": [],
        "estimated_tokens": 0,
    });

    let json_str = serde_json::to_string_pretty(&context_data).expect("valid json");
    let estimated_tokens = json_str.len().div_ceil(4);
    context_data["estimated_tokens"] = json!(estimated_tokens);

    Ok(context_data)
}

/// `context pack <sessionDir> <taskId>`
pub fn cmd_pack(session_dir: Option<&str>, task_id: Option<&str>) {
    let session_dir = match session_dir {
        Some(d) => d,
        None => {
            out(&json!({ "error": "sessionDir is required" }));
            return;
        }
    };
    let task_id = match task_id {
        Some(t) => t,
        None => {
            out(&json!({ "error": "taskId is required" }));
            return;
        }
    };

    let context_data = match build_context_pack(session_dir, task_id) {
        Ok(data) => data,
        Err(e) => {
            out(&e);
            return;
        }
    };

    let context_file = Path::new(session_dir).join(format!("task-{task_id}.context.json"));
    if let Err(e) = fs::create_dir_all(session_dir) {
        out(&json!({ "success": false, "error": e.to_string() }));
        return;
    }

    match fs::write(
        &context_file,
        serde_json::to_string_pretty(&context_data).expect("valid json"),
    ) {
        Ok(_) => out(&json!({
            "success": true,
            "path": context_file.to_string_lossy(),
            "context": context_data,
        })),
        Err(e) => out(&json!({ "success": false, "error": e.to_string() })),
    }
}

/// `context get <sessionDir> <taskId>` — auto-packs if context file doesn't exist yet.
pub fn cmd_get(session_dir: Option<&str>, task_id: Option<&str>) {
    let session_dir = match session_dir {
        Some(d) => d,
        None => {
            out(&json!({ "error": "sessionDir is required" }));
            return;
        }
    };
    let task_id = match task_id {
        Some(t) => t,
        None => {
            out(&json!({ "error": "taskId is required" }));
            return;
        }
    };

    let context_file = Path::new(session_dir).join(format!("task-{task_id}.context.json"));
    if context_file.exists() {
        let context = read_json(context_file.to_str().unwrap_or(""));
        if context.get("error").is_some() {
            out(&json!({ "error": context["error"] }));
            return;
        }
        out(&context);
        return;
    }

    // Auto-pack: build context on demand from plan.json
    match build_context_pack(session_dir, task_id) {
        Ok(context_data) => {
            let _ = fs::create_dir_all(session_dir);
            let _ = fs::write(
                &context_file,
                serde_json::to_string_pretty(&context_data).expect("valid json"),
            );
            out(&context_data);
        }
        Err(e) => out(&e),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_file_spec;

    #[test]
    fn test_parse_file_spec_with_range() {
        let (path, range) = parse_file_spec("src/cmd/pref.rs:56-71");
        assert_eq!(path, "src/cmd/pref.rs");
        assert_eq!(range, Some((56, 71)));
    }

    #[test]
    fn test_parse_file_spec_no_range() {
        let (path, range) = parse_file_spec("src/cmd/pref.rs");
        assert_eq!(path, "src/cmd/pref.rs");
        assert_eq!(range, None);
    }

    #[test]
    fn test_parse_file_spec_l_prefix_format() {
        // "path:L10-L50" — L prefix is not purely digits, so should not parse as range
        let (path, range) = parse_file_spec("src/main.rs:L10-L50");
        assert_eq!(path, "src/main.rs:L10-L50");
        assert_eq!(range, None);
    }

    #[test]
    fn test_parse_file_spec_windows_style_path() {
        // Colon in suffix that is not a valid range
        let (path, range) = parse_file_spec("path/to/file:notarange");
        assert_eq!(path, "path/to/file:notarange");
        assert_eq!(range, None);
    }

    #[test]
    fn test_parse_file_spec_single_line_range() {
        // start == end is valid (single line)
        let (path, range) = parse_file_spec("src/lib.rs:10-10");
        assert_eq!(path, "src/lib.rs");
        assert_eq!(range, Some((10, 10)));
    }

    #[test]
    fn test_parse_file_spec_invalid_range_reversed() {
        // start > end should not parse as range
        let (path, range) = parse_file_spec("src/lib.rs:50-10");
        assert_eq!(path, "src/lib.rs:50-10");
        assert_eq!(range, None);
    }

    #[test]
    fn test_parse_file_spec_zero_start() {
        // line numbers are 1-based; 0 is invalid
        let (path, range) = parse_file_spec("src/lib.rs:0-10");
        assert_eq!(path, "src/lib.rs:0-10");
        assert_eq!(range, None);
    }
}
