use serde_json::{Value, json};
use std::collections::HashSet;
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

// ─── tracker ─────────────────────────────────────────────────────────────────

const BASH_WRITE_PATTERNS: &[&str] = &[
    ">",
    "mv ",
    "cp ",
    "rm ",
    "touch ",
    "mkdir ",
    "tee ",
    "install ",
    "chmod ",
    "chown ",
    "truncate ",
    "dd ",
    "rsync ",
    "sed -i",
    "npm install",
    "npm ci",
    "npm run",
    "npm build",
    "yarn install",
    "yarn build",
    "yarn add",
    "yarn remove",
    "pip install",
    "git checkout",
    "git reset",
    "git restore",
    "git clean",
    "git merge",
    "git rebase",
    "git commit",
    "git add",
];

fn bash_likely_writes(cmd: &str) -> bool {
    if cmd.is_empty() {
        return false;
    }
    // Check redirect operators
    if cmd.contains('>') {
        return true;
    }
    // Check command patterns (word-boundary aware)
    for pat in BASH_WRITE_PATTERNS {
        if pat == &">" {
            continue;
        }
        // Simple word boundary check: pattern at start or preceded by space/pipe/;/&
        if cmd.starts_with(pat)
            || cmd.contains(&format!(" {pat}"))
            || cmd.contains(&format!("|{pat}"))
            || cmd.contains(&format!(";{pat}"))
            || cmd.contains(&format!("&{pat}"))
        {
            return true;
        }
    }
    false
}

