use crate::helpers::out;
use serde_json::{json, Value};
use std::fs;
use std::io::BufRead;
use std::path::{Path, PathBuf};

struct Pricing {
    input: f64,
    cache_write: f64,
    cache_read: f64,
    output: f64,
}

fn get_pricing(model_id: &str) -> Pricing {
    let norm = model_id.to_lowercase().replace(['-', '_'], "");
    if norm.contains("claudeopus4") && !norm.contains("claudeopus41") {
        Pricing { input: 5.0, cache_write: 6.25, cache_read: 0.50, output: 25.0 }
    } else if norm.contains("claudeopus41") || norm.contains("claudeopus3") {
        Pricing { input: 15.0, cache_write: 18.75, cache_read: 1.50, output: 75.0 }
    } else if norm.contains("claudesonnet") {
        Pricing { input: 3.0, cache_write: 3.75, cache_read: 0.30, output: 15.0 }
    } else if norm.contains("claudehaiku45") {
        Pricing { input: 1.0, cache_write: 1.25, cache_read: 0.10, output: 5.0 }
    } else if norm.contains("claudehaiku3") {
        Pricing { input: 0.25, cache_write: 0.30, cache_read: 0.03, output: 1.25 }
    } else {
        Pricing { input: 3.0, cache_write: 3.75, cache_read: 0.30, output: 15.0 }
    }
}

struct TurnUsage {
    model: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_creation_tokens: u64,
    cache_read_tokens: u64,
    timestamp: Option<String>,
}

impl TurnUsage {
    fn cacheable_tokens(&self) -> u64 {
        self.cache_creation_tokens + self.cache_read_tokens
    }

    fn hit_rate(&self) -> f64 {
        let cacheable = self.cacheable_tokens();
        if cacheable == 0 { return 0.0; }
        self.cache_read_tokens as f64 / cacheable as f64
    }
}

struct TurnMetrics {
    turn: TurnUsage,
    actual_cost: f64,
    cost_no_cache: f64,
    cache_write_overhead: f64,
}

impl TurnMetrics {
    fn savings(&self) -> f64 {
        self.cost_no_cache - self.actual_cost
    }
}

fn compute_turn_metrics(turn: TurnUsage) -> TurnMetrics {
    let p = get_pricing(&turn.model);
    let ppt_input = p.input / 1_000_000.0;
    let ppt_write = p.cache_write / 1_000_000.0;
    let ppt_read = p.cache_read / 1_000_000.0;
    let ppt_output = p.output / 1_000_000.0;

    let actual_cost = turn.cache_creation_tokens as f64 * ppt_write
        + turn.cache_read_tokens as f64 * ppt_read
        + turn.input_tokens as f64 * ppt_input
        + turn.output_tokens as f64 * ppt_output;

    let cost_no_cache = (turn.cacheable_tokens() + turn.input_tokens) as f64 * ppt_input
        + turn.output_tokens as f64 * ppt_output;

    let cache_write_overhead = turn.cache_creation_tokens as f64 * (ppt_write - ppt_input);

    TurnMetrics { turn, actual_cost, cost_no_cache, cache_write_overhead }
}

struct SessionResult {
    session_id: String,
    project: String,
    model: String,
    started_at: Option<String>,
    num_turns: usize,
    total_input: u64,
    total_output: u64,
    total_cache_creation: u64,
    total_cache_read: u64,
    hit_rate: f64,
    efficiency_score: f64,
    grade: String,
    actual_cost: f64,
    cost_no_cache: f64,
    savings: f64,
    net_savings: f64,
    savings_pct: f64,
    turns: Vec<TurnMetrics>,
}

fn grade_from_score(score: f64) -> &'static str {
    if score >= 0.70 { "A" }
    else if score >= 0.50 { "B" }
    else if score >= 0.30 { "C" }
    else if score >= 0.10 { "D" }
    else { "F" }
}

fn parse_session(path: &Path) -> Option<Vec<TurnUsage>> {
    let file = fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    let mut turns = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.is_empty() { continue; }
        let event: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if event.get("type").and_then(|v| v.as_str()) != Some("assistant") {
            continue;
        }
        let message = match event.get("message") {
            Some(m) if m.is_object() => m,
            _ => continue,
        };
        let usage = match message.get("usage") {
            Some(u) if u.is_object() => u,
            _ => continue,
        };

        let timestamp = event.get("timestamp").and_then(|v| v.as_str()).map(|s| s.to_string());
        let model = message.get("model").and_then(|v| v.as_str()).unwrap_or("unknown").to_string();

        turns.push(TurnUsage {
            model,
            input_tokens: usage.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            output_tokens: usage.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cache_creation_tokens: usage.get("cache_creation_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            cache_read_tokens: usage.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            timestamp,
        });
    }

    if turns.is_empty() { None } else { Some(turns) }
}

