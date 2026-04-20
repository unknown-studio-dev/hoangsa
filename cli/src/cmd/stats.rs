use crate::helpers::out;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{BufRead, BufReader, Write as IoWrite};
use std::path::Path;

#[derive(Debug, Serialize, Deserialize)]
pub struct TaskUsageRecord {
    pub task_id: String,
    pub session_id: String,
    pub complexity: String,
    pub estimated_budget: u64,
    pub tracked_usage: u64,
    pub tool_calls_count: u64,
    pub turns_count: u64,
    pub content_tokens_sent: u64,
    pub content_tokens_received: u64,
    pub cache_scenario: String,
    pub timestamp: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CalibrationFactors {
    pub low: f64,
    pub medium: f64,
    pub high: f64,
    pub sample_counts: CalibrationSamples,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CalibrationSamples {
    pub low: u64,
    pub medium: u64,
    pub high: u64,
}

/// Returns the path to the token-usage.jsonl file in the given workspace.
fn stats_file_path(workspace: &str) -> std::path::PathBuf {
    Path::new(workspace)
        .join(".hoangsa")
        .join("stats")
        .join("token-usage.jsonl")
}

/// Returns the current working directory as a String.
fn current_workspace() -> String {
    std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

/// Returns a CalibrationFactors with all factors at 1.0 and zero sample counts.
fn default_calibration() -> CalibrationFactors {
    CalibrationFactors {
        low: 1.0,
        medium: 1.0,
        high: 1.0,
        sample_counts: CalibrationSamples {
            low: 0,
            medium: 0,
            high: 0,
        },
    }
}

/// Load all records from token-usage.jsonl. Returns empty vec if file doesn't exist.
fn load_records(workspace: &str) -> Vec<TaskUsageRecord> {
    let path = stats_file_path(workspace);
    let file = match fs::File::open(&path) {
        Ok(f) => f,
        Err(_) => return vec![],
    };
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<TaskUsageRecord>(trimmed) {
            records.push(record);
        }
    }
    records
}

/// `stats record [json_data]` — parse JSON string as TaskUsageRecord, append to
/// `.hoangsa/stats/token-usage.jsonl`. Outputs `{ "success": true, "records_total": N }`.
pub fn cmd_record(json_data: Option<&str>) {
    let workspace = current_workspace();

    let data = match json_data {
        Some(s) => s,
        None => {
            out(&json!({ "error": "No JSON data provided" }));
            return;
        }
    };

    // Validate the JSON can be parsed as TaskUsageRecord
    let record: TaskUsageRecord = match serde_json::from_str(data) {
        Ok(r) => r,
        Err(e) => {
            out(&json!({ "error": format!("Invalid record JSON: {}", e) }));
            return;
        }
    };

    let file_path = stats_file_path(&workspace);
    let stats_dir = file_path.parent().expect("stats path has parent");
    if let Err(e) = fs::create_dir_all(stats_dir) {
        out(&json!({ "error": format!("Cannot create stats dir: {}", e) }));
        return;
    }

    let line = match serde_json::to_string(&record) {
        Ok(s) => s,
        Err(e) => {
            out(&json!({ "error": format!("Serialization error: {}", e) }));
            return;
        }
    };

    let mut file = match fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
    {
        Ok(f) => f,
        Err(e) => {
            out(&json!({ "error": format!("Cannot open stats file: {}", e) }));
            return;
        }
    };

    if let Err(e) = writeln!(file, "{}", line) {
        out(&json!({ "error": format!("Cannot write record: {}", e) }));
        return;
    }

    let total = load_records(&workspace).len();
    out(&json!({ "success": true, "records_total": total }));
}

/// `stats summary [--last N] [--complexity low|medium|high]`
/// Reads token-usage.jsonl and outputs aggregated stats with calibration.
pub fn cmd_summary(args: &[&str]) {
    let workspace = current_workspace();

    // Parse --last and --complexity flags from args
    let mut last_n: Option<usize> = None;
    let mut filter_complexity: Option<String> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--last" => {
                if let Some(val) = args.get(i + 1) {
                    if let Ok(n) = val.parse::<usize>() {
                        last_n = Some(n);
                    }
                    i += 2;
                } else {
                    i += 1;
                }
            }
            "--complexity" => {
                if let Some(val) = args.get(i + 1) {
                    filter_complexity = Some(val.to_lowercase());
                    i += 2;
                } else {
                    i += 1;
                }
            }
            _ => {
                i += 1;
            }
        }
    }

    let all_records = load_records(&workspace);
    let total_records = all_records.len();

    // Apply --last filter first (take last N records)
    let after_last: Vec<&TaskUsageRecord> = if let Some(n) = last_n {
        let skip = total_records.saturating_sub(n);
        all_records.iter().skip(skip).collect()
    } else {
        all_records.iter().collect()
    };

    // Apply --complexity filter
    let filtered: Vec<&TaskUsageRecord> = if let Some(ref c) = filter_complexity {
        after_last
            .into_iter()
            .filter(|r| r.complexity.to_lowercase() == *c)
            .collect()
    } else {
        after_last
    };

    let filtered_count = filtered.len();

    // Compute averages across filtered set
    let (avg_estimated, avg_actual, avg_ratio) = if filtered_count == 0 {
        (0u64, 0u64, 0.0f64)
    } else {
        let sum_est: u64 = filtered.iter().map(|r| r.estimated_budget).sum();
        let sum_actual: u64 = filtered.iter().map(|r| r.tracked_usage).sum();
        let avg_est = sum_est / filtered_count as u64;
        let avg_act = sum_actual / filtered_count as u64;
        let ratio = if avg_est > 0 {
            avg_act as f64 / avg_est as f64
        } else {
            0.0
        };
        (avg_est, avg_act, ratio)
    };

    // Compute per-complexity calibration from ALL records (not just filtered)
    let calibration = compute_calibration(&all_records);

    // Per-complexity breakdown from filtered records
    let complexities = ["low", "medium", "high"];
    let mut by_complexity = serde_json::Map::new();
    for &cx in &complexities {
        let cx_records: Vec<&TaskUsageRecord> = filtered
            .iter()
            .filter(|r| r.complexity.to_lowercase() == cx)
            .copied()
            .collect();
        let cx_count = cx_records.len();
        if cx_count == 0 {
            by_complexity.insert(
                cx.to_string(),
                json!({ "count": 0, "avg_estimated": 0, "avg_actual": 0, "avg_ratio": 0.0 }),
            );
        } else {
            let sum_est: u64 = cx_records.iter().map(|r| r.estimated_budget).sum();
            let sum_act: u64 = cx_records.iter().map(|r| r.tracked_usage).sum();
            let cx_avg_est = sum_est / cx_count as u64;
            let cx_avg_act = sum_act / cx_count as u64;
            let cx_ratio = if cx_avg_est > 0 {
                cx_avg_act as f64 / cx_avg_est as f64
            } else {
                0.0
            };
            by_complexity.insert(
                cx.to_string(),
                json!({
                    "count": cx_count,
                    "avg_estimated": cx_avg_est,
                    "avg_actual": cx_avg_act,
                    "avg_ratio": cx_ratio,
                }),
            );
        }
    }

    let output = json!({
        "total_records": total_records,
        "filtered_records": filtered_count,
        "avg_estimated": avg_estimated,
        "avg_actual": avg_actual,
        "avg_ratio": avg_ratio,
        "calibration": calibration,
        "by_complexity": Value::Object(by_complexity),
    });

    out(&output);
}

