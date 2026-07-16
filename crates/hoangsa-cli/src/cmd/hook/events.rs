use crate::helpers::{out, read_json};
use serde_json::json;
use std::fs;
use std::path::Path;

use super::{chrono_now, enforcement_events_path, find_memory_bin, flag_value, is_source_file};

// ── PostToolUse State Recording ──────────────────────────────────────────────

/// `hook post-enforce`
///
/// PostToolUse hook that records enforcement events after hoangsa-memory tool calls.
/// Records: impact (with file resolution), detect_changes (with files), recall (with query).
/// Always outputs `{"decision":"approve"}` — never blocks.
pub fn cmd_post_enforce(cwd: &str) {
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let parsed: serde_json::Value = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    // Codex sanitizes the MCP server id's hyphen to an underscore
    // (`mcp__hoangsa_memory__…`) — canonicalize so the arms below match
    // under both harnesses.
    let tool_name = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .replace("mcp__hoangsa_memory__", "mcp__hoangsa-memory__");
    let tool_name = tool_name.as_str();
    let tool_input = parsed.get("tool_input").cloned().unwrap_or(json!({}));

    // lesson_saved: any tool whose name contains "remember_lesson" (catches
    // the MCP tool `mcp__hoangsa-memory__memory_remember_lesson`).
    if tool_name.contains("remember_lesson") {
        append_event(cwd, &json!({"event": "lesson_saved"}));
    }

    let event = match tool_name {
        "mcp__hoangsa-memory__memory_impact" => build_impact_event(cwd, &tool_input),
        "mcp__hoangsa-memory__memory_detect_changes" => build_detect_changes_event(&tool_input, &parsed),
        "mcp__hoangsa-memory__memory_recall" => build_recall_event(&tool_input),
        "Edit" | "Write" | "MultiEdit" => build_drift_event(cwd, &tool_input),
        _ => None,
    };

    if let Some(event) = event {
        append_event(cwd, &event);
    }

    out(&json!({"decision": "approve"}));
}

/// Rule #14 (experimental, v1): grep-based post-edit drift detection.
/// Extracts top-level symbol names from the diff (old_string vs new_string),
/// compares against impact-checked symbols for this file from the event log,
/// and emits a `drift_warn` event when the edited set isn't covered.
/// WARN-only — never blocks. False-positive rate tracked via `enforce report`.
fn build_drift_event(cwd: &str, tool_input: &serde_json::Value) -> Option<serde_json::Value> {
    let file_path = tool_input.get("file_path").and_then(|v| v.as_str())?;
    if !is_source_file(file_path) {
        return None;
    }
    let old_string = tool_input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
    let new_string = tool_input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
    let content = tool_input.get("content").and_then(|v| v.as_str()).unwrap_or("");

    // Collect symbols from the edit region (old ∪ new for Edit; content for Write).
    let mut edited: Vec<String> = Vec::new();
    for text in [old_string, new_string, content] {
        if text.is_empty() { continue; }
        extract_symbols(cwd, text, &mut edited);
    }
    edited.sort();
    edited.dedup();
    if edited.is_empty() {
        return None;
    }

    // Collect impact-checked symbols for this file from the event log.
    let events_path = enforcement_events_path(cwd);
    let events = fs::read_to_string(&events_path).unwrap_or_default();
    let mut impacted: Vec<String> = Vec::new();
    for line in events.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if entry.get("event").and_then(|e| e.as_str()) != Some("impact") {
            continue;
        }
        let matches_file = entry
            .get("file")
            .and_then(|f| f.as_str())
            .map(|f| f == file_path || file_path.ends_with(f) || f.ends_with(file_path))
            .unwrap_or(false);
        if !matches_file {
            continue;
        }
        if let Some(sym) = entry.get("symbol").and_then(|s| s.as_str()) {
            impacted.push(sym.to_string());
        }
    }

    // If no impact was recorded for this file, require-memory-impact already
    // surfaced that gap at PreToolUse — don't double-warn here.
    if impacted.is_empty() {
        return None;
    }
    // Event log replays duplicates; shrink before the pairwise substring scan
    impacted.sort();
    impacted.dedup();

    let uncovered: Vec<String> = edited
        .iter()
        .filter(|e| !impacted.iter().any(|i| i.contains(e.as_str()) || e.contains(i.as_str())))
        .cloned()
        .collect();

    if uncovered.is_empty() {
        None
    } else {
        Some(json!({
            "event": "drift_warn",
            "file": file_path,
            "edited_symbols": edited,
            "impact_symbols": impacted,
            "uncovered": uncovered,
        }))
    }
}

