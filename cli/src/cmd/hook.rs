use serde_json::{Value, json};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

fn read_stdin_with_timeout(timeout_secs: u64) -> String {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = std::io::stdin().read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    rx.recv_timeout(Duration::from_secs(timeout_secs))
        .unwrap_or_default()
}

fn read_stdin() -> String {
    read_stdin_with_timeout(3)
}

fn parse_stdin() -> Value {
    let input = read_stdin();
    serde_json::from_str(&input).unwrap_or(json!({}))
}

// ─── statusline ──────────────────────────────────────────────────────────────

pub fn cmd_statusline() {
    let data = parse_stdin();
    let model = data["model"]["display_name"].as_str().unwrap_or("Claude");
    let dir = data["workspace"]["current_dir"].as_str().unwrap_or(".");
    let session = data["session_id"].as_str().unwrap_or("");
    let remaining = data["context_window"]["remaining_percentage"].as_f64();
    let used_pct_live = data["context_window"]["used_percentage"].as_f64();
    let ctx_size = data["context_window"]["context_window_size"].as_u64();
    let total_input = data["context_window"]["total_input_tokens"].as_u64();
    let total_output = data["context_window"]["total_output_tokens"].as_u64();

    // Raw remaining percentage — no cache, no buffer
    let rem = remaining
        .or_else(|| used_pct_live.map(|u| 100.0 - u))
        .unwrap_or(100.0);
    let has_data = remaining.is_some() || used_pct_live.is_some();
    let used = (100.0 - rem).clamp(0.0, 100.0).round() as u32;

    // Raw total tokens
    let total_tokens = match (total_input, total_output) {
        (Some(i), Some(o)) => Some(i + o),
        (Some(i), None) => Some(i),
        (None, Some(o)) => Some(o),
        _ => None,
    };

    // Context window display
    let ctx = {
        // Write bridge file for context-monitor
        if !session.is_empty() && has_data {
            let bridge_path = std::env::temp_dir().join(format!("claude-ctx-{session}.json"));
            let bridge = json!({
                "session_id": session,
                "remaining_percentage": rem,
                "used_pct": used,
                "timestamp": now_unix()
            });
            let _ = fs::write(&bridge_path, bridge.to_string());
        }

        let filled = ((used as f64 / 100.0) * 10.0).round() as usize;
        let bar: String = "\u{2588}".repeat(filled) + &"\u{2591}".repeat(10 - filled);

        // Color + emoji based on remaining_percentage: >50% green 😊, >35% yellow 😢, ≤35% red 😭
        let emoji = if rem > 50.0 { "\u{1f60a}" } else if rem > 35.0 { "\u{1f622}" } else { "\u{1f62d}" };
        let color = if rem > 50.0 { "\x1b[32m" } else if rem > 35.0 { "\x1b[33m" } else { "\x1b[31m" };

        // Token count display — raw values, no baseline subtraction
        let fmt_k = |n: u64| -> String {
            if n == 0 {
                "0K".to_string()
            } else if n < 1000 {
                "<1K".to_string()
            } else {
                format!("{}K", (n + 500) / 1000)
            }
        };
        let token_info = match (total_tokens, ctx_size) {
            (Some(t), Some(s)) => format!(" {}/{}", fmt_k(t), fmt_k(s)),
            (Some(t), None) => format!(" {}", fmt_k(t)),
            _ => String::new(),
        };

        let pct_display = if has_data {
            format!("{used}%")
        } else {
            "--%".to_string()
        };

        format!(
            " {emoji} {color}{bar} {pct_display}{token_info}\x1b[0m"
        )
    };

    // Current task from todos
    let task = get_current_task(session);

    // Update check
    let hoangsa_update = get_update_banner();

    // Output
    let dirname = Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string());

    if task.is_empty() {
        print!(
            "{hoangsa_update}\x1b[2m{model}\x1b[0m \u{2502} \x1b[2m{dirname}\x1b[0m{ctx}"
        );
    } else {
        print!(
            "{hoangsa_update}\x1b[2m{model}\x1b[0m \u{2502} \x1b[1m{task}\x1b[0m \u{2502} \x1b[2m{dirname}\x1b[0m{ctx}"
        );
    }
}

fn get_current_task(session: &str) -> String {
    if session.is_empty() {
        return String::new();
    }
    let claude_dir = get_claude_dir();
    let todos_dir = claude_dir.join("todos");
    if !todos_dir.exists() {
        return String::new();
    }

    let mut entries: Vec<(String, std::time::SystemTime)> = fs::read_dir(&todos_dir)
        .ok()
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with(session) && name.contains("-agent-") && name.ends_with(".json")
                })
                .filter_map(|e| {
                    let mtime = e.metadata().ok()?.modified().ok()?;
                    Some((e.file_name().to_string_lossy().to_string(), mtime))
                })
                .collect()
        })
        .unwrap_or_default();

    entries.sort_by(|a, b| b.1.cmp(&a.1));

    if let Some((name, _)) = entries.first() {
        if let Ok(content) = fs::read_to_string(todos_dir.join(name)) {
            if let Ok(todos) = serde_json::from_str::<Vec<Value>>(&content) {
                for todo in &todos {
                    if todo["status"].as_str() == Some("in_progress") {
                        if let Some(form) = todo["activeForm"].as_str() {
                            return form.to_string();
                        }
                    }
                }
            }
        }
    }
    String::new()
}