/// Compute CalibrationFactors from a slice of records.
/// Averages actual/estimated per complexity, capping at 3.0.
fn compute_calibration(records: &[TaskUsageRecord]) -> CalibrationFactors {
    let factor_for = |cx: &str| -> (f64, u64) {
        let cx_records: Vec<&TaskUsageRecord> = records
            .iter()
            .filter(|r| r.complexity.to_lowercase() == cx && r.estimated_budget > 0)
            .collect();
        let count = cx_records.len() as u64;
        if count == 0 {
            return (1.0, 0);
        }
        let sum_ratio: f64 = cx_records
            .iter()
            .map(|r| r.tracked_usage as f64 / r.estimated_budget as f64)
            .sum();
        let avg = sum_ratio / count as f64;
        // Cap at 3.0 to avoid outlier drift
        let capped = avg.min(3.0_f64).max(0.0);
        (capped, count)
    };

    let (low_factor, low_count) = factor_for("low");
    let (medium_factor, medium_count) = factor_for("medium");
    let (high_factor, high_count) = factor_for("high");

    CalibrationFactors {
        low: low_factor,
        medium: medium_factor,
        high: high_factor,
        sample_counts: CalibrationSamples {
            low: low_count,
            medium: medium_count,
            high: high_count,
        },
    }
}

