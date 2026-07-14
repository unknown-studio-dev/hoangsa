use crate::helpers::{out, read_json};
use serde_json::json;
use std::fs;
use std::path::Path;

use super::{enforcement_events_path, find_memory_bin, reflect_sentinel_path};

// ── Frustration Sensor ───────────────────────────────────────────────────────

/// Which class of signal triggered detection.
#[derive(Debug, PartialEq)]
pub(super) enum FrustrationSignal {
    Lexicon(&'static str),
    ExclamationRun,
    CapsRatio,
    ConfigPattern,
}

impl FrustrationSignal {
    fn as_str(&self) -> &str {
        match self {
            FrustrationSignal::Lexicon(p) => p,
            FrustrationSignal::ExclamationRun => "exclamation_run",
            FrustrationSignal::CapsRatio => "caps_ratio",
            FrustrationSignal::ConfigPattern => "config_pattern",
        }
    }
}

/// Vietnamese + English frustration lexicon (word-boundary, checked
/// case-insensitively). Entries are literal strings; we apply `\b`
/// word-boundary anchors only where the term is a standalone word.
static VI_LEXICON: &[&str] = &[
    "đm", "vcl", "vãi", "ngu thế", "óc", "đã bảo", "bao nhiêu lần",
    "lại nữa", "sao mày", "mày bị", "bực", "phẫn nộ",
];
static EN_LEXICON: &[&str] = &[
    "wtf", "ffs", "fucking", "stupid", "i told you", "how many times",
    "stop doing", "again?!",
];

/// Strip markdown code fences from `text` so curse words inside quoted
/// code do not trigger detection.
fn strip_code_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut inside = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            inside = !inside;
            out.push('\n');
            continue;
        }
        if !inside {
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

/// True if `pattern` appears as a word-boundary match in `haystack`
/// (case-insensitive). Multi-word patterns use substring match
/// (word boundary on first word only).
fn lexicon_match(haystack: &str, pattern: &str) -> bool {
    let hl = haystack.to_lowercase();
    let pl = pattern.to_lowercase();
    // Multi-word or punctuation-containing patterns: substring
    if pl.contains(' ') || !pl.chars().all(|c| c.is_alphabetic()) {
        return hl.contains(&pl);
    }
    // Single word: require word boundary (not adjacent to alphanumeric)
    let mut start = 0;
    while let Some(pos) = hl[start..].find(&pl) {
        let abs = start + pos;
        let before_ok = abs == 0
            || !hl.as_bytes().get(abs - 1).copied().unwrap_or(b' ').is_ascii_alphanumeric();
        let after_ok = abs + pl.len() >= hl.len()
            || !hl.as_bytes()
                .get(abs + pl.len())
                .copied()
                .unwrap_or(b' ')
                .is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

/// Pure frustration detector. Returns `Some(signal_class)` when the
/// prompt text exhibits frustration, `None` otherwise.
///
/// Input: raw prompt text.
/// Extra patterns: substring-match strings from config.
///
/// Edge cases:
///   - Text inside ``` code fences is ignored.
///   - "OK!!" must NOT trigger (≥3 consecutive `!` or `?` on prompts
///     with ≥5 alphabetic characters only — guards "OK!!").
pub(super) fn detect_frustration(
    text: &str,
    extra_patterns: &[String],
) -> Option<FrustrationSignal> {
    let stripped = strip_code_fences(text);
    let s = stripped.as_str();

    // Lexicon checks
    for &pat in VI_LEXICON.iter().chain(EN_LEXICON.iter()) {
        if lexicon_match(s, pat) {
            return Some(FrustrationSignal::Lexicon(pat));
        }
    }
    // Config extra patterns (substring)
    for pat in extra_patterns {
        if !pat.is_empty() && s.to_lowercase().contains(&pat.to_lowercase()) {
            return Some(FrustrationSignal::ConfigPattern);
        }
    }

    // Heuristic: ≥3 consecutive '!' or '?' — only on prompts with ≥5
    // alphabetic chars (guards "OK!!")
    let alpha_count = s.chars().filter(|c| c.is_alphabetic()).count();
    if alpha_count >= 5 {
        let mut run = 0u32;
        let mut prev = '\0';
        for ch in s.chars() {
            if ch == '!' || ch == '?' {
                if ch == prev { run += 1; } else { run = 1; }
                if run >= 3 {
                    return Some(FrustrationSignal::ExclamationRun);
                }
            } else {
                run = 0;
            }
            prev = ch;
        }
    }

    // Heuristic: caps ratio >0.6 on prompts with ≥20 alphabetic chars
    if alpha_count >= 20 {
        let upper = s.chars().filter(|c| c.is_uppercase()).count();
        if upper as f64 / alpha_count as f64 > 0.6 {
            return Some(FrustrationSignal::CapsRatio);
        }
    }

    None
}

/// Load `enforcement.frustration_patterns` from `.hoangsa/config.json`.
fn load_extra_patterns(cwd: &str) -> Vec<String> {
    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    if !config_path.exists() {
        return vec![];
    }
    let cfg = read_json(config_path.to_str().unwrap_or(""));
    cfg.get("enforcement")
        .and_then(|e| e.get("frustration_patterns"))
        .and_then(|p| p.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

/// `hook prompt-guard`
///
/// UserPromptSubmit hook: reads the hook payload JSON from stdin,
/// extracts the prompt text, runs the frustration detector, and emits:
///   - If frustration detected: append_event + hookSpecificOutput advice.
///   - Otherwise: `{}` (no-op, never blocks).
pub fn cmd_prompt_guard(cwd: &str) {
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let parsed: serde_json::Value =
        serde_json::from_str(input.trim()).unwrap_or(json!({}));

    // Extract prompt text: try .prompt, then .user_prompt, then raw string
    let prompt_text = parsed
        .get("prompt")
        .and_then(|v| v.as_str())
        .or_else(|| parsed.get("user_prompt").and_then(|v| v.as_str()))
        .or_else(|| parsed.as_str())
        .unwrap_or("")
        .to_string();

    if prompt_text.is_empty() {
        print!("{{}}");
        return;
    }

    let extra = load_extra_patterns(cwd);
    if let Some(signal) = detect_frustration(&prompt_text, &extra) {
        let digest: String = prompt_text.chars().take(200).collect();
        use super::events::append_event;
        append_event(
            cwd,
            &json!({
                "event": "frustration",
                "signal": signal.as_str(),
                "digest": digest,
            }),
        );
        let advice = "User frustration detected. Before proceeding: (1) identify which pattern caused this — check LESSONS.md for any existing lesson that was violated; (2) if a lesson was violated, call memory_remember_lesson NOW with a concrete trigger+advice pair; (3) repair the issue rather than apologising.";
        out(&json!({
            "hookSpecificOutput": {
                "hookEventName": "UserPromptSubmit",
                "additionalContext": advice,
            }
        }));
    } else {
        print!("{{}}");
    }
}

/// `hook stop-check [sessions_dir]`
///
/// Deterministic workflow-completion check for the Claude Code Stop hook.
/// Replaces the fragile prompt-type hook that couldn't distinguish
/// fix/research/audit sessions from menu sessions.
///
/// Logic:
///   - status="cooking" + plan.json has pending/running tasks → approve-with-warning
///   - session did real work (enforcement.events non-empty) + no sentinel +
///     stop_hook_active=false → block with memory-reflect prompt, write sentinel
///   - Everything else → approve
///
/// Archival NOT triggered here — Stop fires every turn and the
/// `is_ingested` short-circuit would skip all but the first fire,
/// leaving most of the session unarchived. Archival lives on PreCompact
/// and SessionEnd (see `cmd_session_archive`) where each fire does real
/// work.
pub fn cmd_stop_check(sessions_dir: Option<&str>, cwd: &str) {
    // Drain stdin once — Claude Code pipes the Stop payload here.
    // `read_to_string` returns at EOF, which Claude Code closes after sending.
    let mut stdin_raw = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut stdin_raw);

    let dir = sessions_dir.map(|s| s.to_string()).unwrap_or_else(|| {
        Path::new(cwd)
            .join(".hoangsa")
            .join("sessions")
            .to_string_lossy()
            .to_string()
    });

    if let Some(session_dir) = find_latest_session(&dir) {
        let state_path = Path::new(&session_dir).join("state.json");
        if state_path.exists() {
            let state = read_json(state_path.to_str().unwrap_or(""));
            if state.get("error").is_none() && state["status"].as_str() == Some("cooking") {
                let plan_path = Path::new(&session_dir).join("plan.json");
                if plan_path.exists() {
                    let plan = read_json(plan_path.to_str().unwrap_or(""));
                    if plan.get("error").is_none() {
                        let pending = count_incomplete_tasks(&plan);
                        if pending > 0 {
                            out(&json!({
                                "decision": "approve",
                                "reason": format!(
                                    "⚠️ HOANGSA: Workflow incomplete — {} task(s) still pending/running in session {}. You MUST complete all tasks before finishing. If you need user input, ask and then continue working.",
                                    pending,
                                    state["session_id"].as_str().unwrap_or("unknown")
                                )
                            }));
                            return;
                        }
                    }
                }
            }
        }
    }

    match evaluate_reflect_prompt(cwd, &stdin_raw) {
        ReflectOutcome::Skip => out(&json!({"decision": "approve"})),
        ReflectOutcome::Prompt(reason) => out(&json!({
            "decision": "block",
            "reason": reason,
        })),
    }
}

/// Reason text injected into the Stop hook when the session did real work
/// but the agent hasn't reflected yet. Surfaces as a system message the
/// agent must respond to before the conversation can terminate.
const REFLECT_REASON: &str = "HOANGSA MEMORY: Before stopping, invoke the `memory-reflect` skill to distill durable learnings from this session into `memory_remember_fact` / `memory_remember_lesson` / `memory_remember_preference`. The skill contains the decision checklist. If nothing is worth persisting, briefly say so and stop.";

pub(super) enum ReflectOutcome {
    /// No prompt needed — approve the Stop.
    Skip,
    /// Block the Stop and inject `reason` as a system message so the
    /// agent runs memory-reflect before terminating.
    Prompt(String),
}

/// Pure-ish decision for the reflect prompt. Writes the sentinel as a
/// side effect when it returns `Prompt` so the next Stop in this session
/// short-circuits to `Skip`.
pub(super) fn evaluate_reflect_prompt(cwd: &str, stdin_raw: &str) -> ReflectOutcome {
    let payload: serde_json::Value =
        serde_json::from_str(stdin_raw.trim()).unwrap_or(json!({}));

    // Claude Code sets stop_hook_active=true while it is already continuing
    // from a previous Stop-hook block. Re-blocking here would loop forever.
    // Check this FIRST — before frustration escalation — so the loop guard
    // always wins regardless of event log state.
    let stop_hook_active = payload
        .get("stop_hook_active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if stop_hook_active {
        return ReflectOutcome::Skip;
    }

    let sentinel = reflect_sentinel_path(cwd);
    if sentinel.exists() {
        return ReflectOutcome::Skip;
    }

    // `state-clear` wipes enforcement.events at SessionStart, so a
    // non-empty file means the agent did impact/recall/detect_changes or
    // an Edit/Write that produced a drift event this session. That's the
    // cheapest "real work happened" signal available without reading
    // episodes.db or shelling out to git.
    let has_work = fs::metadata(enforcement_events_path(cwd))
        .map(|m| m.len() > 0)
        .unwrap_or(false);
    if !has_work {
        return ReflectOutcome::Skip;
    }

    // Read the event log to check for frustration escalation and
    // collect lesson_surfaced triggers for feedback.
    let events_text = fs::read_to_string(enforcement_events_path(cwd))
        .unwrap_or_default();
    let (has_frustration, lesson_saved_after, surfaced_triggers) =
        parse_stop_events(&events_text);

    // Frustration escalation: if a frustration event exists with NO
    // lesson_saved event after it, block and demand lesson distillation.
    if has_frustration && !lesson_saved_after {
        // Write sentinel so the next Stop (stop_hook_active=true path above)
        // approves — but do NOT write it here; the agent must save a lesson
        // first. We intentionally do NOT write the sentinel so the block
        // can recur until the agent persists a lesson. The loop guard is the
        // stop_hook_active check at the very top.
        return ReflectOutcome::Prompt(
            "HOANGSA FRUSTRATION BLOCK: A frustration signal was detected in this session \
             and no lesson has been saved yet. Before stopping, call \
             `memory_remember_lesson` with a concrete trigger and advice pair that \
             explains what went wrong. Repair over apology.".to_string(),
        );
    }

    // Emit lesson_surfaced event when we have surfaced triggers (best-effort).
    if !surfaced_triggers.is_empty() {
        use super::events::append_event;
        append_event(
            cwd,
            &json!({
                "event": "lesson_surfaced",
                "triggers": surfaced_triggers.clone(),
            }),
        );

        // Best-effort feedback to hoangsa-memory for each trigger.
        // Binary missing or subcommand absent → silently skip, never panic.
        // Interface: hoangsa-memory --root <root> memory lesson-feedback success|failure <TRIGGER>...
        if let Some(memory_bin) = find_memory_bin() {
            let memory_root = Path::new(cwd).join(".hoangsa").join("memory");
            let kind = if has_frustration { "failure" } else { "success" };
            for trigger in &surfaced_triggers {
                let _ = std::process::Command::new(&memory_bin)
                    .args(["--root", &memory_root.to_string_lossy()])
                    .args(["memory", "lesson-feedback", kind, trigger.as_str()])
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    }

    if let Some(parent) = sentinel.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&sentinel, "");

    ReflectOutcome::Prompt(REFLECT_REASON.to_string())
}

/// Scan the JSONL event log and return:
///   (has_frustration, lesson_saved_after_last_frustration, lesson_surfaced_triggers)
///
/// `lesson_saved_after` is true when a `lesson_saved` event appears
/// anywhere after the most recent `frustration` event.
/// `surfaced_triggers` are trigger texts from `lesson_surfaced` events.
pub(super) fn parse_stop_events(events_text: &str) -> (bool, bool, Vec<String>) {
    let mut last_frustration_pos: Option<usize> = None;
    let mut lesson_saved_positions: Vec<usize> = Vec::new();
    let mut surfaced_triggers: Vec<String> = Vec::new();

    for (pos, line) in events_text.lines().enumerate() {
        let Ok(entry) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        match entry.get("event").and_then(|e| e.as_str()) {
            Some("frustration") => {
                last_frustration_pos = Some(pos);
            }
            Some("lesson_saved") => {
                lesson_saved_positions.push(pos);
            }
            Some("lesson_surfaced") => {
                if let Some(triggers) = entry.get("triggers").and_then(|t| t.as_array()) {
                    for t in triggers {
                        if let Some(s) = t.as_str() {
                            surfaced_triggers.push(s.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let has_frustration = last_frustration_pos.is_some();
    let lesson_saved_after = last_frustration_pos
        .map(|fp| lesson_saved_positions.iter().any(|&lp| lp > fp))
        .unwrap_or(false);

    (has_frustration, lesson_saved_after, surfaced_triggers)
}

/// `hook lesson-guard`
///
/// PreToolUse hook for Edit/Write. Reads stdin JSON, extracts file_path,
/// calls `hoangsa-memory recall` to find relevant lessons/facts, surfaces them.
/// If a recalled lesson contains "NEVER" + a path fragment that matches
/// the file being edited → block. Otherwise → approve with context shown.
pub fn cmd_lesson_guard(cwd: &str) {
    use std::io::Read;
    use std::process::Command;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let parsed: serde_json::Value =
        serde_json::from_str(&input).unwrap_or(json!({}));

    let file_path = parsed
        .get("tool_input")
        .and_then(|ti| ti.get("file_path"))
        .and_then(|fp| fp.as_str())
        .unwrap_or("");

    if file_path.is_empty() {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Build a query from the file path — extract meaningful path segments
    let query = build_recall_query(file_path);
    if query.is_empty() {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Resolve the memory root the same way the rest of the system does —
    // local .hoangsa/memory when populated, else the global per-slug root.
    // A hardcoded local path silently no-ops the guard on every project
    // migrated to ~/.hoangsa/memory/projects/<slug>/.
    let Some(memory_root) = hoangsa_memory_root(cwd).filter(|r| r.exists()) else {
        out(&json!({"decision": "approve"}));
        return;
    };

    // Call hoangsa-memory CLI to recall lessons relevant to this file path
    let memory_bin = find_memory_bin();
    let memory_bin = match memory_bin {
        Some(b) => b,
        None => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let result = Command::new(&memory_bin)
        .args(["--root", &memory_root.to_string_lossy()])
        .args(["query", &query, "--top-k", "8", "--json"])
        .output();

    let output_bytes = match result {
        Ok(o) => o.stdout,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let recall: serde_json::Value = match serde_json::from_slice(&output_bytes) {
        Ok(v) => v,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let chunks = match recall.get("chunks").and_then(|c| c.as_array()) {
        Some(c) => c,
        None => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    // Filter to only LESSONS.md and MEMORY.md chunks
    let lessons: Vec<&str> = chunks
        .iter()
        .filter(|c| {
            let path = c.get("path").and_then(|p| p.as_str()).unwrap_or("");
            path == "LESSONS.md" || path == "MEMORY.md"
        })
        .filter_map(|c| {
            // The query CLI strips bodies by default (preview carries the
            // text) — an empty body here would hollow out both the NEVER
            // check and the advisory context.
            c.get("body")
                .and_then(|b| b.as_str())
                .filter(|s| !s.is_empty())
                .or_else(|| c.get("preview").and_then(|p| p.as_str()))
        })
        .collect();

    if lessons.is_empty() {
        out(&json!({"decision": "approve"}));
        return;
    }

    // Check: does any lesson say "NEVER" + contain a path fragment matching file_path?
    let fp_lower = file_path.to_lowercase();
    let mut blocking_lesson: Option<&str> = None;

    for lesson in &lessons {
        let lesson_lower = lesson.to_lowercase();
        if !lesson_lower.contains("never") {
            continue;
        }
        // Find "NEVER" in the lesson, then extract path fragments from
        // the text between "NEVER" and the next "—" or sentence end.
        // This avoids matching paths in the "do this instead" advice part.
        if let Some(never_pos) = lesson_lower.find("never") {
            let after_never = &lesson[never_pos..];
            // Take text up to next "—" or "Always" or end
            let end_pos = after_never.find(" — ")
                .or_else(|| after_never.find(". Always"))
                .or_else(|| after_never.find(". The"))
                .unwrap_or(after_never.len());
            let never_clause = &after_never[..end_pos];

            for word in never_clause.split_whitespace() {
                let clean = word.trim_matches(|c: char| {
                    !c.is_alphanumeric() && c != '/' && c != '.' && c != '-' && c != '_'
                }).trim_matches('`');
                if clean.contains('/') && clean.len() > 2 && fp_lower.contains(&clean.to_lowercase()) {
                    blocking_lesson = Some(lesson);
                    break;
                }
            }
        }
        if blocking_lesson.is_some() {
            break;
        }
    }

    // Check if file is gitignored — adds context to the decision
    let is_gitignored = Command::new("git")
        .args(["check-ignore", "-q", file_path])
        .current_dir(cwd)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    let all_lessons_text = lessons.join("\n---\n");

    // Emit lesson_surfaced event for the triggers we're about to surface.
    {
        use super::events::append_event;
        let triggers: Vec<String> = lessons
            .iter()
            .map(|l| l.lines().next().unwrap_or("").trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !triggers.is_empty() {
            append_event(
                cwd,
                &json!({
                    "event": "lesson_surfaced",
                    "triggers": triggers,
                }),
            );
        }
    }

    if let Some(lesson) = blocking_lesson {
        // Hard-block when editing an installed-copy path that a NEVER lesson
        // warns about. Previously this only surfaced a warning and approved —
        // which let the agent override the lesson (happened 5+ times). The
        // block condition is deterministic: NEVER-lesson match + gitignored +
        // path sits under a known installed-copy prefix.
        let fp = file_path;
        let is_installed_copy_path = fp.contains("/.claude/hoangsa/")
            || fp.contains("/.claude/skills/")
            || fp.contains("/.claude/commands/")
            || fp.contains("/.claude/agents/");
        let should_block = is_gitignored && is_installed_copy_path;

        if should_block {
            out(&json!({
                "decision": "block",
                "reason": format!(
                    "BLOCKED: '{}' is a gitignored installed-copy path and matches a NEVER lesson.\n\nLesson:\n{}\n\nEdit the source under templates/ instead, then run bin/install to sync.\n\nIf this is intentional (rare), tell the user to override explicitly.",
                    file_path, lesson
                )
            }));
        } else {
            let gitignore_note = if is_gitignored {
                "\nNote: This file is in .gitignore — it may be an installed copy, not the source."
            } else {
                ""
            };
            out(&json!({
                "decision": "approve",
                "reason": format!(
                    "⚠️ LESSON GUARD for '{}':{}\n\nRelevant lesson:\n{}\n\n---\nAll recalled lessons:\n{}\n\nIf this edit is intentional, proceed. If not, find the correct source file.",
                    file_path, gitignore_note, lesson, all_lessons_text
                )
            }));
        }
    } else if !lessons.is_empty() {
        // No blocking lesson, but surface lessons as context
        out(&json!({
            "decision": "approve",
            "reason": format!(
                "Lessons for '{}':\n{}",
                file_path, all_lessons_text
            )
        }));
    } else {
        out(&json!({"decision": "approve"}));
    }
}

/// Build a recall query from a file path.
/// Keeps path structure intact so hoangsa-memory can match lessons mentioning paths.
pub(super) fn build_recall_query(path: &str) -> String {
    // Strip home dir prefix for cleaner query
    let clean = if let Ok(home) = std::env::var("HOME") {
        path.strip_prefix(&home).unwrap_or(path)
    } else {
        path
    };
    // Strip leading project dir — keep from first recognizable segment
    let clean = clean.trim_start_matches('/');
    // Keep path-like structure so ".claude/hoangsa" or "templates/" matches
    format!("NEVER edit {clean}")
}

/// Fire-and-forget archive ingest so the current transcript (including
/// any growth since last ingest) lands in the archive. Runs fully
/// detached from the caller (PreCompact / SessionEnd hook) so the
/// user's session never stalls. Retention trimming runs inside the
/// target process.
///
/// Routing:
///   1. If an MCP daemon socket is reachable (at `<root>/mcp.sock`),
///      send a `memory_archive_ingest` call over it. The daemon runs
///      the ingest in its own process, reusing its lazy-initialised
///      ChromaDB Python sidecar.
///   2. Otherwise, spawn a detached `hoangsa-memory archive ingest
///      --refresh` subprocess (old behaviour). The advisory flock in
///      `cmd_archive_ingest` serialises concurrent subprocesses so we
///      still only boot one sidecar at a time.
///
/// The daemon path is the big win — previously every PreCompact /
/// SessionEnd hook fire spawned a fresh ~500 MB Python sidecar, and
/// concurrent Claude Code sessions would pile them up and OOM the
/// machine. Forwarding to the running daemon keeps the sidecar count
/// at one.
///
/// Rate-limit: `~/.hoangsa/memory/archive-ingest.last` is touched after
/// every dispatch; if the previous stamp is younger than
/// `INGEST_COOLDOWN_SECS` we skip entirely. A single Claude Code
/// session can fire PreCompact + SessionEnd within seconds of each
/// other, and multiple concurrent sessions amplify that — without this
/// cooldown, dispatches pile up faster than the daemon or advisory
/// flock can drain them. That pile-up is what preceded the 164GB
/// disk-fill incident recorded in RESEARCH.md.
const INGEST_COOLDOWN_SECS: u64 = 60;

fn spawn_archive_ingest() {
    if !cooldown_elapsed() {
        return;
    }
    let dispatched = if try_forward_to_daemon() {
        true
    } else {
        spawn_detached_ingest()
    };
    if dispatched {
        touch_cooldown_stamp();
    }
}

fn spawn_detached_ingest() -> bool {
    use std::process::{Command, Stdio};
    let Some(bin) = find_memory_bin() else {
        return false;
    };
    Command::new(bin)
        .args(["archive", "ingest", "--refresh"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn cooldown_stamp_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(
        std::path::PathBuf::from(home)
            .join(".hoangsa")
            .join("memory")
            .join("archive-ingest.last"),
    )
}

fn cooldown_elapsed() -> bool {
    let Some(path) = cooldown_stamp_path() else {
        return true;
    };
    let Ok(meta) = std::fs::metadata(&path) else {
        return true;
    };
    let Ok(mtime) = meta.modified() else {
        return true;
    };
    match mtime.elapsed() {
        Ok(dur) => dur.as_secs() >= INGEST_COOLDOWN_SECS,
        Err(_) => true,
    }
}

fn touch_cooldown_stamp() {
    let Some(path) = cooldown_stamp_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path);
}

/// Try to send a `memory_archive_ingest` request to a running MCP
/// daemon. Returns `true` iff the request was written AND the daemon
/// replied within the short timeout.
///
/// We wait for the reply on purpose: a bare "connect + write" can
/// succeed even when the daemon is wedged, which would silently skip
/// the subprocess fallback. Waiting for the one-line JSON-RPC response
/// gives us a real liveness signal. The timeout is short (2s) because
/// this runs inside a hook and we don't want to stall the user's
/// session when the daemon is unresponsive.
fn try_forward_to_daemon() -> bool {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    const DAEMON_TIMEOUT: Duration = Duration::from_secs(2);

    let Some(sock_path) = candidate_mcp_socket() else {
        return false;
    };

    let mut stream = match UnixStream::connect(&sock_path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    // Hard wall-clock on both halves of the conversation. Without these,
    // a half-wedged daemon could block the hook for the kernel default
    // socket timeout (effectively forever).
    let _ = stream.set_read_timeout(Some(DAEMON_TIMEOUT));
    let _ = stream.set_write_timeout(Some(DAEMON_TIMEOUT));

    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "hoangsa-memory.call",
        "params": {
            "name": "memory_archive_ingest",
            "arguments": { "refresh": true }
        }
    });
    let mut line = match serde_json::to_string(&request) {
        Ok(s) => s,
        Err(_) => return false,
    };
    line.push('\n');

    if stream.write_all(line.as_bytes()).is_err() {
        return false;
    }
    if stream.flush().is_err() {
        return false;
    }

    // One-line JSON-RPC response. We don't inspect it — any reply is a
    // liveness signal. On timeout / EOF we return false and let the
    // caller fall back to the subprocess path.
    let mut reader = BufReader::new(stream);
    let mut buf = String::new();
    matches!(reader.read_line(&mut buf), Ok(n) if n > 0)
}

/// Locate an MCP daemon socket. Tries the local `.hoangsa/memory/` in
/// the current working directory first, then the global
/// `~/.hoangsa/memory/projects/<slug>/` layout (mirroring the resolver
/// in `hoangsa-memory-mcp::main`).
fn candidate_mcp_socket() -> Option<std::path::PathBuf> {
    let cwd = std::env::current_dir().ok()?;

    // Local root
    let local = cwd.join(".hoangsa").join("memory").join("mcp.sock");
    if local.exists() {
        return Some(local);
    }

    // Global root — readable-slug layout: last two cwd components,
    // lowercased, non-alnum → '-'. Matches `hoangsa-memory-mcp::main::project_slug`.
    let home = std::env::var_os("HOME")?;
    let slug = project_slug(&cwd);
    let global = std::path::PathBuf::from(home)
        .join(".hoangsa")
        .join("memory")
        .join("projects")
        .join(slug)
        .join("mcp.sock");
    if global.exists() {
        return Some(global);
    }
    None
}

use hoangsa_memory_core::project_slug;

/// `hook session-archive`
///
/// Trigger for the PreCompact and SessionEnd hooks. Spawns a detached
/// `hoangsa-memory archive ingest --refresh`, emits an `approve`
/// decision, and returns. Claude Code's hook interface expects a
/// decision on stdout even when the hook is purely a side-effect.
pub fn cmd_session_archive() {
    spawn_archive_ingest();
    out(&json!({"decision": "approve"}));
}

/// `hook session-start`
///
/// Fires on Claude Code SessionStart. Two responsibilities:
///
/// 1. Decide whether the project needs a one-shot post-install bootstrap
///    (source index + archive ingest + memory skeleton seed) and spawn a
///    detached worker if so.
/// 2. Emit `hookSpecificOutput.additionalContext` with the current
///    USER.md + MEMORY.md + LESSONS.md content so the agent sees
///    preferences / facts / lessons at the top of every session. Previously
///    the docs claimed this happened but no code path did it.
///
/// MUST return in <100 ms — opt-out checks + sentinel read + spawn are
/// all pure file-system ops. Failures (no memory bin, HOME unset,
/// spawn error) degrade gracefully: we emit `approve` and skip, never
/// block the session. Rationale in
/// `.hoangsa/sessions/brainstorm/post-install-onboarding/BRAINSTORM.md`.
pub fn cmd_session_start(cwd: &str) {
    use crate::cmd::bootstrap;
    let project = std::path::Path::new(cwd);
    let reason = match bootstrap::should_bootstrap(project) {
        Ok(()) => {
            if bootstrap::spawn_detached_worker(project) {
                "spawned"
            } else {
                "spawn_failed"
            }
        }
        Err(r) => {
            let _ = r;
            "skipped"
        }
    };

    let additional_context = hoangsa_memory_root(cwd)
        .as_deref()
        .and_then(compose_session_start_context);

    let mut response = json!({"decision": "approve", "bootstrap": reason});
    if let Some(ctx) = additional_context {
        response["hookSpecificOutput"] = json!({
            "hookEventName": "SessionStart",
            "additionalContext": ctx,
        });
    }
    out(&response);
}

/// Resolve the same memory root the MCP server uses.
///
/// Always returns `Some(_)` — `compose_session_start_context` handles
/// missing/empty files by returning `None`, so we don't need to gate here.
fn hoangsa_memory_root(cwd: &str) -> Option<std::path::PathBuf> {
    Some(hoangsa_memory_core::resolve_root(Path::new(cwd), None))
}

/// Read `USER.md` + `MEMORY.md` + `LESSONS.md` from the memory root and
/// compose them into a single `additionalContext` blob for the
/// SessionStart hook. Returns `None` when all three files are missing or
/// empty — we don't want to inject a header-only section.
pub(super) fn compose_session_start_context(root: &Path) -> Option<String> {
    let surfaces = [
        ("USER.md", "user preferences"),
        ("MEMORY.md", "project facts"),
        ("LESSONS.md", "project lessons"),
    ];

    let mut body = String::new();
    let mut any = false;
    for (filename, label) in surfaces {
        let Ok(content) = fs::read_to_string(root.join(filename)) else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }
        any = true;
        body.push_str(&format!(
            "─── {filename} ({label}) ───\n{}\n\n",
            content.trim_end()
        ));
    }
    if !any {
        return None;
    }
    Some(format!(
        "## hoangsa-memory (auto-injected at SessionStart)\n\n{body}"
    ))
}

/// Count tasks with status other than "completed", "done", "skipped".
pub(super) fn count_incomplete_tasks(plan: &serde_json::Value) -> usize {
    let tasks = match plan["tasks"].as_array() {
        Some(t) => t,
        None => return 0,
    };

    tasks
        .iter()
        .filter(|t| {
            let s = t["status"].as_str().unwrap_or("pending");
            !matches!(s, "completed" | "done" | "skipped" | "failed")
        })
        .count()
}

/// Find the most recently modified session directory.
fn find_latest_session(sessions_root: &str) -> Option<String> {
    let root = Path::new(sessions_root);
    let type_dirs = fs::read_dir(root).ok()?;

    // Reuse the canonical list from `session.rs` so hook routing stays in
    // sync with `session init` / `collect_sessions`. A divergent local
    // list drops brainstorm sessions on the floor (writes nothing, or
    // worse, writes to an older non-brainstorm session via mtime).
    let mut best: Option<(std::time::SystemTime, String)> = None;

    for type_entry in type_dirs.filter_map(|e| e.ok()) {
        let ft = type_entry.file_type().ok()?;
        if !ft.is_dir() {
            continue;
        }
        let type_name = type_entry.file_name().into_string().ok()?;
        if !crate::cmd::session::KNOWN_TYPES.contains(&type_name.as_str()) {
            continue;
        }

        let name_dirs = match fs::read_dir(type_entry.path()) {
            Ok(d) => d,
            Err(_) => continue,
        };

        for name_entry in name_dirs.filter_map(|e| e.ok()) {
            if !name_entry
                .file_type()
                .map(|ft| ft.is_dir())
                .unwrap_or(false)
            {
                continue;
            }
            let mtime = name_entry
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);

            if best.as_ref().is_none_or(|(t, _)| mtime > *t) {
                best = Some((mtime, name_entry.path().to_string_lossy().to_string()));
            }
        }
    }

    best.map(|(_, path)| path)
}

// ── Session token-usage instrumentation ──────────────────────────────────────

/// Aggregate Anthropic usage counters across a Claude Code transcript.
#[derive(Default, Clone, Copy)]
pub(super) struct UsageTotals {
    pub(super) input: u64,
    pub(super) output: u64,
    pub(super) cache_read: u64,
    pub(super) cache_creation: u64,
    pub(super) turns: u64,
}

impl UsageTotals {
    pub(super) fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }
}

/// Sum `message.usage` fields across all assistant lines in a transcript JSONL.
pub(super) fn tally_transcript(transcript_path: &Path) -> Option<UsageTotals> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(transcript_path).ok()?;
    let reader = BufReader::new(file);
    let mut t = UsageTotals::default();
    for line in reader.lines().map_while(Result::ok) {
        let v: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("type").and_then(|s| s.as_str()) != Some("assistant") {
            continue;
        }
        let Some(usage) = v.get("message").and_then(|m| m.get("usage")) else {
            continue;
        };
        let get = |k: &str| usage.get(k).and_then(|n| n.as_u64()).unwrap_or(0);
        t.input += get("input_tokens");
        t.output += get("output_tokens");
        t.cache_read += get("cache_read_input_tokens");
        t.cache_creation += get("cache_creation_input_tokens");
        t.turns += 1;
    }
    Some(t)
}

/// `hook session-usage`
///
/// Fires on Claude Code Stop. Reads transcript_path from stdin, sums up
/// token usage across all assistant messages, writes
/// `$SESSION_DIR/usage.json` for the latest active session under cwd.
///
/// Best-effort — never blocks the turn:
///   - No latest session → skip silently.
///   - No transcript or malformed lines → skip silently.
///   - Write failure → skip silently.
///
/// The file is rewritten (idempotent) every turn because Stop fires once
/// per turn and the transcript grows monotonically.
pub fn cmd_session_usage(cwd: &str) {
    use std::io::Read as _;
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    let parsed: serde_json::Value = serde_json::from_str(&input).unwrap_or(json!({}));

    let approve = || out(&json!({"decision": "approve"}));

    let transcript_path = parsed
        .get("transcript_path")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if transcript_path.is_empty() {
        approve();
        return;
    }

    let effective_cwd = parsed
        .get("cwd")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(cwd);

    let sessions_root = Path::new(effective_cwd)
        .join(".hoangsa")
        .join("sessions")
        .to_string_lossy()
        .to_string();
    let Some(session_dir) = find_latest_session(&sessions_root) else {
        approve();
        return;
    };

    let Some(totals) = tally_transcript(Path::new(transcript_path)) else {
        approve();
        return;
    };

    // Read session_id from state.json if present — useful for cross-referencing.
    let state_path = Path::new(&session_dir).join("state.json");
    let session_id = if state_path.exists() {
        let v = read_json(state_path.to_str().unwrap_or(""));
        v.get("session_id")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };

    let payload = json!({
        "session_id": session_id,
        "transcript_path": transcript_path,
        "updated_at": now_iso_for_usage(),
        "turns": totals.turns,
        "input_tokens": totals.input,
        "output_tokens": totals.output,
        "cache_read_tokens": totals.cache_read,
        "cache_creation_tokens": totals.cache_creation,
        "total_tokens": totals.total(),
    });

    let usage_path = Path::new(&session_dir).join("usage.json");
    let _ = fs::write(
        &usage_path,
        serde_json::to_string_pretty(&payload).unwrap_or_default(),
    );

    approve();
}

/// ISO-8601 timestamp for usage.json. Separate from the oneliner in
/// `state.rs` so hook.rs keeps a single time-formatting helper.
fn now_iso_for_usage() -> String {
    use time::OffsetDateTime;
    use time::macros::format_description;
    OffsetDateTime::now_utc()
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
        .unwrap_or_default()
}

#[cfg(test)]
mod session_tests {
    use super::*;

    // ── Test 1: prompt_guard_detects_vi_and_en_frustration ───────────────────

    #[test]
    fn prompt_guard_detects_vi_and_en_frustration() {
        // Vietnamese multi-word: "đã bảo bao nhiêu lần rồi" contains "đã bảo"
        let result = detect_frustration("đã bảo bao nhiêu lần rồi", &[]);
        assert!(result.is_some(), "đã bảo bao nhiêu lần rồi must detect frustration");

        // English: "wtf again" matches "wtf" in lexicon
        let result = detect_frustration("wtf again", &[]);
        assert!(result.is_some(), "wtf again must detect frustration");

        // All-caps Vietnamese: "SAO MÀY NGU THẾ" — ≥20 alpha chars with caps ratio >0.6
        // or lexicon hit via "sao mày" / "ngu thế"
        let result = detect_frustration("SAO MÀY NGU THẾ", &[]);
        assert!(result.is_some(), "SAO MÀY NGU THẾ must detect frustration");
    }

    // ── Test 2: prompt_guard_ignores_normal_prompts ──────────────────────────

    #[test]
    fn prompt_guard_ignores_normal_prompts() {
        // Ordinary technical prompt
        let result = detect_frustration("Please fix the null pointer in auth.rs line 42", &[]);
        assert!(result.is_none(), "ordinary technical prompt must not trigger detection");

        // Sketchy word inside a code fence must be stripped
        let fenced = "Here is the function:\n```\nfn stupid_var() { }\n```\nPlease review.";
        let result = detect_frustration(fenced, &[]);
        assert!(result.is_none(), "lexicon hit inside ``` fence must not trigger; got: {result:?}");

        // "OK!!" — only two exclamation marks, alpha_count < 5
        let result = detect_frustration("OK!!", &[]);
        assert!(result.is_none(), "OK!! must not trigger (length guard)");
    }

    // ── Test 3: stop_check_blocks_frustration_without_lesson ─────────────────

    #[test]
    fn stop_check_blocks_frustration_without_lesson() {
        use std::fs;

        // ── Case A: frustration with no lesson_saved → block ─────────────────
        {
            let tmp = tempfile::tempdir().expect("tempdir");
            let cwd = tmp.path();
            // Write a non-empty events file with a frustration event but no lesson_saved
            let events_path = cwd
                .join(".hoangsa")
                .join("state")
                .join("enforcement.events");
            fs::create_dir_all(events_path.parent().unwrap()).unwrap();
            fs::write(
                &events_path,
                "{\"event\":\"impact\"}\n{\"event\":\"frustration\",\"signal\":\"wtf\"}\n",
            )
            .unwrap();

            let outcome = evaluate_reflect_prompt(cwd.to_str().unwrap(), "{}");
            match outcome {
                ReflectOutcome::Prompt(reason) => {
                    assert!(
                        reason.contains("FRUSTRATION BLOCK") || reason.contains("lesson"),
                        "block reason must mention frustration or lesson; got: {reason}"
                    );
                }
                ReflectOutcome::Skip => panic!("expected frustration block, got Skip"),
            }
        }

        // ── Case B: frustration followed by lesson_saved → normal reflect, not frustration-block ─
        {
            let tmp = tempfile::tempdir().expect("tempdir");
            let cwd = tmp.path();
            let events_path = cwd
                .join(".hoangsa")
                .join("state")
                .join("enforcement.events");
            fs::create_dir_all(events_path.parent().unwrap()).unwrap();
            // frustration at line 0, lesson_saved at line 1 → lesson_saved_after=true
            fs::write(
                &events_path,
                "{\"event\":\"frustration\",\"signal\":\"wtf\"}\n{\"event\":\"lesson_saved\"}\n",
            )
            .unwrap();

            let outcome = evaluate_reflect_prompt(cwd.to_str().unwrap(), "{}");
            // Should NOT be a frustration block; the normal memory-reflect prompt fires instead
            match &outcome {
                ReflectOutcome::Prompt(reason) => {
                    assert!(
                        !reason.contains("FRUSTRATION BLOCK"),
                        "must not be frustration-block when lesson_saved follows frustration; got: {reason}"
                    );
                    assert!(
                        reason.contains("memory-reflect"),
                        "expected normal memory-reflect prompt; got: {reason}"
                    );
                }
                ReflectOutcome::Skip => panic!("expected reflect Prompt, got Skip"),
            }
        }

        // ── Case C: stop_hook_active=true → approve (loop guard) ────────────
        {
            let tmp = tempfile::tempdir().expect("tempdir");
            let cwd = tmp.path();
            let events_path = cwd
                .join(".hoangsa")
                .join("state")
                .join("enforcement.events");
            fs::create_dir_all(events_path.parent().unwrap()).unwrap();
            fs::write(
                &events_path,
                "{\"event\":\"frustration\",\"signal\":\"wtf\"}\n",
            )
            .unwrap();

            let outcome =
                evaluate_reflect_prompt(cwd.to_str().unwrap(), r#"{"stop_hook_active":true}"#);
            assert!(
                matches!(outcome, ReflectOutcome::Skip),
                "stop_hook_active=true must always approve (loop guard)"
            );
        }
    }
}
