use crate::helpers::{out, read_json};
use serde_json::{Value, json};
use std::fs;
use std::path::Path;

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

    let plan_file = Path::new(session_dir).join("plan.json");
    if !plan_file.exists() {
        out(&json!({ "error": format!("plan.json not found at {}", plan_file.display()) }));
        return;
    }

    let plan = read_json(plan_file.to_str().unwrap_or(""));
    if plan.get("error").is_some() {
        out(&json!({ "error": plan["error"] }));
        return;
    }

    let tasks = plan.get("tasks").and_then(|v| v.as_array());
    let task = tasks.and_then(|arr| {
        arr.iter()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(task_id))
    });

    let task = match task {
        Some(t) => t,
        None => {
            out(&json!({ "error": format!("Task {} not found in plan.json", task_id) }));
            return;
        }
    };

    // Build file_segments (with path traversal guard — SEC-003 fix)
    let workspace_dir = plan
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let workspace_canonical = match std::fs::canonicalize(workspace_dir) {
        Ok(p) => p,
        Err(_) => {
            out(&json!({ "error": format!("workspace_dir does not exist: {}", workspace_dir) }));
            return;
        }
    };

    let mut file_segments: Vec<Value> = Vec::new();
    if let Some(files) = task.get("files").and_then(|v| v.as_array()) {
        for file_val in files {
            if let Some(file_path) = file_val.as_str() {
                // Reject paths outside workspace_dir
                let resolved = if Path::new(file_path).is_absolute() {
                    match std::fs::canonicalize(file_path) {
                        Ok(p) => p,
                        Err(_) => {
                            // File doesn't exist yet (CREATE action) — normalize manually
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
                            // File doesn't exist yet — normalize to strip .. components
                            let mut normalized = std::path::PathBuf::new();
                            for component in workspace_canonical.join(file_path).components() {
                                normalized.push(component);
                            }
                            normalized
                        }
                    }
                };
                if !resolved.starts_with(&workspace_canonical) {
                    out(
                        &json!({ "error": format!("Path traversal rejected: {} is outside workspace {}", file_path, workspace_dir) }),
                    );
                    return;
                }

                let full_path = if Path::new(file_path).is_absolute() {
                    std::path::PathBuf::from(file_path)
                } else {
                    workspace_canonical.join(file_path)
                };
                let exists = full_path.exists();
                let action = if exists { "MODIFY" } else { "CREATE" };
                let (lines, end_line) = if exists {
                    match fs::read_to_string(&full_path) {
                        Ok(content) => {
                            let line_count = content.lines().count();
                            (content, line_count)
                        }
                        Err(_) => (String::new(), 0),
                    }
                } else {
                    (String::new(), 0)
                };
                file_segments.push(json!({
                    "path": file_path,
                    "action": action,
                    "lines": lines,
                    "start_line": 1,
                    "end_line": end_line,
                }));
            }
        }
    }

    // Normalise acceptance to array
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
        "dependency_signatures": [],
        "estimated_tokens": 0,
    });

    // Estimate tokens: ~4 chars per token
    let json_str = serde_json::to_string_pretty(&context_data).unwrap();
    let estimated_tokens = json_str.len().div_ceil(4); // ceil division
    context_data["estimated_tokens"] = json!(estimated_tokens);

    let context_file = Path::new(session_dir).join(format!("task-{task_id}.context.json"));
    if let Err(e) = fs::create_dir_all(session_dir) {
        out(&json!({ "success": false, "error": e.to_string() }));
        return;
    }

    // Re-serialize with updated estimated_tokens
    match fs::write(
        &context_file,
        serde_json::to_string_pretty(&context_data).unwrap(),
    ) {
        Ok(_) => out(&json!({
            "success": true,
            "path": context_file.to_string_lossy(),
            "context": context_data,
        })),
        Err(e) => out(&json!({ "success": false, "error": e.to_string() })),
    }
}

/// `context get <sessionDir> <taskId>`
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
    if !context_file.exists() {
        out(&json!({ "error": format!("Context file not found: {}. Run `context pack {} {}` first.", context_file.display(), session_dir, task_id) }));
        return;
    }
    let context = read_json(context_file.to_str().unwrap_or(""));
    if context.get("error").is_some() {
        out(&json!({ "error": context["error"] }));
        return;
    }
    out(&context);
}