/// Default symbol-detection regexes, used when
/// `.hoangsa/config.json → enforcement.symbol_patterns` is absent.
/// Each pattern MUST have exactly one capture group for the symbol name.
/// Kept broad on purpose: false positives degrade only the drift-warn metric,
/// not correctness (drift is WARN-only per Decision #10 in the brainstorm).
const DEFAULT_SYMBOL_PATTERNS: &[&str] = &[
    r"(?m)\b(?:pub\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\b(?:pub\s+)?(?:struct|enum|trait|impl)\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bdef\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bfunction\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bfunc\s+([A-Za-z_][A-Za-z0-9_]*)",
    r"(?m)\bclass\s+([A-Za-z_][A-Za-z0-9_]*)",
];

/// Read symbol-detection regexes from `.hoangsa/config.json` under
/// `enforcement.symbol_patterns` (array of regex strings). Falls back to
/// `DEFAULT_SYMBOL_PATTERNS` when absent or malformed. Cached per project
/// root — one hook invocation resolves several symbols and must not re-read
/// + re-parse config.json each time.
fn read_symbol_patterns(cwd: &str) -> Vec<String> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>> =
        std::sync::OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Ok(map) = cache.lock()
        && let Some(cached) = map.get(cwd) {
            return cached.clone();
        }

    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    let patterns = if !config_path.exists() {
        DEFAULT_SYMBOL_PATTERNS.iter().map(|s| s.to_string()).collect()
    } else {
        let config = read_json(config_path.to_str().unwrap_or(""));
        let configured = config
            .get("enforcement")
            .and_then(|e| e.get("symbol_patterns"))
            .and_then(|p| p.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect::<Vec<_>>()
            });
        match configured {
            Some(patterns) if !patterns.is_empty() => patterns,
            _ => DEFAULT_SYMBOL_PATTERNS.iter().map(|s| s.to_string()).collect(),
        }
    };
    if let Ok(mut map) = cache.lock() {
        map.insert(cwd.to_string(), patterns.clone());
    }
    patterns
}

/// Extract plausible top-level symbol names from a source snippet.
/// Regexes come from `.hoangsa/config.json → enforcement.symbol_patterns`
/// (array of regex), falling back to `DEFAULT_SYMBOL_PATTERNS`. Best-effort;
/// grep-level, not AST. Each pattern must expose the symbol name in capture 1.
fn extract_symbols(cwd: &str, text: &str, out: &mut Vec<String>) {
    for pat in read_symbol_patterns(cwd) {
        if let Ok(re) = regex::Regex::new(&pat) {
            for cap in re.captures_iter(text) {
                if let Some(m) = cap.get(1) {
                    out.push(m.as_str().to_string());
                }
            }
        }
    }
}

fn build_impact_event(cwd: &str, tool_input: &serde_json::Value) -> Option<serde_json::Value> {
    let fqn = tool_input.get("fqn").and_then(|v| v.as_str())
        .or_else(|| tool_input.get("target").and_then(|v| v.as_str()))
        .unwrap_or("");
    if fqn.is_empty() {
        return None;
    }

    // FQN that already looks like a path → use as-is.
    let file = if fqn.contains('/') || (fqn.contains('.') && !fqn.contains("::")) {
        fqn.to_string()
    } else {
        resolve_symbol_to_file(cwd, fqn).unwrap_or_default()
    };

    Some(json!({
        "event": "impact",
        "symbol": fqn,
        "file": file,
    }))
}