pub fn cmd_tracker() {
    let data = parse_stdin();
    let tool_name = data["tool_name"].as_str().unwrap_or("");
    let tool_input = &data["tool_input"];
    let workspace_dir = data["workspace"]["current_dir"]
        .as_str()
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });

    match tool_name {
        "Write" | "Edit" | "NotebookEdit" | "Bash" => {}
        _ => return,
    }

    // For Bash, only proceed if likely writes files
    if tool_name == "Bash" {
        let command = tool_input["command"].as_str().unwrap_or("");
        if !bash_likely_writes(command) {
            return;
        }
    }

    let gn_dir = Path::new(&workspace_dir).join(".gitnexus");
    let outdated_path = gn_dir.join(".outdated");

    if !gn_dir.exists() {
        let _ = fs::create_dir_all(&gn_dir);
    }

    // Read existing or start fresh
    let mut outdated: Value = if outdated_path.exists() {
        fs::read_to_string(&outdated_path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_else(
                || json!({"marked_at": now_iso(), "changed_files": [], "tool_events": 0}),
            )
    } else {
        json!({"marked_at": now_iso(), "changed_files": [], "tool_events": 0})
    };

    // Extract file paths
    let file_path = match tool_name {
        "Write" | "Edit" => tool_input["file_path"].as_str(),
        "NotebookEdit" => tool_input["notebook_path"].as_str(),
        _ => None,
    };

    // Merge and deduplicate
    let mut file_set: HashSet<String> = outdated["changed_files"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    if let Some(fp) = file_path {
        file_set.insert(fp.to_string());
    }

    // Only write .outdated if there are actual changed files tracked
    // Exception: Bash commands that pass bash_likely_writes() should still
    // mark the index as outdated even without specific file paths, since
    // we know they likely modify files but can't determine which ones.
    let is_bash_write = tool_name == "Bash";
    if file_set.is_empty() && !is_bash_write {
        return;
    }

    let events = outdated["tool_events"].as_u64().unwrap_or(0) + 1;
    outdated["marked_at"] = json!(now_iso());
    outdated["changed_files"] = json!(file_set.into_iter().collect::<Vec<_>>());
    outdated["tool_events"] = json!(events);

    let _ = fs::write(
        &outdated_path,
        serde_json::to_string_pretty(&outdated).unwrap(),
    );
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

    // GitNexus status
    let workspace_path = data["workspace"]["cwd"].as_str().unwrap_or(dir);
    let gn_status = get_gitnexus_status(workspace_path);

    // Output
    let dirname = Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string());

    if task.is_empty() {
        print!(
            "{hoangsa_update}\x1b[2m{model}\x1b[0m \u{2502} \x1b[2m{dirname}\x1b[0m{ctx} \u{2502} {gn_status}"
        );
    } else {
        print!(
            "{hoangsa_update}\x1b[2m{model}\x1b[0m \u{2502} \x1b[1m{task}\x1b[0m \u{2502} \x1b[2m{dirname}\x1b[0m{ctx} \u{2502} {gn_status}"
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

fn get_gitnexus_status(workspace: &str) -> String {
    let gn_dir = Path::new(workspace).join(".gitnexus");
    if !gn_dir.exists() {
        return "\u{26AA} GN: no index".to_string();
    }
    let outdated_file = gn_dir.join(".outdated");
    if outdated_file.exists() {
        if let Ok(content) = fs::read_to_string(&outdated_file) {
            if let Ok(outdated) = serde_json::from_str::<Value>(&content) {
                let count = outdated["changed_files"]
                    .as_array()
                    .map(|a| a.len())
                    .unwrap_or(0);
                // If changed_files is empty, treat as fresh and clean up stale file
                if count == 0 {
                    let _ = fs::remove_file(&outdated_file);
                } else {
                    let duration = outdated["marked_at"]
                        .as_str()
                        .and_then(parse_iso_timestamp)
                        .map(|t| format_duration(now_unix().saturating_sub(t)))
                        .unwrap_or_default();
                    return format!("\u{1f7e1} GN: outdated ({count} files, {duration})");
                }
            } else {
                return "\u{1f7e1} GN: outdated".to_string();
            }
        }
    }
    // Fresh — show age from directory mtime
    if let Ok(meta) = fs::metadata(&gn_dir) {
        if let Ok(mtime) = meta.modified() {
            let age_secs = std::time::SystemTime::now()
                .duration_since(mtime)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            return format!("\u{1f7e2} GN: fresh ({} ago)", format_duration(age_secs));
        }
    }
    "\u{1f7e2} GN: fresh".to_string()
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

fn now_iso() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Simple ISO 8601 without external crate
    let secs = now.as_secs();
    let (y, m, d, h, min, s) = unix_to_ymdhms(secs);
    format!(
        "{y:04}-{m:02}-{d:02}T{h:02}:{min:02}:{s:02}.000Z"
    )
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_to_ymdhms(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    let s = secs % 60;
    let total_min = secs / 60;
    let min = total_min % 60;
    let total_hours = total_min / 60;
    let h = total_hours % 24;
    let mut days = (total_hours / 24) as i64;

    // Compute year/month/day from days since epoch (1970-01-01)
    let mut y: i64 = 1970;
    loop {
        let days_in_year = if is_leap(y) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        y += 1;
    }

    let month_days = if is_leap(y) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut m = 0;
    for md in &month_days {
        if days < *md {
            break;
        }
        days -= md;
        m += 1;
    }

    (y as u64, m as u64 + 1, days as u64 + 1, h, min, s)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

fn parse_iso_timestamp(s: &str) -> Option<u64> {
    // Parse "2026-03-14T15:00:00.000Z" → unix seconds
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let y: u64 = s[0..4].parse().ok()?;
    let m: u64 = s[5..7].parse().ok()?;
    let d: u64 = s[8..10].parse().ok()?;
    let h: u64 = s[11..13].parse().ok()?;
    let min: u64 = s[14..16].parse().ok()?;
    let sec: u64 = s[17..19].parse().ok()?;

    let mut days: u64 = 0;
    for yr in 1970..y {
        days += if is_leap(yr as i64) { 366 } else { 365 };
    }
    let month_days = if is_leap(y as i64) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };
    for item in month_days.iter().take((m as usize - 1).min(11)) {
        days += item;
    }
    days += d - 1;

    Some(days * 86400 + h * 3600 + min * 60 + sec)
}

fn format_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs}s");
    }
    let min = secs / 60;
    if min < 60 {
        return format!("{min}m");
    }
    let hours = min / 60;
    format!("{hours}h")
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
