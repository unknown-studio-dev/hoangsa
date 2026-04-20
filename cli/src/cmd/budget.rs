use crate::cmd::stats::{CalibrationFactors, load_calibration};
use crate::helpers::{count_tokens, out, read_file, read_json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

// ─── Types ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
pub struct BudgetBreakdown {
    pub work_tokens: u64,
    pub system_prompt_tokens: u64,
    pub system_prompt_effective: u64,
    pub context_pack_tokens: u64,
    pub tool_overhead_tokens: u64,
    pub safety_margin_tokens: u64,
    pub total: u64,
    pub cache_scenario: CacheScenario,
    pub calibration_applied: bool,
    pub calibration_factor: f64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheScenario {
    Cold,
    Warm,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OverheadConstants {
    pub base_rules_tokens: u64,
    pub addon_tokens_per_addon: u64,
    pub tool_def_tokens_per_tool: u64,
    pub task_envelope_tokens: u64,
    pub tool_call_tokens_per_call: u64,
    pub cache_warm_factor: f64,
    pub safety_margin_pct: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ComplexityProfile {
    pub work_tokens_min: u64,
    pub work_tokens_max: u64,
    pub expected_tool_calls_min: u64,
    pub expected_tool_calls_max: u64,
}

// ─── Constants ───────────────────────────────────────────────────────────────

const DEFAULT_OVERHEAD: OverheadConstants = OverheadConstants {
    base_rules_tokens: 2500,
    addon_tokens_per_addon: 300,
    tool_def_tokens_per_tool: 150,
    task_envelope_tokens: 500,
    tool_call_tokens_per_call: 800,
    cache_warm_factor: 0.1,
    safety_margin_pct: 0.15,
};

// ─── Complexity profiles ──────────────────────────────────────────────────────

fn complexity_profile(complexity: &str) -> ComplexityProfile {
    match complexity {
        "low" => ComplexityProfile {
            work_tokens_min: 8000,
            work_tokens_max: 15000,
            expected_tool_calls_min: 5,
            expected_tool_calls_max: 10,
        },
        "medium" => ComplexityProfile {
            work_tokens_min: 15000,
            work_tokens_max: 30000,
            expected_tool_calls_min: 15,
            expected_tool_calls_max: 25,
        },
        "high" | _ => ComplexityProfile {
            work_tokens_min: 30000,
            work_tokens_max: 45000,
            expected_tool_calls_min: 30,
            expected_tool_calls_max: 50,
        },
    }
}

// ─── System prompt estimation ─────────────────────────────────────────────────

/// Measure the actual token count of system prompt components.
/// Checks local `.claude/hoangsa/` first, then `~/.claude/hoangsa/`.
fn estimate_system_prompt_tokens(cwd: &str) -> u64 {
    let home_dir = std::env::var("HOME").unwrap_or_default();

    // Resolve worker-rules/base.md
    let local_base = Path::new(cwd).join(".claude/hoangsa/worker-rules/base.md");
    let global_base = Path::new(&home_dir).join(".claude/hoangsa/worker-rules/base.md");

    let base_tokens = if let Some(content) = read_file(local_base.to_str().unwrap_or(""))
        .or_else(|| read_file(global_base.to_str().unwrap_or("")))
    {
        count_tokens(&content)
    } else {
        DEFAULT_OVERHEAD.base_rules_tokens
    };

    // Load active addons from config
    let local_config = Path::new(cwd).join(".hoangsa/config.json");
    let config = read_json(local_config.to_str().unwrap_or(""));
    let addon_tokens = if config.get("error").is_none() {
        if let Some(addons) = config.get("active_addons").and_then(|v| v.as_array()) {
            let mut total: u64 = 0;
            let mut sorted_addons: Vec<&str> = addons
                .iter()
                .filter_map(|v| v.as_str())
                .collect();
            sorted_addons.sort();
            for addon_name in sorted_addons {
                // Try local then global
                let local_addon = Path::new(cwd)
                    .join(".claude/hoangsa/worker-rules/addons")
                    .join(format!("{}.md", addon_name));
                let global_addon = Path::new(&home_dir)
                    .join(".claude/hoangsa/worker-rules/addons")
                    .join(format!("{}.md", addon_name));
                let tokens = if let Some(content) =
                    read_file(local_addon.to_str().unwrap_or(""))
                        .or_else(|| read_file(global_addon.to_str().unwrap_or("")))
                {
                    count_tokens(&content)
                } else {
                    DEFAULT_OVERHEAD.addon_tokens_per_addon
                };
                total += tokens;
            }
            total
        } else {
            0
        }
    } else {
        0
    };

    // Tool definitions: count tools in config or use a default estimate
    let tool_count: u64 = if config.get("error").is_none() {
        config
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| a.len() as u64)
            .unwrap_or(10) // default: assume 10 tools
    } else {
        10
    };
    let tool_def_tokens = tool_count * DEFAULT_OVERHEAD.tool_def_tokens_per_tool;

    // Task envelope is fixed
    let task_envelope = DEFAULT_OVERHEAD.task_envelope_tokens;

    base_tokens + addon_tokens + tool_def_tokens + task_envelope
}

// ─── Tool overhead estimation ─────────────────────────────────────────────────

/// Estimate tool call overhead from complexity profile midpoint.
fn estimate_tool_overhead(complexity: &str) -> u64 {
    let profile = complexity_profile(complexity);
    let midpoint = (profile.expected_tool_calls_min + profile.expected_tool_calls_max) / 2;
    midpoint * DEFAULT_OVERHEAD.tool_call_tokens_per_call
}

// ─── Core budget computation ──────────────────────────────────────────────────

/// Compute BudgetBreakdown for a single task.
fn compute_breakdown(
    complexity: &str,
    cwd: &str,
    context_pack_tokens: u64,
    cache_scenario: CacheScenario,
    calibration: &CalibrationFactors,
) -> BudgetBreakdown {
    let profile = complexity_profile(complexity);

    let work_tokens = (profile.work_tokens_min + profile.work_tokens_max) / 2;
    let system_prompt_tokens = estimate_system_prompt_tokens(cwd);
    let tool_overhead_tokens = estimate_tool_overhead(complexity);

    // Cache-adjusted system prompt effective cost
    let system_prompt_effective = match cache_scenario {
        CacheScenario::Warm => {
            (system_prompt_tokens as f64 * DEFAULT_OVERHEAD.cache_warm_factor) as u64
        }
        CacheScenario::Cold => system_prompt_tokens,
    };

    let subtotal = work_tokens + system_prompt_effective + context_pack_tokens + tool_overhead_tokens;
    let safety_margin_tokens = (subtotal as f64 * DEFAULT_OVERHEAD.safety_margin_pct) as u64;
    let base_total = subtotal + safety_margin_tokens;

    // Calibration
    let (calibration_factor, sample_count) = match complexity {
        "low" => (calibration.low, calibration.sample_counts.low),
        "medium" => (calibration.medium, calibration.sample_counts.medium),
        _ => (calibration.high, calibration.sample_counts.high),
    };

    let calibration_applied = sample_count >= 5;
    let total = if calibration_applied {
        (base_total as f64 * calibration_factor) as u64
    } else {
        base_total
    };

    BudgetBreakdown {
        work_tokens,
        system_prompt_tokens,
        system_prompt_effective,
        context_pack_tokens,
        tool_overhead_tokens,
        safety_margin_tokens,
        total,
        cache_scenario,
        calibration_applied,
        calibration_factor,
    }
}

// ─── Load plan helper ─────────────────────────────────────────────────────────

fn resolve_plan_path(plan_path: Option<&str>, cwd: &str) -> String {
    if let Some(p) = plan_path {
        p.to_string()
    } else {
        // Try common session plan locations
        let state_file = Path::new(cwd).join(".hoangsa/state/session.json");
        if state_file.exists() {
            let state = read_json(state_file.to_str().unwrap_or(""));
            if state.get("error").is_none() {
                if let Some(session_id) = state.get("session_id").and_then(|v| v.as_str()) {
                    let plan = Path::new(cwd)
                        .join(".hoangsa/sessions")
                        .join(session_id)
                        .join("plan.json");
                    if plan.exists() {
                        return plan.to_string_lossy().to_string();
                    }
                }
            }
        }
        // Default fallback
        Path::new(cwd)
            .join("plan.json")
            .to_string_lossy()
            .to_string()
    }
}

/// Determine cache scenario: Cold if no depends_on, Warm if has dependencies.
fn cache_scenario_for_task(task: &Value) -> CacheScenario {
    let has_deps = task
        .get("depends_on")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false);
    if has_deps {
        CacheScenario::Warm
    } else {
        CacheScenario::Cold
    }
}

/// Load context pack token count from session dir if available.
fn load_context_pack_tokens(cwd: &str, session_id: Option<&str>, task_id: &str) -> u64 {
    let sid = match session_id {
        Some(s) => s.to_string(),
        None => {
            let state_file = Path::new(cwd).join(".hoangsa/state/session.json");
            if state_file.exists() {
                let state = read_json(state_file.to_str().unwrap_or(""));
                if state.get("error").is_none() {
                    state
                        .get("session_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                } else {
                    return 0;
                }
            } else {
                return 0;
            }
        }
    };

    if sid.is_empty() {
        return 0;
    }

    let pack_file = Path::new(cwd)
        .join(".hoangsa/sessions")
        .join(&sid)
        .join(format!("context-{}.json", task_id));

    if !pack_file.exists() {
        return 0;
    }

    let pack = read_json(pack_file.to_str().unwrap_or(""));
    if pack.get("error").is_none() {
        pack.get("estimated_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
    } else {
        0
    }
}

// ─── Public commands ──────────────────────────────────────────────────────────

/// `budget estimate [--plan <path>] [--task <id>]`
/// Reads plan.json, finds the task by ID, computes BudgetBreakdown.
pub fn cmd_estimate(plan_path: Option<&str>, task_id: Option<&str>) {
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let plan_file = resolve_plan_path(plan_path, &cwd);
    let plan = read_json(&plan_file);

    if plan.get("error").is_some() {
        out(&json!({ "error": plan["error"] }));
        return;
    }

    let tasks = match plan.get("tasks").and_then(|v| v.as_array()) {
        Some(t) => t,
        None => {
            out(&json!({ "error": "plan.json has no tasks array" }));
            return;
        }
    };

    // Select the task
    let task = match task_id {
        Some(tid) => tasks
            .iter()
            .find(|t| t.get("id").and_then(|v| v.as_str()) == Some(tid)),
        None => tasks.first(),
    };

    let task = match task {
        Some(t) => t,
        None => {
            let msg = if let Some(tid) = task_id {
                format!("Task {} not found in plan", tid)
            } else {
                "No tasks in plan".to_string()
            };
            out(&json!({ "error": msg }));
            return;
        }
    };

    let tid = task.get("id").and_then(|v| v.as_str()).unwrap_or("?");
    let complexity = task
        .get("complexity")
        .and_then(|v| v.as_str())
        .unwrap_or("high");

    let workspace_dir = plan
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(&cwd);

    let session_id = plan.get("session_id").and_then(|v| v.as_str());
    let context_pack_tokens = load_context_pack_tokens(workspace_dir, session_id, tid);
    let cache_scenario = cache_scenario_for_task(task);

    let stats_dir = Path::new(workspace_dir)
        .join(".hoangsa/stats")
        .to_string_lossy()
        .to_string();
    let calibration = load_calibration(&stats_dir);

    let breakdown = compute_breakdown(
        complexity,
        workspace_dir,
        context_pack_tokens,
        cache_scenario,
        &calibration,
    );

    let profile = complexity_profile(complexity);

    out(&json!({
        "breakdown": breakdown,
        "overhead_constants": DEFAULT_OVERHEAD,
        "complexity_profile": profile,
    }));
}

/// `budget breakdown [--plan <path>]`
/// Compute breakdown for ALL tasks in plan.
pub fn cmd_breakdown(plan_path: Option<&str>) {
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let plan_file = resolve_plan_path(plan_path, &cwd);
    let plan = read_json(&plan_file);

    if plan.get("error").is_some() {
        out(&json!({ "error": plan["error"] }));
        return;
    }

    let tasks = match plan.get("tasks").and_then(|v| v.as_array()) {
        Some(t) => t,
        None => {
            out(&json!({ "error": "plan.json has no tasks array" }));
            return;
        }
    };

    let workspace_dir = plan
        .get("workspace_dir")
        .and_then(|v| v.as_str())
        .unwrap_or(&cwd);

    let stats_dir = Path::new(workspace_dir)
        .join(".hoangsa/stats")
        .to_string_lossy()
        .to_string();
    let calibration = load_calibration(&stats_dir);

    let session_id = plan.get("session_id").and_then(|v| v.as_str());

    let mut task_breakdowns: Vec<Value> = Vec::new();
    let mut total_sum: u64 = 0;

    // Track wave membership: depends_on = empty → Wave 1, else later wave
    // For simplicity: wave = 1 if no deps, wave = 2 if has deps
    let mut wave_map: std::collections::HashMap<String, u32> = std::collections::HashMap::new();

    for task in tasks {
        let tid = task.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let has_deps = task
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        let wave: u32 = if has_deps { 2 } else { 1 };
        wave_map.insert(tid.to_string(), wave);
    }

    for task in tasks {
        let tid = task.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let complexity = task
            .get("complexity")
            .and_then(|v| v.as_str())
            .unwrap_or("high");

        let context_pack_tokens = load_context_pack_tokens(workspace_dir, session_id, tid);
        let cache_scenario = cache_scenario_for_task(task);

        let breakdown = compute_breakdown(
            complexity,
            workspace_dir,
            context_pack_tokens,
            cache_scenario,
            &calibration,
        );

        total_sum += breakdown.total;

        task_breakdowns.push(json!({
            "id": tid,
            "breakdown": breakdown,
        }));
    }

    // Build wave summary
    let mut waves: std::collections::BTreeMap<u32, (u64, Vec<String>)> =
        std::collections::BTreeMap::new();
    for tb in &task_breakdowns {
        let tid = tb.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let wave = *wave_map.get(tid).unwrap_or(&1);
        let task_total = tb
            .get("breakdown")
            .and_then(|b| b.get("total"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cache_s = tb
            .get("breakdown")
            .and_then(|b| b.get("cache_scenario"))
            .and_then(|v| v.as_str())
            .unwrap_or("cold")
            .to_string();
        let entry = waves.entry(wave).or_insert((0, Vec::new()));
        entry.0 += task_total;
        entry.1.push(cache_s);
    }

    let wave_summary: Vec<Value> = waves
        .iter()
        .map(|(wave, (budget, scenarios))| {
            json!({
                "wave": wave,
                "budget": budget,
                "cache_scenarios": scenarios,
            })
        })
        .collect();

    let plan_total_declared = plan
        .get("budget_tokens")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    out(&json!({
        "tasks": task_breakdowns,
        "plan_total": {
            "estimated": plan_total_declared,
            "breakdown_sum": total_sum,
        },
        "wave_summary": wave_summary,
    }));
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_complexity_profile_low() {
        let p = complexity_profile("low");
        assert_eq!(p.work_tokens_min, 8000);
        assert_eq!(p.work_tokens_max, 15000);
        assert_eq!(p.expected_tool_calls_min, 5);
        assert_eq!(p.expected_tool_calls_max, 10);
    }

    #[test]
    fn test_budget_complexity_profile_medium() {
        let p = complexity_profile("medium");
        assert_eq!(p.work_tokens_min, 15000);
        assert_eq!(p.work_tokens_max, 30000);
        assert_eq!(p.expected_tool_calls_min, 15);
        assert_eq!(p.expected_tool_calls_max, 25);
    }

    #[test]
    fn test_budget_complexity_profile_high() {
        let p = complexity_profile("high");
        assert_eq!(p.work_tokens_min, 30000);
        assert_eq!(p.work_tokens_max, 45000);
        assert_eq!(p.expected_tool_calls_min, 30);
        assert_eq!(p.expected_tool_calls_max, 50);
    }

    #[test]
    fn test_budget_complexity_profile_unknown_defaults_to_high() {
        let p = complexity_profile("unknown");
        assert_eq!(p.work_tokens_min, 30000);
        assert_eq!(p.work_tokens_max, 45000);
    }

    #[test]
    fn test_budget_estimate_tool_overhead_low() {
        // low: (5+10)/2 = 7 calls × 800 = 5600
        let overhead = estimate_tool_overhead("low");
        assert_eq!(overhead, 5600);
    }

    #[test]
    fn test_budget_estimate_tool_overhead_medium() {
        // medium: (15+25)/2 = 20 calls × 800 = 16000
        let overhead = estimate_tool_overhead("medium");
        assert_eq!(overhead, 16000);
    }

    #[test]
    fn test_budget_estimate_tool_overhead_high() {
        // high: (30+50)/2 = 40 calls × 800 = 32000
        let overhead = estimate_tool_overhead("high");
        assert_eq!(overhead, 32000);
    }

    #[test]
    fn test_budget_breakdown_cold_no_calibration() {
        let calibration = CalibrationFactors {
            low: 1.0,
            medium: 1.0,
            high: 1.0,
            sample_counts: crate::cmd::stats::CalibrationSamples {
                low: 0,
                medium: 0,
                high: 0,
            },
        };
        let breakdown = compute_breakdown("medium", "/tmp", 1000, CacheScenario::Cold, &calibration);

        // Cold: system_prompt_effective = system_prompt_tokens
        assert_eq!(breakdown.system_prompt_effective, breakdown.system_prompt_tokens);
        assert!(!breakdown.calibration_applied);
        assert_eq!(breakdown.calibration_factor, 1.0);
        assert!(breakdown.total > 0);

        // work_tokens should be midpoint of medium: (15000+30000)/2 = 22500
        assert_eq!(breakdown.work_tokens, 22500);
    }

    #[test]
    fn test_budget_breakdown_warm_reduces_system_prompt_cost() {
        let calibration = CalibrationFactors {
            low: 1.0,
            medium: 1.0,
            high: 1.0,
            sample_counts: crate::cmd::stats::CalibrationSamples {
                low: 0,
                medium: 0,
                high: 0,
            },
        };
        let cold = compute_breakdown("high", "/tmp", 0, CacheScenario::Cold, &calibration);
        let warm = compute_breakdown("high", "/tmp", 0, CacheScenario::Warm, &calibration);

        // Warm scenario: system_prompt_effective = system_prompt_tokens × 0.1
        assert!(warm.system_prompt_effective < cold.system_prompt_effective);
        assert_eq!(
            warm.system_prompt_effective,
            (cold.system_prompt_tokens as f64 * DEFAULT_OVERHEAD.cache_warm_factor) as u64
        );
        // Warm total should be lower than cold total
        assert!(warm.total < cold.total);
    }

    #[test]
    fn test_budget_breakdown_calibration_applied_when_enough_samples() {
        let calibration = CalibrationFactors {
            low: 1.0,
            medium: 1.5,
            high: 1.0,
            sample_counts: crate::cmd::stats::CalibrationSamples {
                low: 0,
                medium: 10, // >= 5 → calibration applied
                high: 0,
            },
        };
        let breakdown = compute_breakdown("medium", "/tmp", 0, CacheScenario::Cold, &calibration);
        assert!(breakdown.calibration_applied);
        assert_eq!(breakdown.calibration_factor, 1.5);
    }

    #[test]
    fn test_budget_breakdown_calibration_not_applied_below_threshold() {
        let calibration = CalibrationFactors {
            low: 2.0,
            medium: 1.0,
            high: 1.0,
            sample_counts: crate::cmd::stats::CalibrationSamples {
                low: 3, // < 5 → not applied
                medium: 0,
                high: 0,
            },
        };
        let breakdown = compute_breakdown("low", "/tmp", 0, CacheScenario::Cold, &calibration);
        assert!(!breakdown.calibration_applied);
    }

    #[test]
    fn test_budget_cache_scenario_for_task_cold_when_no_deps() {
        let task = serde_json::json!({ "id": "T-01", "depends_on": [] });
        matches!(cache_scenario_for_task(&task), CacheScenario::Cold);
    }

    #[test]
    fn test_budget_cache_scenario_for_task_warm_when_has_deps() {
        let task = serde_json::json!({ "id": "T-02", "depends_on": ["T-01"] });
        matches!(cache_scenario_for_task(&task), CacheScenario::Warm);
    }

    #[test]
    fn test_budget_safety_margin_is_15_pct() {
        let calibration = CalibrationFactors {
            low: 1.0,
            medium: 1.0,
            high: 1.0,
            sample_counts: crate::cmd::stats::CalibrationSamples {
                low: 0,
                medium: 0,
                high: 0,
            },
        };
        let breakdown = compute_breakdown("low", "/tmp", 0, CacheScenario::Cold, &calibration);
        let subtotal = breakdown.work_tokens
            + breakdown.system_prompt_effective
            + breakdown.context_pack_tokens
            + breakdown.tool_overhead_tokens;
        let expected_margin = (subtotal as f64 * 0.15) as u64;
        assert_eq!(breakdown.safety_margin_tokens, expected_margin);
    }

    #[test]
    fn test_budget_context_pack_tokens_included_in_total() {
        let calibration = CalibrationFactors {
            low: 1.0,
            medium: 1.0,
            high: 1.0,
            sample_counts: crate::cmd::stats::CalibrationSamples {
                low: 0,
                medium: 0,
                high: 0,
            },
        };
        let without_pack = compute_breakdown("low", "/tmp", 0, CacheScenario::Cold, &calibration);
        let with_pack = compute_breakdown("low", "/tmp", 5000, CacheScenario::Cold, &calibration);
        assert!(with_pack.total > without_pack.total);
        assert_eq!(with_pack.context_pack_tokens, 5000);
    }
}