fn build_detect_changes_event(tool_input: &serde_json::Value, full_payload: &serde_json::Value) -> Option<serde_json::Value> {
    // Try to extract files from tool_result (the actual output of detect_changes)
    let mut files: Vec<String> = Vec::new();

    // If diff was passed as input, extract file paths from it
    if let Some(diff) = tool_input.get("diff").and_then(|v| v.as_str()) {
        for line in diff.lines() {
            if let Some(path) = line.strip_prefix("+++ b/") {
                files.push(path.to_string());
            } else if let Some(path) = line.strip_prefix("--- a/")
                && path != "/dev/null" {
                    files.push(path.to_string());
                }
        }
    }

    // Also check tool_result for file mentions
    if files.is_empty()
        && let Some(result) = full_payload.get("tool_result").and_then(|v| v.as_str()) {
            // Parse result looking for file paths
            for line in result.lines() {
                let trimmed = line.trim();
                if trimmed.contains('/') && (trimmed.ends_with(".rs") || trimmed.ends_with(".ts") || trimmed.ends_with(".py")) {
                    // Rough extraction of paths
                    for word in trimmed.split_whitespace() {
                        let clean = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '.' && c != '-' && c != '_');
                        if clean.contains('/') && clean.len() > 3 {
                            files.push(clean.to_string());
                        }
                    }
                }
            }
        }

    files.sort();
    files.dedup();

    Some(json!({
        "event": "detect_changes",
        "files": files,
    }))
}

fn build_recall_event(tool_input: &serde_json::Value) -> Option<serde_json::Value> {
    let query = tool_input.get("query").and_then(|v| v.as_str()).unwrap_or("");
    if query.is_empty() {
        return None;
    }
    Some(json!({
        "event": "recall",
        "query": query,
    }))
}

/// Resolve a symbol (FQN or bare name) to a source file path.
///
/// 1. Ask the hoangsa-memory CLI for the symbol's canonical location (uses the code graph).
/// 2. On miss or when hoangsa-memory is unavailable, fall back to a config-driven grep
///    built from `enforcement.symbol_patterns` (same source as extract_symbols).
///
/// Both paths scan from `cwd` — no more hardcoded `cli/src/` / `src/`.
fn resolve_symbol_to_file(cwd: &str, symbol: &str) -> Option<String> {
    use std::process::Command;

    // Strip module prefix: "rule::cmd_rule_add" → "cmd_rule_add".
    let bare = symbol.rsplit("::").next().unwrap_or(symbol);

    // Preferred: hoangsa-memory index lookup.
    if let Some(memory_bin) = find_memory_bin() {
        let memory_root = Path::new(cwd).join(".hoangsa").join("memory");
        if memory_root.exists()
            && let Ok(out) = Command::new(&memory_bin)
                .args(["--root", &memory_root.to_string_lossy()])
                .args(["context", bare, "--json"])
                .current_dir(cwd)
                .output()
                && let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) {
                    if let Some(path) = v.get("symbol").and_then(|s| s.get("path")).and_then(|p| p.as_str()) {
                        return Some(path.to_string());
                    }
                    if let Some(path) = v.get("path").and_then(|p| p.as_str()) {
                        return Some(path.to_string());
                    }
                }
    }

    // Fallback: in-process regex walk using the configured symbol patterns.
    // Portable across platforms (BSD grep lacks PCRE). Only runs when hoangsa-memory
    // can't resolve — bounded by depth + source-extension filter.
    let patterns = read_symbol_patterns(cwd);
    let escaped = regex::escape(bare);
    let compiled: Vec<regex::Regex> = patterns
        .iter()
        .map(|pat| pat.replacen("([A-Za-z_][A-Za-z0-9_]*)", &escaped, 1))
        .filter(|p| p.contains(&escaped))
        .filter_map(|p| regex::Regex::new(&p).ok())
        .collect();
    if compiled.is_empty() {
        return None;
    }
    let mut file_budget: u32 = 2000;
    find_symbol_in_tree(cwd, Path::new(cwd), &compiled, 0, &mut file_budget)
}