fn analyze_session(path: &Path) -> Option<SessionResult> {
    let turns = parse_session(path)?;
    let session_id = path.file_stem()?.to_str()?.to_string();
    let project = path.parent()?.file_name()?.to_str()?.to_string();
    let model = turns.last().map(|t| t.model.clone()).unwrap_or_default();
    let started_at = turns.iter().filter_map(|t| t.timestamp.clone()).min();
    let num_turns = turns.len();

    let total_input: u64 = turns.iter().map(|t| t.input_tokens).sum();
    let total_output: u64 = turns.iter().map(|t| t.output_tokens).sum();
    let total_cache_creation: u64 = turns.iter().map(|t| t.cache_creation_tokens).sum();
    let total_cache_read: u64 = turns.iter().map(|t| t.cache_read_tokens).sum();
    let total_cacheable = total_cache_creation + total_cache_read;

    let hit_rate = if total_cacheable == 0 { 0.0 } else { total_cache_read as f64 / total_cacheable as f64 };

    let metrics: Vec<TurnMetrics> = turns.into_iter().map(compute_turn_metrics).collect();

    let actual_cost: f64 = metrics.iter().map(|m| m.actual_cost).sum();
    let cost_no_cache: f64 = metrics.iter().map(|m| m.cost_no_cache).sum();
    let cache_write_overhead: f64 = metrics.iter().map(|m| m.cache_write_overhead).sum();
    let savings = cost_no_cache - actual_cost;
    let net_savings = savings - cache_write_overhead;
    let savings_pct = if cost_no_cache == 0.0 { 0.0 } else { savings / cost_no_cache * 100.0 };

    let total_all_input = total_input + total_cacheable;
    let denom = total_all_input + total_cacheable;
    let efficiency_score = if denom == 0 { 0.0 } else { hit_rate * (total_cacheable as f64 / denom as f64) };
    let grade = grade_from_score(efficiency_score).to_string();

    Some(SessionResult {
        session_id, project, model, started_at, num_turns,
        total_input, total_output, total_cache_creation, total_cache_read,
        hit_rate, efficiency_score, grade, actual_cost, cost_no_cache,
        savings, net_savings, savings_pct, turns: metrics,
    })
}

fn discover_sessions(root: &Path) -> Vec<PathBuf> {
    let mut results = Vec::new();
    let projects_dir = root.join("projects");
    if projects_dir.is_dir() {
        collect_jsonl(&projects_dir, &mut results);
    } else if root.is_dir() {
        collect_jsonl(root, &mut results);
    }
    results.sort_by(|a, b| {
        let ma = fs::metadata(a).and_then(|m| m.modified()).ok();
        let mb = fs::metadata(b).and_then(|m| m.modified()).ok();
        mb.cmp(&ma)
    });
    results
}

fn collect_jsonl(dir: &Path, results: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_jsonl(&path, results);
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            results.push(path);
        }
    }
}

/// Resolve the Claude project directory for the current cwd.
/// Claude stores sessions at `~/.claude/projects/-<cwd-with-dashes>/`
fn resolve_project_dir(cwd: &str) -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    let slug = cwd.replace('/', "-");
    PathBuf::from(format!("{home}/.claude/projects/{slug}"))
}

