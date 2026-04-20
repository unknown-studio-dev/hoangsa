use crate::helpers::{out, read_json};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Detect cycles in the task DAG using DFS.
pub fn detect_cycles(tasks: &[Value]) -> Vec<String> {
    let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for t in tasks {
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let deps: Vec<&str> = t
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();
        adj.insert(id, deps);
    }

    let mut visited: HashSet<&str> = HashSet::new();
    let mut in_stack: HashSet<&str> = HashSet::new();
    let mut cycles: Vec<String> = Vec::new();

    fn dfs<'a>(
        id: &'a str,
        adj: &HashMap<&'a str, Vec<&'a str>>,
        visited: &mut HashSet<&'a str>,
        in_stack: &mut HashSet<&'a str>,
        cycles: &mut Vec<String>,
    ) {
        visited.insert(id);
        in_stack.insert(id);
        if let Some(deps) = adj.get(id) {
            for dep in deps {
                if !visited.contains(dep) {
                    dfs(dep, adj, visited, in_stack, cycles);
                } else if in_stack.contains(dep) {
                    cycles.push(format!("{id} → {dep}"));
                }
            }
        }
        in_stack.remove(id);
    }

    for t in tasks {
        let id = t.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        if !visited.contains(id) {
            dfs(id, &adj, &mut visited, &mut in_stack, &mut cycles);
        }
    }
    cycles
}

/// Detect dangling dependencies (referencing non-existent tasks).
pub fn detect_dangling(tasks: &[Value]) -> Vec<String> {
    let ids: HashSet<&str> = tasks
        .iter()
        .filter_map(|t| t.get("id").and_then(|v| v.as_str()))
        .collect();
    let mut dangling = Vec::new();
    for t in tasks {
        let tid = t.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        if let Some(deps) = t.get("depends_on").and_then(|v| v.as_array()) {
            for dep in deps {
                if let Some(dep_str) = dep.as_str() {
                    if !ids.contains(dep_str) {
                        dangling.push(format!("{tid} depends_on unknown: {dep_str}"));
                    }
                }
            }
        }
    }
    dangling
}

/// Compute execution waves from task DAG.
pub fn compute_waves(tasks: &[Value]) -> Vec<Value> {
    let mut completed: HashSet<String> = HashSet::new();
    let mut waves: Vec<Value> = Vec::new();
    let mut remaining: Vec<&Value> = tasks.iter().collect();

    while !remaining.is_empty() {
        let wave: Vec<&Value> = remaining
            .iter()
            .filter(|t| {
                t.get("depends_on")
                    .and_then(|v| v.as_array())
                    .map(|deps| {
                        deps.iter()
                            .all(|d| d.as_str().map(|s| completed.contains(s)).unwrap_or(true))
                    })
                    .unwrap_or(true)
            })
            .copied()
            .collect();

        if wave.is_empty() {
            let blocked: Vec<Value> = remaining
                .iter()
                .filter_map(|t| t.get("id").cloned())
                .collect();
            waves.push(json!({ "blocked": blocked }));
            break;
        }

        let wave_json: Vec<Value> = wave
            .iter()
            .map(|t| {
                json!({
                    "id": t.get("id"),
                    "name": t.get("name"),
                    "complexity": t.get("complexity"),
                    "budget_tokens": t.get("budget_tokens"),
                })
            })
            .collect();

        for t in &wave {
            if let Some(id) = t.get("id").and_then(|v| v.as_str()) {
                completed.insert(id.to_string());
            }
        }

        remaining.retain(|t| {
            t.get("id")
                .and_then(|v| v.as_str())
                .map(|id| !completed.contains(id))
                .unwrap_or(true)
        });

        waves.push(Value::Array(wave_json));
    }
    waves
}

/// `dag check <plan_path>` — detect cycles and dangling deps.
pub fn cmd_check(file_path: &str) {
    if !Path::new(file_path).exists() {
        out(&json!({ "valid": false, "errors": [format!("Plan file not found: {}", file_path)] }));
        return;
    }
    let plan = read_json(file_path);
    if plan.get("error").is_some() {
        out(&json!({ "valid": false, "errors": [plan["error"]] }));
        return;
    }
    let tasks = plan
        .get("tasks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let cycles = detect_cycles(&tasks);
    let dangling = detect_dangling(&tasks);
    out(&json!({
        "valid": cycles.is_empty() && dangling.is_empty(),
        "cycles": cycles,
        "dangling": dangling,
    }));
}

/// `dag waves <plan_path>` — compute execution waves.
pub fn cmd_waves(file_path: &str) {
    if !Path::new(file_path).exists() {
        out(&json!({ "error": format!("Plan file not found: {}", file_path) }));
        return;
    }
    let plan = read_json(file_path);
    if plan.get("error").is_some() {
        out(&json!({ "error": plan["error"] }));
        return;
    }
    let tasks = plan
        .get("tasks")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let waves = compute_waves(&tasks);
    let wave_count = waves.len();
    out(&json!({ "waves": waves, "wave_count": wave_count }));
}