/// Recursive DFS over source files looking for any pattern match.
/// Skips vendor/build dirs and binary extensions. Returns the first match.
/// `file_budget` bounds the worst case (symbol not in tree) — a miss must
/// not cost a full scan of a huge repo on every hook invocation.
fn find_symbol_in_tree(
    cwd: &str,
    dir: &Path,
    patterns: &[regex::Regex],
    depth: u32,
    file_budget: &mut u32,
) -> Option<String> {
    if depth > 8 || *file_budget == 0 {
        return None;
    }
    const SKIP_DIRS: &[&str] = &[
        ".git", "node_modules", "target", "dist", "build", ".hoangsa",
        ".claude", "__pycache__", ".venv", "venv", ".next",
    ];
    const SOURCE_EXTS: &[&str] = &[
        "rs", "ts", "tsx", "js", "jsx", "py", "go", "java", "c", "cpp",
        "h", "hpp", "rb", "swift", "kt", "scala", "cs", "php", "lua", "ex",
    ];

    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue; // hidden entries are never resolvable source
        }
        let ft = match entry.file_type() { Ok(t) => t, Err(_) => continue };
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if let Some(found) = find_symbol_in_tree(cwd, &path, patterns, depth + 1, file_budget) {
                return Some(found);
            }
        } else if ft.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !SOURCE_EXTS.contains(&ext) {
                continue;
            }
            if *file_budget == 0 {
                return None;
            }
            *file_budget -= 1;
            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            for re in patterns {
                if re.is_match(&content) {
                    return Some(
                        path.strip_prefix(cwd)
                            .ok()
                            .map(|p| p.to_string_lossy().to_string())
                            .unwrap_or_else(|| path.to_string_lossy().to_string()),
                    );
                }
            }
        }
    }
    None
}

pub(super) fn append_event(cwd: &str, event: &serde_json::Value) {
    // Don't materialise `.hoangsa/state/` in projects that haven't been
    // initialised — otherwise running Claude (or any hook-fired Bash) in
    // a non-hoangsa directory leaves a stray `.hoangsa/` behind. The
    // walk-up in `resolve_cwd` makes this the project root when init'd,
    // so this check both gates the no-init case and prevents stray dirs
    // in subfolders.
    if !is_hoangsa_project(cwd) {
        return;
    }
    let events_path = enforcement_events_path(cwd);
    if let Some(parent) = events_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut enriched = event.clone();
    if enriched.get("ts").is_none() {
        let ts = chrono_now();
        enriched.as_object_mut().map(|o| o.insert("ts".to_string(), json!(ts)));
    }

    let mut line = serde_json::to_string(&enriched).unwrap_or_default();
    line.push('\n');

    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .and_then(|mut f| {
            use std::io::Write as _;
            f.write_all(line.as_bytes())
        });
}

// ── Enforce Override + Report ────────────────────────────────────────────────

/// `enforce override --rule <id> --target <path> --reason <text>`
///
/// Records a scoped override event. The enforce hook checks for these
/// before blocking, allowing bypass of false positives.
pub fn cmd_enforce_override(cwd: &str, args: &[&str]) {
    let rule = flag_value(args, "--rule").unwrap_or("");
    let target = flag_value(args, "--target").unwrap_or("");
    let reason = flag_value(args, "--reason").unwrap_or("");

    if rule.is_empty() || target.is_empty() {
        out(&json!({"success": false, "error": "Required: --rule <id> --target <path> --reason <text>"}));
        return;
    }
    if reason.is_empty() {
        out(&json!({"success": false, "error": "--reason is required (explains why override is safe)"}));
        return;
    }

    let event = json!({
        "event": "override",
        "rule": rule,
        "target": target,
        "reason": reason,
    });

    append_event(cwd, &event);
    out(&json!({"success": true, "rule": rule, "target": target, "reason": reason}));
}