pub fn cmd_cache(args: &[&str], cwd: &str) {
    let mut top_n: usize = 10;
    let mut session_filter: Option<&str> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "-n" | "--top" => {
                if let Some(v) = args.get(i + 1) {
                    top_n = v.parse().unwrap_or(10);
                    i += 1;
                }
            }
            "-s" | "--session" => {
                session_filter = args.get(i + 1).copied();
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }

    let project_dir = resolve_project_dir(cwd);
    if !project_dir.is_dir() {
        out(&json!({"error": "No Claude sessions found for this project", "path": project_dir.display().to_string()}));
        return;
    }

    let jsonl_files = discover_sessions(&project_dir);
    if jsonl_files.is_empty() {
        out(&json!({"error": "No JSONL session files found", "path": project_dir.display().to_string()}));
        return;
    }

    let mut sessions: Vec<SessionResult> = jsonl_files
        .iter()
        .filter_map(|p| analyze_session(p))
        .collect();

    if let Some(sf) = session_filter {
        let matches: Vec<SessionResult> = sessions
            .into_iter()
            .filter(|s| s.session_id.starts_with(sf) || s.session_id == sf)
            .collect();

        if matches.is_empty() {
            out(&json!({"error": format!("No session matching '{sf}'")}));
            return;
        }
        if matches.len() > 1 {
            let ids: Vec<&str> = matches.iter().map(|s| s.session_id.as_str()).collect();
            out(&json!({"error": "Ambiguous session ID", "candidates": ids}));
            return;
        }

        let s = &matches[0];
        let turn_data: Vec<Value> = s.turns.iter().map(|tm| {
            json!({
                "model": tm.turn.model,
                "input_tokens": tm.turn.input_tokens,
                "output_tokens": tm.turn.output_tokens,
                "cache_creation_tokens": tm.turn.cache_creation_tokens,
                "cache_read_tokens": tm.turn.cache_read_tokens,
                "hit_rate": (tm.turn.hit_rate() * 1000.0).round() / 1000.0,
                "actual_cost": (tm.actual_cost * 10000.0).round() / 10000.0,
                "cost_no_cache": (tm.cost_no_cache * 10000.0).round() / 10000.0,
                "savings": (tm.savings() * 10000.0).round() / 10000.0,
            })
        }).collect();

        out(&json!({
            "session_id": s.session_id,
            "project": s.project,
            "model": s.model,
            "started_at": s.started_at,
            "num_turns": s.num_turns,
            "total_input_tokens": s.total_input,
            "total_output_tokens": s.total_output,
            "total_cache_creation_tokens": s.total_cache_creation,
            "total_cache_read_tokens": s.total_cache_read,
            "hit_rate": (s.hit_rate * 1000.0).round() / 1000.0,
            "efficiency_score": (s.efficiency_score * 1000.0).round() / 1000.0,
            "grade": s.grade,
            "actual_cost": (s.actual_cost * 10000.0).round() / 10000.0,
            "cost_no_cache": (s.cost_no_cache * 10000.0).round() / 10000.0,
            "savings": (s.savings * 10000.0).round() / 10000.0,
            "net_savings": (s.net_savings * 10000.0).round() / 10000.0,
            "savings_pct": (s.savings_pct * 10.0).round() / 10.0,
            "turns": turn_data,
        }));
        return;
    }

    sessions.truncate(top_n);

    if sessions.is_empty() {
        out(&json!({"error": "No sessions found"}));
        return;
    }

    let total_actual: f64 = sessions.iter().map(|s| s.actual_cost).sum();
    let total_no_cache: f64 = sessions.iter().map(|s| s.cost_no_cache).sum();
    let total_savings = total_no_cache - total_actual;
    let total_overhead: f64 = sessions.iter().map(|s| s.savings - s.net_savings).sum();
    let total_net = total_savings - total_overhead;
    let avg_hit = sessions.iter().map(|s| s.hit_rate).sum::<f64>() / sessions.len() as f64;
    let savings_pct = if total_no_cache == 0.0 { 0.0 } else { total_savings / total_no_cache * 100.0 };

    let session_list: Vec<Value> = sessions.iter().map(|s| {
        json!({
            "session_id": s.session_id,
            "model": s.model,
            "started_at": s.started_at,
            "num_turns": s.num_turns,
            "hit_rate": (s.hit_rate * 1000.0).round() / 1000.0,
            "grade": s.grade,
            "actual_cost": (s.actual_cost * 10000.0).round() / 10000.0,
            "savings": (s.savings * 10000.0).round() / 10000.0,
        })
    }).collect();

    out(&json!({
        "project": project_dir.display().to_string(),
        "summary": {
            "sessions_analyzed": sessions.len(),
            "total_actual_cost": (total_actual * 10000.0).round() / 10000.0,
            "total_cost_no_cache": (total_no_cache * 10000.0).round() / 10000.0,
            "total_savings": (total_savings * 10000.0).round() / 10000.0,
            "total_net_savings": (total_net * 10000.0).round() / 10000.0,
            "savings_pct": (savings_pct * 10.0).round() / 10.0,
            "avg_hit_rate": (avg_hit * 1000.0).round() / 1000.0,
        },
        "sessions": session_list,
    }));
}