fn get_update_banner() -> String {
    let claude_dir = get_claude_dir();
    let cache_file = claude_dir.join("cache/hoangsa-update-check.json");
    if let Ok(content) = fs::read_to_string(&cache_file) {
        if let Ok(cache) = serde_json::from_str::<Value>(&content) {
            if cache["update_available"] == true {
                return "\x1b[33m\u{2b06} /hoangsa:update\x1b[0m \u{2502} ".to_string();
            }
        }
    }
    String::new()
}


// ─── check-update ────────────────────────────────────────────────────────────

pub fn cmd_check_update() {
    // Drain stdin
    let _ = read_stdin();

    let home = dirs_home();
    let cwd = std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let claude_dir = get_claude_dir();
    let cache_dir = claude_dir.join("cache");
    let cache_file = cache_dir.join("hoangsa-update-check.json");

    let _ = fs::create_dir_all(&cache_dir);

    // Find VERSION file
    let project_version = Path::new(&cwd).join(".claude/hoangsa/VERSION");
    let global_version = Path::new(&home).join(".claude/hoangsa/VERSION");

    let installed = if project_version.exists() {
        fs::read_to_string(&project_version)
            .unwrap_or_else(|_| "0.0.0".to_string())
            .trim()
            .to_string()
    } else if global_version.exists() {
        fs::read_to_string(&global_version)
            .unwrap_or_else(|_| "0.0.0".to_string())
            .trim()
            .to_string()
    } else {
        "0.0.0".to_string()
    };

    // Check npm for latest version — no shell interpolation (SEC-001 fix)
    let cache_file_clone = cache_file.clone();
    let installed_clone = installed.clone();

    let _ = std::thread::spawn(move || {
        let output = std::process::Command::new("npm")
            .args(["view", "hoangsa-cc", "version"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();

        let latest = match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => return,
        };

        if latest.is_empty() {
            return;
        }

        let update_available = installed_clone != latest;
        let checked = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let json = format!(
            r#"{{"update_available":{},"installed":"{}","latest":"{}","checked":{}}}"#,
            update_available,
            installed_clone.replace('\\', "\\\\").replace('"', "\\\""),
            latest.replace('\\', "\\\\").replace('"', "\\\""),
            checked,
        );

        let _ = fs::write(&cache_file_clone, json);
    });
}

// ─── context-monitor ─────────────────────────────────────────────────────────

const WARNING_THRESHOLD: f64 = 35.0;
const CRITICAL_THRESHOLD: f64 = 25.0;
const STALE_SECONDS: u64 = 60;
const DEBOUNCE_CALLS: u64 = 5;

pub fn cmd_context_monitor() {
    let data = parse_stdin();
    let session_id = match data["session_id"].as_str() {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => return,
    };

    let tmp = std::env::temp_dir();
    let metrics_path = tmp.join(format!("claude-ctx-{session_id}.json"));

    if !metrics_path.exists() {
        return;
    }

    let metrics: Value = match fs::read_to_string(&metrics_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
    {
        Some(v) => v,
        None => return,
    };

    let now = now_unix();

    // Ignore stale metrics
    if let Some(ts) = metrics["timestamp"].as_u64() {
        if now.saturating_sub(ts) > STALE_SECONDS {
            return;
        }
    }

    let remaining = metrics["remaining_percentage"].as_f64().unwrap_or(100.0);
    let used_pct = metrics["used_pct"].as_u64().unwrap_or(0);

    if remaining > WARNING_THRESHOLD {
        return;
    }

    // Debounce
    let warn_path = tmp.join(format!("claude-ctx-{session_id}-warned.json"));
    let mut warn_data: Value = fs::read_to_string(&warn_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({"callsSinceWarn": 0, "lastLevel": null}));

    // Detect /clear: remaining jumped UP by >30% indicates context was cleared
    let last_remaining = warn_data["lastRemainingPct"].as_f64().unwrap_or(0.0);
    if remaining - last_remaining > 30.0 {
        // /clear detected — reset debounce completely
        warn_data = json!({"callsSinceWarn": 0, "lastLevel": null, "lastRemainingPct": remaining});
        let _ = fs::write(&warn_path, warn_data.to_string());
        return; // After clear, context is healthy — no warning needed
    }

    // Track current remaining for next call
    warn_data["lastRemainingPct"] = json!(remaining);

    let first_warn = !warn_path.exists();
    let calls = warn_data["callsSinceWarn"].as_u64().unwrap_or(0) + 1;
    warn_data["callsSinceWarn"] = json!(calls);

    let is_critical = remaining <= CRITICAL_THRESHOLD;
    let current_level = if is_critical { "critical" } else { "warning" };
    let severity_escalated =
        current_level == "critical" && warn_data["lastLevel"].as_str() == Some("warning");

    if !first_warn && calls < DEBOUNCE_CALLS && !severity_escalated {
        let _ = fs::write(&warn_path, warn_data.to_string());
        return;
    }

    // Reset debounce
    warn_data["callsSinceWarn"] = json!(0u64);
    warn_data["lastLevel"] = json!(current_level);
    let _ = fs::write(&warn_path, warn_data.to_string());

    let remaining_u64 = remaining as u64;
    let message = if is_critical {
        format!("CTX CRITICAL {used_pct}% used, {remaining_u64}% left. Claude may hallucinate and become dumb. Wrap up now.")
    } else {
        format!("CTX WARNING {used_pct}% used, {remaining_u64}% left. Avoid new complex work.")
    };

    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "PostToolUse",
            "additionalContext": message
        }
    });
    print!("{output}");
}

// ─── util ────────────────────────────────────────────────────────────────────

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn dirs_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned())
}

fn get_claude_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    PathBuf::from(dirs_home()).join(".claude")
}
