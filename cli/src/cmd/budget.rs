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

    let local_base = Path::new(cwd).join(".claude/hoangsa/worker-rules/base.md");
    let global_base = Path::new(&home_dir).join(".claude/hoangsa/worker-rules/base.md");

    let base_tokens = if let Some(content) = read_file(local_base.to_str().unwrap_or(""))
        .or_else(|| read_file(global_base.to_str().unwrap_or("")))
    {
        count_tokens(&content)
    } else {
        DEFAULT_OVERHEAD.base_rules_tokens
    };

    let local_config = Path::new(cwd).join(".hoangsa/config.json");
    let config = read_json(local_config.to_str().unwrap_or(""));
    let config_ok = config.get("error").is_none();

    let addon_tokens = if config_ok {
        if let Some(addons) = config.get("active_addons").and_then(|v| v.as_array()) {
            let mut sorted_addons: Vec<&str> =
                addons.iter().filter_map(|v| v.as_str()).collect();
            sorted_addons.sort();
            sorted_addons.iter().map(|addon_name| {
                let local_addon = Path::new(cwd)
                    .join(".claude/hoangsa/worker-rules/addons")
                    .join(format!("{}.md", addon_name));
                let global_addon = Path::new(&home_dir)
                    .join(".claude/hoangsa/worker-rules/addons")
                    .join(format!("{}.md", addon_name));
                read_file(local_addon.to_str().unwrap_or(""))
                    .or_else(|| read_file(global_addon.to_str().unwrap_or("")))
                    .map(|c| count_tokens(&c))
                    .unwrap_or(DEFAULT_OVERHEAD.addon_tokens_per_addon)
            }).sum()
        } else {
            0
        }
    } else {
        0
    };

    let tool_count: u64 = if config_ok {
        config
            .get("tools")
            .and_then(|v| v.as_array())
            .map(|a| a.len() as u64)
            .unwrap_or(10)
    } else {
        10
    };
    let tool_def_tokens = tool_count * DEFAULT_OVERHEAD.tool_def_tokens_per_tool;

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

    let system_prompt_effective = match cache_scenario {
        CacheScenario::Warm => {
            (system_prompt_tokens as f64 * DEFAULT_OVERHEAD.cache_warm_factor) as u64
        }
        CacheScenario::Cold => system_prompt_tokens,
    };

    let subtotal = work_tokens + system_prompt_effective + context_pack_tokens + tool_overhead_tokens;
    let safety_margin_tokens = (subtotal as f64 * DEFAULT_OVERHEAD.safety_margin_pct) as u64;
    let base_total = subtotal + safety_margin_tokens;

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
    let mut waves: std::collections::BTreeMap<u32, (u64, Vec<String>)> =
        std::collections::BTreeMap::new();

    for task in tasks {
        let tid = task.get("id").and_then(|v| v.as_str()).unwrap_or("?");
        let complexity = task
            .get("complexity")
            .and_then(|v| v.as_str())
            .unwrap_or("high");
        let wave: u32 = if task
            .get("depends_on")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false)
        {
            2
        } else {
            1
        };

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

        let cache_s = match breakdown.cache_scenario {
            CacheScenario::Cold => "cold",
            CacheScenario::Warm => "warm",
        };
        let entry = waves.entry(wave).or_insert((0, Vec::new()));
        entry.0 += breakdown.total;
        entry.1.push(cache_s.to_string());

        task_breakdowns.push(json!({
            "id": tid,
            "breakdown": breakdown,
        }));
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
    use crate::cmd::stats::CalibrationSamples;

    /// Returns a CalibrationFactors with all factors at 1.0 and zero sample counts.
    fn no_cal() -> CalibrationFactors {
        CalibrationFactors {
            low: 1.0,
            medium: 1.0,
            high: 1.0,
            sample_counts: CalibrationSamples { low: 0, medium: 0, high: 0 },
        }
    }

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
        let overhead = estimate_tool_overhead("low");
        assert_eq!(overhead, 5600);
    }

    #[test]
    fn test_budget_estimate_tool_overhead_medium() {
        let overhead = estimate_tool_overhead("medium");
        assert_eq!(overhead, 16000);
    }

    #[test]
    fn test_budget_estimate_tool_overhead_high() {
        let overhead = estimate_tool_overhead("high");
        assert_eq!(overhead, 32000);
    }

    #[test]
    fn test_budget_breakdown_cold_no_calibration() {
        let breakdown = compute_breakdown("medium", "/tmp", 1000, CacheScenario::Cold, &no_cal());

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
        let cal = no_cal();
        let cold = compute_breakdown("high", "/tmp", 0, CacheScenario::Cold, &cal);
        let warm = compute_breakdown("high", "/tmp", 0, CacheScenario::Warm, &cal);

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
            sample_counts: CalibrationSamples { low: 0, medium: 10, high: 0 },
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
            sample_counts: CalibrationSamples { low: 3, medium: 0, high: 0 },
        };
        let breakdown = compute_breakdown("low", "/tmp", 0, CacheScenario::Cold, &calibration);
        assert!(!breakdown.calibration_applied);
    }

    #[test]
    fn test_budget_cache_scenario_for_task_cold_when_no_deps() {
        let task = serde_json::json!({ "id": "T-01", "depends_on": [] });
        assert!(matches!(cache_scenario_for_task(&task), CacheScenario::Cold));
    }

    #[test]
    fn test_budget_cache_scenario_for_task_warm_when_has_deps() {
        let task = serde_json::json!({ "id": "T-02", "depends_on": ["T-01"] });
        assert!(matches!(cache_scenario_for_task(&task), CacheScenario::Warm));
    }

    #[test]
    fn test_budget_safety_margin_is_15_pct() {
        let breakdown = compute_breakdown("low", "/tmp", 0, CacheScenario::Cold, &no_cal());
        let subtotal = breakdown.work_tokens
            + breakdown.system_prompt_effective
            + breakdown.context_pack_tokens
            + breakdown.tool_overhead_tokens;
        let expected_margin = (subtotal as f64 * 0.15) as u64;
        assert_eq!(breakdown.safety_margin_tokens, expected_margin);
    }

    #[test]
    fn test_budget_context_pack_tokens_included_in_total() {
        let cal = no_cal();
        let without_pack = compute_breakdown("low", "/tmp", 0, CacheScenario::Cold, &cal);
        let with_pack = compute_breakdown("low", "/tmp", 5000, CacheScenario::Cold, &cal);
        assert!(with_pack.total > without_pack.total);
        assert_eq!(with_pack.context_pack_tokens, 5000);
    }

    #[test]
    fn test_budget_default_overhead_constants() {
        assert_eq!(DEFAULT_OVERHEAD.base_rules_tokens, 2500);
        assert!((DEFAULT_OVERHEAD.safety_margin_pct - 0.15).abs() < f64::EPSILON);
        assert!((DEFAULT_OVERHEAD.cache_warm_factor - 0.1).abs() < f64::EPSILON);
        assert_eq!(DEFAULT_OVERHEAD.tool_call_tokens_per_call, 800);
        assert_eq!(DEFAULT_OVERHEAD.addon_tokens_per_addon, 300);
        assert_eq!(DEFAULT_OVERHEAD.tool_def_tokens_per_tool, 150);
        assert_eq!(DEFAULT_OVERHEAD.task_envelope_tokens, 500);
    }

    #[test]
    fn test_budget_estimate_with_calibration_applied() {
        // REQ-10: when sample_counts >= 5, the calibration factor is applied to total
        let factor = 1.8_f64;
        let calibration = CalibrationFactors {
            low: 1.0,
            medium: factor,
            high: 1.0,
            sample_counts: CalibrationSamples { low: 0, medium: 5, high: 0 },
        };
        let uncalibrated = CalibrationFactors {
            medium: factor,
            ..no_cal()
        };
        let with_cal = compute_breakdown("medium", "/tmp", 0, CacheScenario::Cold, &calibration);
        let without_cal = compute_breakdown("medium", "/tmp", 0, CacheScenario::Cold, &uncalibrated);

        assert!(with_cal.calibration_applied, "should apply calibration when sample_count >= 5");
        assert_eq!(with_cal.calibration_factor, factor);
        // Total should be scaled by factor relative to uncalibrated base
        let expected_total = (without_cal.total as f64 * factor) as u64;
        assert_eq!(with_cal.total, expected_total);
    }

    #[test]
    fn test_budget_estimate_without_calibration() {
        // REQ-10: when sample_counts < 5, calibration_applied = false and total is unscaled
        let calibration = CalibrationFactors {
            high: 2.5, // would significantly change total if applied
            sample_counts: CalibrationSamples { low: 0, medium: 0, high: 4 },
            ..no_cal()
        };
        let breakdown = compute_breakdown("high", "/tmp", 0, CacheScenario::Cold, &calibration);
        assert!(!breakdown.calibration_applied, "calibration_applied must be false when sample_count < 5");

        // Verify the total is NOT scaled (base_total == total when not applied)
        let subtotal = breakdown.work_tokens
            + breakdown.system_prompt_effective
            + breakdown.context_pack_tokens
            + breakdown.tool_overhead_tokens;
        let safety = (subtotal as f64 * DEFAULT_OVERHEAD.safety_margin_pct) as u64;
        let expected_base_total = subtotal + safety;
        assert_eq!(breakdown.total, expected_base_total);
    }

    #[test]
    fn test_budget_system_prompt_fallback() {
        // REQ-11: estimate_system_prompt_tokens returns a reasonable value even
        // when no files exist (falls back to DEFAULT_OVERHEAD.base_rules_tokens + tool overhead)
        let tokens = estimate_system_prompt_tokens("/nonexistent/path/that/does/not/exist");
        // Must be > 0 and at least cover the base rules fallback plus 10 default tools
        let min_expected = DEFAULT_OVERHEAD.base_rules_tokens
            + 10 * DEFAULT_OVERHEAD.tool_def_tokens_per_tool
            + DEFAULT_OVERHEAD.task_envelope_tokens;
        assert!(
            tokens >= min_expected,
            "fallback tokens ({}) should be at least {} (base + 10 tools + envelope)",
            tokens,
            min_expected
        );
    }

    #[test]
    fn test_budget_safety_margin_percentage() {
        // REQ-01: safety margin is exactly 15% of subtotal
        for complexity in &["low", "medium", "high"] {
            let bd = compute_breakdown(complexity, "/tmp", 2000, CacheScenario::Cold, &no_cal());
            let subtotal = bd.work_tokens
                + bd.system_prompt_effective
                + bd.context_pack_tokens
                + bd.tool_overhead_tokens;
            let expected_margin = (subtotal as f64 * 0.15) as u64;
            assert_eq!(
                bd.safety_margin_tokens,
                expected_margin,
                "safety margin for {} should be exactly 15% of subtotal",
                complexity
            );
        }
    }

    #[test]
    fn test_budget_complexity_profiles_bounds() {
        // REQ-01: all three profiles must have min < max for both work_tokens and tool_calls
        for complexity in &["low", "medium", "high"] {
            let p = complexity_profile(complexity);
            assert!(
                p.work_tokens_min < p.work_tokens_max,
                "{}: work_tokens_min ({}) must be < work_tokens_max ({})",
                complexity,
                p.work_tokens_min,
                p.work_tokens_max
            );
            assert!(
                p.expected_tool_calls_min < p.expected_tool_calls_max,
                "{}: expected_tool_calls_min ({}) must be < expected_tool_calls_max ({})",
                complexity,
                p.expected_tool_calls_min,
                p.expected_tool_calls_max
            );
        }
    }
}