/// `enforce report`
///
/// Aggregates enforcement events into a human-readable summary.
pub fn cmd_enforce_report(cwd: &str) {
    let events_path = enforcement_events_path(cwd);
    let content = fs::read_to_string(&events_path).unwrap_or_default();

    if content.is_empty() {
        out(&json!({"report": "No enforcement events recorded this session."}));
        return;
    }

    let mut blocks: Vec<(String, String)> = Vec::new();
    let mut warns: Vec<(String, String)> = Vec::new();
    let mut overrides: Vec<(String, String, String)> = Vec::new();
    let mut drifts: Vec<(String, Vec<String>)> = Vec::new();
    let mut impacts = 0u32;
    let mut detect_changes = 0u32;
    let mut recalls = 0u32;
    let mut total_events = 0usize;

    for line in content.lines() {
        total_events += 1;
        if let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) {
            let event_type = entry.get("event").and_then(|e| e.as_str()).unwrap_or("");
            match event_type {
                "block" => {
                    let rule = entry.get("rule").and_then(|r| r.as_str()).unwrap_or("?").to_string();
                    let target = entry.get("target").and_then(|t| t.as_str()).unwrap_or("?").to_string();
                    blocks.push((rule, target));
                }
                "warn" => {
                    let rule = entry.get("rule").and_then(|r| r.as_str()).unwrap_or("?").to_string();
                    let target = entry.get("target").and_then(|t| t.as_str()).unwrap_or("?").to_string();
                    warns.push((rule, target));
                }
                "override" => {
                    let rule = entry.get("rule").and_then(|r| r.as_str()).unwrap_or("?").to_string();
                    let target = entry.get("target").and_then(|t| t.as_str()).unwrap_or("?").to_string();
                    let reason = entry.get("reason").and_then(|r| r.as_str()).unwrap_or("").to_string();
                    overrides.push((rule, target, reason));
                }
                "drift_warn" => {
                    let file = entry.get("file").and_then(|f| f.as_str()).unwrap_or("?").to_string();
                    let uncovered: Vec<String> = entry
                        .get("uncovered")
                        .and_then(|v| v.as_array())
                        .map(|arr| arr.iter().filter_map(|s| s.as_str().map(|s| s.to_string())).collect())
                        .unwrap_or_default();
                    drifts.push((file, uncovered));
                }
                "impact" => impacts += 1,
                "detect_changes" => detect_changes += 1,
                "recall" => recalls += 1,
                _ => {}
            }
        }
    }

    let fp_risk = if blocks.is_empty() {
        0.0
    } else {
        overrides.len() as f64 / blocks.len() as f64
    };

    let report = json!({
        "total_events": total_events,
        "blocks": blocks.len(),
        "warns": warns.len(),
        "overrides": overrides.len(),
        "drifts": drifts.len(),
        "impacts": impacts,
        "detect_changes": detect_changes,
        "recalls": recalls,
        "fp_risk": format!("{:.2}", fp_risk),
        "top_blocks": blocks,
        "top_warns": warns,
        "override_details": overrides,
        "drift_details": drifts,
    });

    out(&report);
}

/// True when `<cwd>/.hoangsa/config.json` exists — our marker that the
/// project has been through `/hoangsa:init` and is opting into hoangsa
/// state writes.
fn is_hoangsa_project(cwd: &str) -> bool {
    Path::new(cwd)
        .join(".hoangsa")
        .join("config.json")
        .is_file()
}