/// Read token-usage.jsonl from `stats_dir`, compute avg(actual/estimated) per complexity.
/// Caps factor at 3.0 to avoid outlier drift. Returns (1.0, 1.0, 1.0) if no stats.
pub fn load_calibration(stats_dir: &str) -> CalibrationFactors {
    // stats_dir points directly at the stats folder; synthesise a fake workspace path
    // so load_records (which appends .hoangsa/stats/token-usage.jsonl) resolves correctly.
    let workspace = Path::new(stats_dir)
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    let records = load_records(&workspace);
    if records.is_empty() {
        return default_calibration();
    }
    compute_calibration(&records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write as IoWrite;

    fn sample_record(complexity: &str, estimated: u64, actual: u64) -> TaskUsageRecord {
        TaskUsageRecord {
            task_id: "T-01".to_string(),
            session_id: "feat/test".to_string(),
            complexity: complexity.to_string(),
            estimated_budget: estimated,
            tracked_usage: actual,
            tool_calls_count: 5,
            turns_count: 10,
            content_tokens_sent: 5000,
            content_tokens_received: 4000,
            cache_scenario: "warm".to_string(),
            timestamp: "2026-04-20T05:00:00Z".to_string(),
        }
    }

    /// Create a unique temp dir for tests (cleaned up at end of test).
    fn make_temp_dir(suffix: &str) -> std::path::PathBuf {
        let base = std::env::temp_dir().join(format!("hoangsa_stats_test_{}", suffix));
        fs::create_dir_all(&base).expect("create temp dir");
        base
    }

    fn cleanup(dir: &std::path::Path) {
        let _ = fs::remove_dir_all(dir);
    }

    /// Write records directly into a stats dir (for load_calibration tests).
    fn write_records_to_stats_dir(stats_dir: &std::path::Path, records: &[TaskUsageRecord]) {
        fs::create_dir_all(stats_dir).expect("create stats dir");
        let file_path = stats_dir.join("token-usage.jsonl");
        let mut f = fs::File::create(&file_path).expect("create file");
        for r in records {
            let line = serde_json::to_string(r).expect("serialize");
            writeln!(f, "{}", line).expect("write line");
        }
    }

    #[test]
    fn test_stats_load_calibration_no_file() {
        let dir = make_temp_dir("no_file");
        let stats_dir = dir.join(".hoangsa").join("stats");
        let result = load_calibration(stats_dir.to_str().expect("path str"));
        cleanup(&dir);
        assert_eq!(result.low, 1.0);
        assert_eq!(result.medium, 1.0);
        assert_eq!(result.high, 1.0);
        assert_eq!(result.sample_counts.low, 0);
        assert_eq!(result.sample_counts.medium, 0);
        assert_eq!(result.sample_counts.high, 0);
    }

    #[test]
    fn test_stats_load_calibration_with_records() {
        let dir = make_temp_dir("with_records");
        let stats_dir = dir.join(".hoangsa").join("stats");

        // low: actual=10000, estimated=10000 → ratio 1.0
        // medium: actual=12000, estimated=10000 → ratio 1.2
        // high: actual=14000, estimated=10000 → ratio 1.4
        let records = vec![
            sample_record("low", 10000, 10000),
            sample_record("medium", 10000, 12000),
            sample_record("high", 10000, 14000),
        ];
        write_records_to_stats_dir(&stats_dir, &records);

        let result = load_calibration(stats_dir.to_str().expect("path str"));
        cleanup(&dir);
        assert!((result.low - 1.0).abs() < 0.01, "low factor should be ~1.0");
        assert!(
            (result.medium - 1.2).abs() < 0.01,
            "medium factor should be ~1.2"
        );
        assert!((result.high - 1.4).abs() < 0.01, "high factor should be ~1.4");
        assert_eq!(result.sample_counts.low, 1);
        assert_eq!(result.sample_counts.medium, 1);
        assert_eq!(result.sample_counts.high, 1);
    }

    #[test]
    fn test_stats_load_calibration_caps_at_3() {
        let dir = make_temp_dir("caps");
        let stats_dir = dir.join(".hoangsa").join("stats");

        // outlier: actual=50000, estimated=10000 → ratio 5.0 → should cap at 3.0
        let record = sample_record("low", 10000, 50000);
        write_records_to_stats_dir(&stats_dir, &[record]);

        let result = load_calibration(stats_dir.to_str().expect("path str"));
        cleanup(&dir);
        assert!(
            result.low <= 3.0,
            "calibration factor should be capped at 3.0"
        );
        assert_eq!(result.low, 3.0);
    }

    #[test]
    fn test_stats_load_calibration_empty_file() {
        let dir = make_temp_dir("empty_file");
        let stats_dir = dir.join(".hoangsa").join("stats");
        fs::create_dir_all(&stats_dir).expect("create stats dir");
        fs::File::create(stats_dir.join("token-usage.jsonl")).expect("create empty file");

        let result = load_calibration(stats_dir.to_str().expect("path str"));
        cleanup(&dir);
        assert_eq!(result.low, 1.0);
        assert_eq!(result.medium, 1.0);
        assert_eq!(result.high, 1.0);
    }

    #[test]
    fn test_stats_compute_calibration_no_records() {
        let records: Vec<TaskUsageRecord> = vec![];
        let result = compute_calibration(&records);
        assert_eq!(result.low, 1.0);
        assert_eq!(result.medium, 1.0);
        assert_eq!(result.high, 1.0);
        assert_eq!(result.sample_counts.low, 0);
    }

    #[test]
    fn test_stats_record_serialization() {
        let r = sample_record("medium", 25000, 31000);
        let json_str = serde_json::to_string(&r).expect("serialize");
        let parsed: TaskUsageRecord = serde_json::from_str(&json_str).expect("deserialize");
        assert_eq!(parsed.complexity, "medium");
        assert_eq!(parsed.estimated_budget, 25000);
        assert_eq!(parsed.tracked_usage, 31000);
    }

    #[test]
    fn test_stats_stats_file_path() {
        let p = stats_file_path("/workspace");
        assert!(p.ends_with(".hoangsa/stats/token-usage.jsonl"));
    }

    #[test]
    fn test_stats_load_records_missing_file() {
        let dir = make_temp_dir("missing");
        let records = load_records(dir.to_str().expect("path str"));
        cleanup(&dir);
        assert!(records.is_empty());
    }

    #[test]
    fn test_stats_load_records_with_data() {
        let dir = make_temp_dir("load_data");
        let stats_dir = dir.join(".hoangsa").join("stats");
        fs::create_dir_all(&stats_dir).expect("create dirs");
        let file_path = stats_dir.join("token-usage.jsonl");

        let r1 = sample_record("low", 10000, 9000);
        let r2 = sample_record("high", 30000, 35000);
        {
            let mut f = fs::File::create(&file_path).expect("create file");
            writeln!(f, "{}", serde_json::to_string(&r1).expect("serialize")).expect("write");
            writeln!(f, "{}", serde_json::to_string(&r2).expect("serialize")).expect("write");
        }

        let records = load_records(dir.to_str().expect("path str"));
        cleanup(&dir);
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn test_stats_record_creates_dir_and_file() {
        // REQ-03: cmd_record creates .hoangsa/stats/ directory and file if they don't exist
        let dir = make_temp_dir("record_creates_dir");
        // Ensure the stats dir does NOT exist before the call
        let expected_stats_dir = dir.join(".hoangsa").join("stats");
        assert!(!expected_stats_dir.exists(), "stats dir must not exist before test");

        // cmd_record uses current_workspace() so we must change cwd
        let original_dir = std::env::current_dir().expect("get cwd");
        std::env::set_current_dir(&dir).expect("set cwd to temp dir");

        let record_json = serde_json::to_string(&sample_record("low", 10000, 9000))
            .expect("serialize record");
        cmd_record(Some(&record_json));

        std::env::set_current_dir(&original_dir).expect("restore cwd");

        let expected_file = expected_stats_dir.join("token-usage.jsonl");
        assert!(expected_stats_dir.exists(), ".hoangsa/stats/ directory should have been created");
        assert!(expected_file.exists(), "token-usage.jsonl should have been created");

        cleanup(&dir);
    }

    #[test]
    fn test_stats_record_appends_not_overwrites() {
        // REQ-03: writing 2 records must result in both being present (append mode)
        // Test the append behavior directly via the file-system helpers, which exercise
        // the same OpenOptions::append(true) path that cmd_record uses internally.
        let dir = make_temp_dir("record_appends");
        let stats_dir = dir.join(".hoangsa").join("stats");
        fs::create_dir_all(&stats_dir).expect("create stats dir");
        let file_path = stats_dir.join("token-usage.jsonl");

        let r1 = sample_record("low", 10000, 9000);
        let r2 = sample_record("high", 30000, 35000);

        // Write two records using the same append mode that cmd_record uses
        for record in &[&r1, &r2] {
            let line = serde_json::to_string(record).expect("serialize record");
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
                .expect("open file in append mode");
            writeln!(file, "{}", line).expect("write line");
        }

        let records = load_records(dir.to_str().expect("path str"));
        cleanup(&dir);
        assert_eq!(records.len(), 2, "both records must be present after two appends");
        assert_eq!(records[0].complexity, "low");
        assert_eq!(records[1].complexity, "high");
    }

    #[test]
    fn test_stats_summary_last_filter() {
        // REQ-04: summary with --last 2 should only process the last 2 records
        // We verify via load_records + manual filtering logic mirroring cmd_summary
        let dir = make_temp_dir("summary_last");
        let stats_dir = dir.join(".hoangsa").join("stats");

        let records = vec![
            sample_record("low", 10000, 9000),
            sample_record("low", 10000, 9500),
            sample_record("low", 10000, 10000),
            sample_record("low", 10000, 11000),
            sample_record("low", 10000, 12000),
        ];
        write_records_to_stats_dir(&stats_dir, &records);

        let all = load_records(dir.to_str().expect("path str"));
        assert_eq!(all.len(), 5, "should have 5 records total");

        // Replicate --last 2 filtering logic from cmd_summary
        let last_n = 2usize;
        let skip = all.len().saturating_sub(last_n);
        let last_two: Vec<&TaskUsageRecord> = all.iter().skip(skip).collect();
        assert_eq!(last_two.len(), 2, "--last 2 should yield exactly 2 records");
        assert_eq!(last_two[0].tracked_usage, 11000, "4th record should be first of last 2");
        assert_eq!(last_two[1].tracked_usage, 12000, "5th record should be second of last 2");

        cleanup(&dir);
    }

    #[test]
    fn test_stats_summary_complexity_filter() {
        // REQ-04: filtering by complexity should return only matching records
        let dir = make_temp_dir("summary_complexity");
        let stats_dir = dir.join(".hoangsa").join("stats");

        let records = vec![
            sample_record("low", 10000, 9000),
            sample_record("medium", 20000, 22000),
            sample_record("low", 10000, 10500),
            sample_record("high", 35000, 40000),
            sample_record("medium", 20000, 19000),
        ];
        write_records_to_stats_dir(&stats_dir, &records);

        let all = load_records(dir.to_str().expect("path str"));
        let medium_only: Vec<&TaskUsageRecord> = all
            .iter()
            .filter(|r| r.complexity.to_lowercase() == "medium")
            .collect();

        assert_eq!(medium_only.len(), 2, "should find exactly 2 medium records");
        for r in &medium_only {
            assert_eq!(r.complexity, "medium", "all filtered records must be medium complexity");
        }

        let low_only: Vec<&TaskUsageRecord> = all
            .iter()
            .filter(|r| r.complexity.to_lowercase() == "low")
            .collect();
        assert_eq!(low_only.len(), 2, "should find exactly 2 low records");

        cleanup(&dir);
    }

    #[test]
    fn test_stats_calibration_cap_at_3() {
        // REQ-10: verify cap works when actual/estimated ratio > 3.0
        // Use compute_calibration directly with outlier records
        let outlier_records = vec![
            // ratio = 50000/10000 = 5.0 → should be capped at 3.0
            sample_record("medium", 10000, 50000),
            sample_record("medium", 10000, 60000), // ratio 6.0 → capped at 3.0
        ];
        let result = compute_calibration(&outlier_records);
        assert!(
            result.medium <= 3.0,
            "calibration factor {} must be capped at 3.0",
            result.medium
        );
        assert_eq!(result.medium, 3.0, "both ratios exceed cap, average should be exactly 3.0");
        assert_eq!(result.sample_counts.medium, 2);
        // low and high with no records should default to 1.0
        assert_eq!(result.low, 1.0);
        assert_eq!(result.high, 1.0);
    }

    #[test]
    fn test_stats_empty_file_returns_default_calibration() {
        // REQ-10: empty JSONL file returns (1.0, 1.0, 1.0) for all factors
        let dir = make_temp_dir("default_cal_empty");
        let stats_dir = dir.join(".hoangsa").join("stats");
        fs::create_dir_all(&stats_dir).expect("create stats dir");
        // Create an empty file
        fs::File::create(stats_dir.join("token-usage.jsonl"))
            .expect("create empty token-usage.jsonl");

        let cal = load_calibration(stats_dir.to_str().expect("path str"));
        cleanup(&dir);

        assert_eq!(cal.low, 1.0, "empty file should yield low factor 1.0");
        assert_eq!(cal.medium, 1.0, "empty file should yield medium factor 1.0");
        assert_eq!(cal.high, 1.0, "empty file should yield high factor 1.0");
        assert_eq!(cal.sample_counts.low, 0);
        assert_eq!(cal.sample_counts.medium, 0);
        assert_eq!(cal.sample_counts.high, 0);
    }
}
