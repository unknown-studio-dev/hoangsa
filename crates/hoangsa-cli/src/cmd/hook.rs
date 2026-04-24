use crate::helpers::{out, read_json};
use serde_json::json;
use std::fs;
use std::path::Path;

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

enum ReflectOutcome {
    /// No prompt needed — approve the Stop.
    Skip,
    /// Block the Stop and inject `reason` as a system message so the
    /// agent runs memory-reflect before terminating.
    Prompt(String),
}

/// Pure-ish decision for the reflect prompt. Writes the sentinel as a
/// side effect when it returns `Prompt` so the next Stop in this session
/// short-circuits to `Skip`.
fn evaluate_reflect_prompt(cwd: &str, stdin_raw: &str) -> ReflectOutcome {
    let payload: serde_json::Value =
        serde_json::from_str(stdin_raw.trim()).unwrap_or(json!({}));

    // Claude Code sets stop_hook_active=true while it is already continuing
    // from a previous Stop-hook block. Re-blocking here would loop forever.
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

    if let Some(parent) = sentinel.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&sentinel, "");

    ReflectOutcome::Prompt(REFLECT_REASON.to_string())
}

fn reflect_sentinel_path(cwd: &str) -> std::path::PathBuf {
    Path::new(cwd)
        .join(".hoangsa")
        .join("state")
        .join("reflected.sentinel")
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

    // Find hoangsa-memory binary
    let memory_root = Path::new(cwd).join(".hoangsa").join("memory");
    if !memory_root.exists() {
        out(&json!({"decision": "approve"}));
        return;
    }

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
        .filter_map(|c| c.get("body").and_then(|b| b.as_str()))
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
fn build_recall_query(path: &str) -> String {
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

/// Find a binary by searching PATH (cross-platform).
/// `stem` is the binary name without extension (e.g. "hoangsa-memory").
fn find_bin_in_path(stem: &str) -> Option<String> {
    let path_var = std::env::var("PATH").ok()?;
    let sep = if cfg!(windows) { ';' } else { ':' };
    let names: &[&str] = if cfg!(windows) {
        &[".exe", ".cmd", ""]
    } else {
        &[""]
    };
    for dir in path_var.split(sep) {
        for suffix in names {
            let name = format!("{stem}{suffix}");
            let candidate = Path::new(dir).join(&name);
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }
    None
}

fn find_memory_bin() -> Option<String> {
    // PATH first so a user-installed override wins; otherwise fall back
    // to the canonical global install location. `bin/install` places
    // `hoangsa-memory` there unconditionally but does NOT add it to
    // PATH, so a PATH-only lookup silently fails and the archive hook
    // is a no-op — exactly what happened before this fallback landed.
    if let Some(p) = find_bin_in_path("hoangsa-memory") {
        return Some(p);
    }
    let home = std::env::var("HOME").ok()?;
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let candidate = Path::new(&home)
        .join(".hoangsa")
        .join("bin")
        .join(format!("hoangsa-memory{suffix}"));
    if candidate.exists() {
        return Some(candidate.to_string_lossy().to_string());
    }
    None
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

/// Last-two-path-components slug (lowercased, non-alnum → '-'). Kept
/// in sync with `hoangsa-memory-mcp::main::project_slug` so the hook finds the
/// same socket the daemon binds.
fn project_slug(path: &Path) -> String {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let components: Vec<&str> = canonical
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    let n = components.len();
    let parts = if n >= 2 {
        &components[n - 2..]
    } else {
        &components[..]
    };
    let raw = parts.join("-");
    let mut result = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for c in raw.chars().flat_map(|c| c.to_lowercase()) {
        if c.is_ascii_alphanumeric() {
            result.push(c);
            prev_dash = false;
        } else if !prev_dash {
            result.push('-');
            prev_dash = true;
        }
    }
    result.trim_matches('-').to_string()
}

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

/// Resolve the same memory root the MCP server uses. Kept in sync with
/// `hoangsa_memory::resolve::resolve_root` — `hoangsa-cli` can't depend
/// on the full memory crate (would pull in graph, vector store, fastembed)
/// just to read three markdown files, so the chain is duplicated.
///
/// Returns `None` if `HOME` is unset and no local/env root is populated.
fn hoangsa_memory_root(cwd: &str) -> Option<std::path::PathBuf> {
    if let Ok(env) = std::env::var("HOANGSA_MEMORY_ROOT") {
        let p = std::path::PathBuf::from(env);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    let local = Path::new(cwd).join(".hoangsa").join("memory");
    let local_populated = local
        .join("graph.redb")
        .metadata()
        .map(|m| m.is_file() && m.len() > 4096)
        .unwrap_or(false);
    if local_populated {
        return Some(local);
    }
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    let slug = project_slug(Path::new(cwd));
    Some(
        home.join(".hoangsa")
            .join("memory")
            .join("projects")
            .join(slug),
    )
}

/// Read `USER.md` + `MEMORY.md` + `LESSONS.md` from the memory root and
/// compose them into a single `additionalContext` blob for the
/// SessionStart hook. Returns `None` when all three files are missing or
/// empty — we don't want to inject a header-only section.
fn compose_session_start_context(root: &Path) -> Option<String> {
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
fn count_incomplete_tasks(plan: &serde_json::Value) -> usize {
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

// ── Unified Enforcement Hook ─────────────────────────────────────────────────

/// `hook enforce`
///
/// Single PreToolUse entry point for ALL enforcement:
/// 1. Pattern-based rules from rules.json (same as rule-gate)
/// 2. Stateful rule: require memory_impact before Edit (first-touch files only)
/// 3. Stateful rule: require detect_changes before git commit
///
/// Critical (block) rules fail-CLOSED. Quality (warn) rules fail-OPEN.
pub fn cmd_enforce(cwd: &str) {
    use crate::cmd::rule::{
        evaluate_rule_conditions, read_rules_config_pub, Enforcement, RuleAction,
    };
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let parsed: serde_json::Value = serde_json::from_str(&input).unwrap_or(json!({}));
    let tool_name = parsed
        .get("tool_name")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let tool_input = parsed.get("tool_input").cloned().unwrap_or(json!({}));

    // ── Layer 1: Pattern-based rules from rules.json ──
    let config = match read_rules_config_pub(cwd) {
        Ok(Some(c)) => c,
        Ok(None) => {
            // No rules.json — still run stateful checks
            if let Some(result) = stateful_check(cwd, tool_name, &tool_input) {
                print_decision(&result);
                return;
            }
            out(&json!({"decision": "approve"}));
            return;
        }
        Err(_) => {
            // Parse error — fail-OPEN for quality, but stateful checks still run
            if let Some(result) = stateful_check(cwd, tool_name, &tool_input) {
                print_decision(&result);
                return;
            }
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    let mut warnings: Vec<String> = Vec::new();

    for rule in &config.rules {
        if !rule.enabled {
            continue;
        }
        // Skip rules that aren't hook-enforced or prompt-enforced
        // (preflight rules are checked elsewhere by CLI)
        if rule.enforcement == Enforcement::Preflight {
            continue;
        }
        // Stateful rules are dispatched by stateful_check below, not pattern-matched.
        if rule.stateful.is_some() {
            continue;
        }

        let matcher_matches = rule.matcher.split('|').any(|m| m.trim() == tool_name);
        if !matcher_matches {
            continue;
        }

        if !evaluate_rule_conditions(rule, &tool_input) {
            continue;
        }

        match rule.action {
            RuleAction::Block => {
                let reason = format!(
                    "⛔ RULE VIOLATION: {}\n\nRule: {}\nAction: BLOCK\n\n{}",
                    rule.id, rule.name, rule.message
                );
                out(&json!({"decision": "block", "reason": reason}));
                return;
            }
            RuleAction::Warn => {
                warnings.push(format!("⚠️ {}: {}", rule.id, rule.message));
            }
        }
    }

    // ── Layer 2: Stateful checks (require impact/detect_changes) ──
    if let Some(result) = stateful_check(cwd, tool_name, &tool_input) {
        match result.decision.as_str() {
            "block" => {
                // Append any pattern warnings to the reason
                let mut reason = result.reason;
                if !warnings.is_empty() {
                    reason = format!("{}\n\n---\nAdditional warnings:\n{}", reason, warnings.join("\n"));
                }
                out(&json!({"decision": "block", "reason": reason}));
                return;
            }
            _ => {
                // Stateful check passed but may have added warnings
                if let Some(w) = result.warning {
                    warnings.push(w);
                }
            }
        }
    }

    // ── Output final decision ──
    if warnings.is_empty() {
        out(&json!({"decision": "approve"}));
    } else {
        let reason = warnings.join("\n\n");
        out(&json!({"decision": "approve", "reason": reason}));
    }
}

struct EnforceResult {
    decision: String,
    reason: String,
    warning: Option<String>,
}

fn print_decision(result: &EnforceResult) {
    if result.decision == "block" {
        out(&json!({"decision": "block", "reason": result.reason}));
    } else if let Some(w) = &result.warning {
        out(&json!({"decision": "approve", "reason": w}));
    } else {
        out(&json!({"decision": "approve"}));
    }
}

/// Stateful enforcement checks based on event log.
/// Returns None if no stateful rule applies to this tool call.
fn stateful_check(cwd: &str, tool_name: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    match tool_name {
        "Edit" | "Write" => {
            if stateful_rule_enabled(cwd, "require-memory-impact") {
                stateful_check_edit(cwd, tool_input)
            } else {
                None
            }
        }
        "Bash" => {
            if stateful_rule_enabled(cwd, "require-detect-changes")
                && let Some(r) = stateful_check_bash(cwd, tool_input)
            {
                return Some(r);
            }
            if stateful_rule_enabled(cwd, "no-git-add-ignored")
                && let Some(r) = check_gitignore_add(cwd, tool_input)
            {
                return Some(r);
            }
            None
        }
        _ => None,
    }
}

/// Look up a stateful rule by its `stateful` field value; default-enabled if no
/// entry is present (backwards compatibility with installs predating the field).
fn stateful_rule_enabled(cwd: &str, stateful_id: &str) -> bool {
    use crate::cmd::rule::read_rules_config_pub;
    let config = match read_rules_config_pub(cwd) {
        Ok(Some(c)) => c,
        _ => return true,
    };
    for rule in &config.rules {
        if rule.stateful.as_deref() == Some(stateful_id) {
            return rule.enabled;
        }
    }
    true
}

/// Rule #9: Require memory_impact for first-touch files before Edit.
/// Softened: if the file already has events (prior impact or edit), skip.
/// Thin wrapper — does I/O, delegates correlation to `intent_guard_edit`.
fn stateful_check_edit(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let file_path = tool_input.get("file_path").and_then(|v| v.as_str())?;
    if !is_source_file(file_path) {
        return None;
    }
    let events = fs::read_to_string(enforcement_events_path(cwd)).unwrap_or_default();
    match intent_guard_edit(&events, file_path) {
        IntentOutcome::Approve => None,
        IntentOutcome::Block(reason) => Some(EnforceResult { decision: "block".to_string(), reason, warning: None }),
        IntentOutcome::Warn(w) => Some(EnforceResult { decision: "approve".to_string(), reason: String::new(), warning: Some(w) }),
    }
}

/// Rule #10: Require detect_changes before git commit.
/// Thin wrapper — does I/O, delegates correlation to `intent_guard_bash_commit`.
fn stateful_check_bash(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let command = tool_input.get("command").and_then(|v| v.as_str())?;
    if !is_git_commit(command) {
        return None;
    }
    let events = fs::read_to_string(enforcement_events_path(cwd)).unwrap_or_default();
    let diff_files = get_staged_files(cwd);
    match intent_guard_bash_commit(&events, &diff_files) {
        IntentOutcome::Approve => None,
        IntentOutcome::Block(reason) => Some(EnforceResult { decision: "block".to_string(), reason, warning: None }),
        IntentOutcome::Warn(w) => Some(EnforceResult { decision: "approve".to_string(), reason: String::new(), warning: Some(w) }),
    }
}

/// Rule `no-git-add-ignored`: Block `git add` of gitignored files early so
/// the agent gets a specific error instead of a cryptic "git commit failed"
/// after a silent `git add` exit 1.
///
/// Thin wrapper — does I/O (`git check-ignore`), delegates parsing to
/// `parse_git_add_files` and the block message to `gitignore_block_reason`.
fn check_gitignore_add(cwd: &str, tool_input: &serde_json::Value) -> Option<EnforceResult> {
    let command = tool_input.get("command").and_then(|v| v.as_str())?;
    let files = parse_git_add_files(command)?;
    let reason = gitignore_block_reason(&files, |f| is_path_gitignored(cwd, f))?;
    Some(EnforceResult { decision: "block".to_string(), reason, warning: None })
}

/// Pure: extract non-flag file args from a `git add ...` command.
///
/// Returns `None` when the command should not be checked by this rule:
/// - Not a `git add` command (prefix mismatch).
/// - Has `-f` / `--force` (covered by `no-git-add-force`).
/// - Has `-A` / `--all` / `.` (covered by `warn-git-add-all`; enumerating
///   ignored files in the working tree is out of scope for v1).
/// - No file args.
///
/// Uses whitespace tokenisation — file paths with spaces (rare for source
/// files) fall through this rule and hit the original git-add failure.
fn parse_git_add_files(command: &str) -> Option<Vec<String>> {
    let trimmed = command.trim_start();
    let rest = trimmed.strip_prefix("git")?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let rest = rest.trim_start().strip_prefix("add")?;
    if !rest.starts_with(|c: char| c.is_whitespace()) {
        return None;
    }
    let tokens: Vec<&str> = rest.split_whitespace().collect();

    let mut files: Vec<String> = Vec::new();
    for tok in tokens {
        if matches!(tok, "-f" | "--force" | "-A" | "--all" | ".") {
            return None;
        }
        if tok.starts_with('-') {
            continue;
        }
        files.push(tok.to_string());
    }
    if files.is_empty() {
        None
    } else {
        Some(files)
    }
}

/// Pure: given a list of file args and an `is_ignored` predicate, return the
/// block reason if any file is ignored, else `None`.
fn gitignore_block_reason<F: Fn(&str) -> bool>(files: &[String], is_ignored: F) -> Option<String> {
    let ignored: Vec<&String> = files.iter().filter(|f| is_ignored(f)).collect();
    if ignored.is_empty() {
        return None;
    }
    let list = ignored.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ");
    Some(format!(
        "⛔ RULE VIOLATION: no-git-add-ignored\n\nRule: Block git add of gitignored files\nAction: BLOCK\n\ngit add contains gitignored files: {list}. Remove them from the command or update .gitignore."
    ))
}

/// I/O: run `git check-ignore -q <path>` in `cwd`. Exit 0 = ignored.
/// Any error (git missing, outside repo, etc.) → `false` (graceful degrade).
fn is_path_gitignored(cwd: &str, path: &str) -> bool {
    use std::process::{Command, Stdio};
    Command::new("git")
        .args(["check-ignore", "-q", path])
        .current_dir(cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Outcome of a pure intent-guard check. Kept separate from `EnforceResult`
/// so the guard logic is trivially unit-testable without mocking I/O.
#[derive(Debug, Clone, PartialEq)]
pub enum IntentOutcome {
    Approve,
    Block(String),
    Warn(String),
}

/// Pure: is there a prior `impact` or `override` event covering `file_path`?
/// Tolerant of rel↔abs path differences (via `paths_refer_to_same_file`).
/// Takes the full events-log text as input so it's trivially unit-testable
/// — no filesystem reads, no `cwd` threading, no env dependency.
pub fn intent_guard_edit(events: &str, file_path: &str) -> IntentOutcome {
    let covered = events.lines().any(|line| {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return false,
        };
        match entry.get("event").and_then(|e| e.as_str()).unwrap_or("") {
            "impact" => entry
                .get("file")
                .and_then(|f| f.as_str())
                .map(|f| !f.is_empty() && paths_refer_to_same_file(f, file_path))
                .unwrap_or(false),
            "override" => {
                entry.get("rule").and_then(|r| r.as_str()) == Some("require-memory-impact")
                    && entry
                        .get("target")
                        .and_then(|t| t.as_str())
                        .map(|t| paths_refer_to_same_file(t, file_path))
                        .unwrap_or(false)
            }
            _ => false,
        }
    });

    if covered {
        IntentOutcome::Approve
    } else {
        IntentOutcome::Block(format!(
            "⛔ STATEFUL: require-memory-impact\n\n\
             No memory_impact found for '{path}'\n\
             Run memory_impact on this file before editing.\n\n\
             If this is a false positive, use:\n\
             hoangsa-cli enforce override --rule require-memory-impact --target {path} --reason \"...\"",
            path = file_path
        ))
    }
}

/// Pure: compare the most-recent `detect_changes` event's file list against
/// the actual staged-diff file list. Returns Block if no detect_changes at
/// all, Warn if scope drift, Approve if covered. An override event for
/// `require-detect-changes` short-circuits to Approve.
pub fn intent_guard_bash_commit(events: &str, staged_files: &[String]) -> IntentOutcome {
    let mut detected: Vec<String> = Vec::new();
    let mut has_override = false;
    for line in events.lines() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        match entry.get("event").and_then(|e| e.as_str()).unwrap_or("") {
            "detect_changes" => {
                if let Some(files) = entry.get("files").and_then(|f| f.as_array()) {
                    for f in files {
                        if let Some(s) = f.as_str() {
                            detected.push(s.to_string());
                        }
                    }
                }
            }
            "override" if entry.get("rule").and_then(|r| r.as_str()) == Some("require-detect-changes") => {
                has_override = true;
            }
            _ => {}
        }
    }

    if has_override {
        return IntentOutcome::Approve;
    }

    if detected.is_empty() {
        return IntentOutcome::Block(
            "⛔ STATEFUL: require-detect-changes\n\n\
             No memory_detect_changes found before commit.\n\
             Run memory_detect_changes to verify scope before committing.\n\n\
             If this is a false positive, use:\n\
             hoangsa-cli enforce override --rule require-detect-changes --target commit --reason \"...\""
                .to_string(),
        );
    }

    // No staged files → nothing to correlate against (e.g. amend, empty commit).
    if staged_files.is_empty() {
        return IntentOutcome::Approve;
    }

    let uncovered: Vec<&String> = staged_files
        .iter()
        .filter(|f| {
            !detected
                .iter()
                .any(|d| f.ends_with(d.as_str()) || d.ends_with(f.as_str()))
        })
        .collect();

    if uncovered.is_empty() {
        IntentOutcome::Approve
    } else {
        let list = uncovered
            .iter()
            .map(|f| f.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        IntentOutcome::Warn(format!(
            "⚠️ INTENT GUARD: Files in staged diff not covered by detect_changes: [{list}]\n\
             Consider re-running memory_detect_changes before commit."
        ))
    }
}

fn get_staged_files(cwd: &str) -> Vec<String> {
    use std::process::Command;
    let output = Command::new("git")
        .args(["diff", "--cached", "--name-only"])
        .current_dir(cwd)
        .output();
    match output {
        Ok(o) => String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect(),
        Err(_) => Vec::new(),
    }
}

fn is_git_commit(command: &str) -> bool {
    let re = regex::Regex::new(r"git\s+commit").unwrap_or_else(|_| regex::Regex::new("$^").expect("infallible"));
    re.is_match(command)
}

/// Relaxed path equality: treat an absolute path and a repo-relative path as
/// referring to the same file when one ends with the other. Used to bridge
/// impact events (often relative) against Edit file_path (usually absolute).
fn paths_refer_to_same_file(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    // Normalize leading "./"
    let an = a.trim_start_matches("./");
    let bn = b.trim_start_matches("./");
    if an == bn {
        return true;
    }
    // Tolerate abs↔rel: one must end with the other preceded by a path separator
    // to avoid "foo/bar.rs" matching "other/foo/bar.rs" incorrectly — wait, that's
    // actually the intended match here (same basename+subpath), so a bare ends_with
    // is correct. Require at least one path separator to avoid "a.rs" matching
    // "banana.rs".
    let ends_match = |long: &str, short: &str| -> bool {
        short.contains('/') && long.ends_with(short) && {
            let boundary = long.len() - short.len();
            boundary == 0 || long.as_bytes().get(boundary - 1) == Some(&b'/')
        }
    };
    ends_match(an, bn) || ends_match(bn, an)
}

fn is_source_file(path: &str) -> bool {
    let source_extensions = [
        ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java",
        ".c", ".cpp", ".h", ".hpp", ".rb", ".swift", ".kt",
    ];
    source_extensions.iter().any(|ext| path.ends_with(ext))
}

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

    let tool_name = parsed.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    let tool_input = parsed.get("tool_input").cloned().unwrap_or(json!({}));

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
/// `DEFAULT_SYMBOL_PATTERNS` when absent or malformed.
fn read_symbol_patterns(cwd: &str) -> Vec<String> {
    let config_path = Path::new(cwd).join(".hoangsa").join("config.json");
    if !config_path.exists() {
        return DEFAULT_SYMBOL_PATTERNS.iter().map(|s| s.to_string()).collect();
    }
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
    find_symbol_in_tree(cwd, Path::new(cwd), &compiled, 0)
}

/// Recursive DFS over source files looking for any pattern match.
/// Skips vendor/build dirs and binary extensions. Returns the first match.
fn find_symbol_in_tree(
    cwd: &str,
    dir: &Path,
    patterns: &[regex::Regex],
    depth: u32,
) -> Option<String> {
    if depth > 8 {
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
        if name.starts_with('.') && depth == 0 {
            continue;
        }
        let ft = match entry.file_type() { Ok(t) => t, Err(_) => continue };
        if ft.is_dir() {
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            if let Some(found) = find_symbol_in_tree(cwd, &path, patterns, depth + 1) {
                return Some(found);
            }
        } else if ft.is_file() {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if !SOURCE_EXTS.contains(&ext) {
                continue;
            }
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

fn append_event(cwd: &str, event: &serde_json::Value) {
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

    for line in content.lines() {
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

    let total_events = content.lines().count();
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

// ── Enforcement State: append-only JSONL event log ──────────────────────────

fn enforcement_events_path(cwd: &str) -> std::path::PathBuf {
    Path::new(cwd)
        .join(".hoangsa")
        .join("state")
        .join("enforcement.events")
}

/// `hook state-record`
///
/// Appends a single enforcement event (JSONL line) to `.hoangsa/state/enforcement.events`.
/// Reads event JSON from stdin. Adds `ts` field if missing.
/// Always outputs `{"decision":"approve"}` — never blocks.
pub fn cmd_state_record(cwd: &str) {
    use std::io::Read as _;

    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();

    let mut event: serde_json::Value = match serde_json::from_str(input.trim()) {
        Ok(v) => v,
        Err(_) => {
            out(&json!({"decision": "approve"}));
            return;
        }
    };

    if event.get("ts").is_none() {
        let ts = chrono_now();
        event.as_object_mut().map(|o| o.insert("ts".to_string(), json!(ts)));
    }

    let events_path = enforcement_events_path(cwd);
    if let Some(parent) = events_path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    let mut line = serde_json::to_string(&event).unwrap_or_default();
    line.push('\n');

    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .and_then(|mut f| {
            use std::io::Write as _;
            f.write_all(line.as_bytes())
        });

    out(&json!({"decision": "approve"}));
}

/// `hook state-check --event <type> [--file <path>] [--symbol <name>]`
///
/// Checks if a matching event exists in the enforcement log.
/// Outputs JSON: `{"found": true/false, "event": ...}` or `{"found": false}`.
pub fn cmd_state_check(cwd: &str, args: &[&str]) {
    let event_type = flag_value(args, "--event").unwrap_or("");
    let file_filter = flag_value(args, "--file");
    let symbol_filter = flag_value(args, "--symbol");

    if event_type.is_empty() {
        out(&json!({"found": false, "error": "missing --event flag"}));
        return;
    }

    let events_path = enforcement_events_path(cwd);
    let content = match fs::read_to_string(&events_path) {
        Ok(c) => c,
        Err(_) => {
            out(&json!({"found": false}));
            return;
        }
    };

    for line in content.lines().rev() {
        let entry: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if entry.get("event").and_then(|e| e.as_str()) != Some(event_type) {
            continue;
        }

        if let Some(file) = file_filter {
            let entry_file = entry.get("file").and_then(|f| f.as_str()).unwrap_or("");
            if entry_file != file {
                continue;
            }
        }

        if let Some(symbol) = symbol_filter {
            let entry_sym = entry.get("symbol").and_then(|s| s.as_str()).unwrap_or("");
            if entry_sym != symbol {
                continue;
            }
        }

        out(&json!({"found": true, "event": entry}));
        return;
    }

    out(&json!({"found": false}));
}

/// `hook state-clear`
///
/// Fires on every SessionStart (startup, resume, clear). Clears the
/// enforcement events file, and on `source == "clear"` snapshots the
/// statusline cost baseline so the displayed cost resets to $0.00.
pub fn cmd_state_clear(cwd: &str) {
    let events_path = enforcement_events_path(cwd);
    let _ = fs::remove_file(&events_path);
    let _ = fs::remove_file(reflect_sentinel_path(cwd));

    // Best-effort: read SessionStart payload (if any) and handle /clear.
    let mut raw = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut raw);
    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&raw) {
        let source = payload.get("source").and_then(|v| v.as_str()).unwrap_or("");
        let sid = payload.get("session_id").and_then(|v| v.as_str()).unwrap_or("");
        if source == "clear" && !sid.is_empty() {
            snapshot_statusline_baseline(sid);
        }
    }

    out(&json!({"success": true}));
}

/// On `/clear`, promote the last-seen cost into the baseline so the
/// statusline displays `max(0, total - baseline) = 0` until the new
/// conversation accrues cost. Rewrites the stored session_id from the
/// payload so the next statusline tick (which may carry a fresh sid
/// from CC) still treats the baseline as current.
fn snapshot_statusline_baseline(session_id: &str) {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
    let run_dir = home.join(".hoangsa").join("run");
    let path = crate::cmd::statusline::cost_state_path(&run_dir);
    let Some(mut state) = crate::cmd::statusline::read_cost_state(&path) else { return };
    state.baseline = state.last_seen;
    state.session_id = session_id.to_string();
    crate::cmd::statusline::write_cost_state(&path, &state);
}

fn flag_value<'a>(args: &'a [&'a str], flag: &str) -> Option<&'a str> {
    args.iter()
        .position(|&a| a == flag)
        .and_then(|i| args.get(i + 1))
        .copied()
}

fn chrono_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}Z", secs)
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
struct UsageTotals {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_creation: u64,
    turns: u64,
}

impl UsageTotals {
    fn total(&self) -> u64 {
        self.input + self.output + self.cache_read + self.cache_creation
    }
}

/// Sum `message.usage` fields across all assistant lines in a transcript JSONL.
fn tally_transcript(transcript_path: &Path) -> Option<UsageTotals> {
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
mod tests {
    use super::*;
    use serde_json::json;

    // ── intent_guard_edit ────────────────────────────────────────────────────

    #[test]
    fn test_intent_guard_edit_empty_log_blocks() {
        let result = intent_guard_edit("", "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)), "empty events must block");
    }

    #[test]
    fn test_intent_guard_edit_matching_impact_approves() {
        let events = r#"{"event":"impact","file":"cli/src/cmd/foo.rs","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/Users/me/proj/cli/src/cmd/foo.rs");
        assert_eq!(result, IntentOutcome::Approve, "abs↔rel path match should approve");
    }

    #[test]
    fn test_intent_guard_edit_rejects_empty_file_field() {
        // An impact event where file resolution failed (empty string) must NOT
        // satisfy the guard — otherwise every unresolved event would unlock every file.
        let events = r#"{"event":"impact","file":"","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_override_approves() {
        let events = r#"{"event":"override","rule":"require-memory-impact","target":"/abs/path/foo.rs","reason":"test"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_edit_override_for_different_rule_blocks() {
        let events = r#"{"event":"override","rule":"some-other-rule","target":"/abs/path/foo.rs","reason":"test"}
"#;
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_different_file_blocks() {
        let events = r#"{"event":"impact","file":"cli/src/cmd/foo.rs","symbol":"foo::bar"}
"#;
        let result = intent_guard_edit(events, "/Users/me/proj/cli/src/cmd/bar.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_edit_malformed_lines_skipped() {
        // Malformed JSON lines must not crash or satisfy the guard.
        let events = "garbage line\n{invalid json\n\n";
        let result = intent_guard_edit(events, "/abs/path/foo.rs");
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    // ── intent_guard_bash_commit ─────────────────────────────────────────────

    #[test]
    fn test_intent_guard_bash_no_detect_changes_blocks() {
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit("", &files);
        assert!(matches!(result, IntentOutcome::Block(_)));
    }

    #[test]
    fn test_intent_guard_bash_override_approves() {
        let events = r#"{"event":"override","rule":"require-detect-changes","target":"commit","reason":"..."}
"#;
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit(events, &files);
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_bash_detect_changes_covers_diff() {
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let files = vec!["cli/src/cmd/foo.rs".to_string()];
        let result = intent_guard_bash_commit(events, &files);
        assert_eq!(result, IntentOutcome::Approve);
    }

    #[test]
    fn test_intent_guard_bash_diff_grew_warns() {
        // detect_changes covered foo.rs but bar.rs snuck into the staged diff.
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let files = vec![
            "cli/src/cmd/foo.rs".to_string(),
            "cli/src/cmd/bar.rs".to_string(),
        ];
        let result = intent_guard_bash_commit(events, &files);
        match result {
            IntentOutcome::Warn(msg) => assert!(msg.contains("bar.rs")),
            other => panic!("expected Warn, got {other:?}"),
        }
    }

    #[test]
    fn test_intent_guard_bash_empty_staged_files_approves() {
        // No staged files (e.g. `git commit --amend` no-op) → nothing to correlate.
        let events = r#"{"event":"detect_changes","files":["cli/src/cmd/foo.rs"]}
"#;
        let result = intent_guard_bash_commit(events, &[]);
        assert_eq!(result, IntentOutcome::Approve);
    }

    // ── build_recall_query ───────────────────────────────────────────────────

    #[test]
    fn test_build_recall_query_relative_path() {
        let q = build_recall_query("src/cmd/pref.rs");
        assert_eq!(q, "NEVER edit src/cmd/pref.rs");
    }

    #[test]
    fn test_build_recall_query_empty_path() {
        let q = build_recall_query("");
        // empty path → empty after strip → "NEVER edit "
        assert!(q.starts_with("NEVER edit"));
    }

    #[test]
    fn test_build_recall_query_absolute_non_home_path() {
        // path that is definitely not under HOME: /tmp/file.rs
        let q = build_recall_query("/tmp/file.rs");
        assert!(q.contains("tmp/file.rs"), "expected path segment in query, got: {q}");
        assert!(q.starts_with("NEVER edit"));
    }

    // ── count_incomplete_tasks ───────────────────────────────────────────────

    #[test]
    fn test_count_incomplete_tasks_all_pending() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "pending" },
                { "id": "T-02", "status": "running" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 2);
    }

    #[test]
    fn test_count_incomplete_tasks_all_done() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "completed" },
                { "id": "T-02", "status": "done" },
                { "id": "T-03", "status": "skipped" },
                { "id": "T-04", "status": "failed" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    #[test]
    fn test_count_incomplete_tasks_mixed() {
        let plan = json!({
            "tasks": [
                { "id": "T-01", "status": "completed" },
                { "id": "T-02", "status": "pending" },
                { "id": "T-03", "status": "running" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 2);
    }

    #[test]
    fn test_count_incomplete_tasks_missing_status() {
        // Missing status field defaults to "pending" (incomplete)
        let plan = json!({
            "tasks": [
                { "id": "T-01" },
            ]
        });
        assert_eq!(count_incomplete_tasks(&plan), 1);
    }

    #[test]
    fn test_count_incomplete_tasks_no_tasks_key() {
        let plan = json!({});
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    #[test]
    fn test_count_incomplete_tasks_empty_tasks() {
        let plan = json!({ "tasks": [] });
        assert_eq!(count_incomplete_tasks(&plan), 0);
    }

    // ── enforcement state ───────────────────────────────────────────────────

    #[test]
    fn test_enforcement_events_path() {
        let p = enforcement_events_path("/tmp/project");
        assert_eq!(
            p.to_string_lossy(),
            "/tmp/project/.hoangsa/state/enforcement.events"
        );
    }

    // ── reflect prompt ──────────────────────────────────────────────────────

    #[test]
    fn test_reflect_sentinel_path() {
        let p = reflect_sentinel_path("/tmp/project");
        assert_eq!(
            p.to_string_lossy(),
            "/tmp/project/.hoangsa/state/reflected.sentinel"
        );
    }

    fn seed_events_file(cwd: &std::path::Path) {
        let events = enforcement_events_path(cwd.to_str().unwrap());
        fs::create_dir_all(events.parent().unwrap()).unwrap();
        fs::write(&events, "{\"event\":\"impact\"}\n").unwrap();
    }

    #[test]
    fn reflect_prompts_when_work_done_and_no_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());

        let outcome = evaluate_reflect_prompt(cwd, "{}");
        match outcome {
            ReflectOutcome::Prompt(reason) => {
                assert!(reason.contains("memory-reflect"), "reason: {reason}");
            }
            ReflectOutcome::Skip => panic!("expected Prompt, got Skip"),
        }
        // Sentinel must be written so the next Stop short-circuits.
        assert!(reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_skips_when_stop_hook_active() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());

        let outcome =
            evaluate_reflect_prompt(cwd, r#"{"stop_hook_active":true}"#);
        assert!(matches!(outcome, ReflectOutcome::Skip));
        // Must NOT write the sentinel — avoids suppressing the next session.
        assert!(!reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_skips_when_sentinel_already_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());
        let sentinel = reflect_sentinel_path(cwd);
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(&sentinel, "").unwrap();

        let outcome = evaluate_reflect_prompt(cwd, "{}");
        assert!(matches!(outcome, ReflectOutcome::Skip));
    }

    #[test]
    fn reflect_skips_when_no_work_recorded() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        // No events file at all.
        let outcome = evaluate_reflect_prompt(cwd, "{}");
        assert!(matches!(outcome, ReflectOutcome::Skip));
        assert!(!reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_skips_when_events_file_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        let events = enforcement_events_path(cwd);
        fs::create_dir_all(events.parent().unwrap()).unwrap();
        fs::write(&events, "").unwrap();

        let outcome = evaluate_reflect_prompt(cwd, "{}");
        assert!(matches!(outcome, ReflectOutcome::Skip));
        assert!(!reflect_sentinel_path(cwd).exists());
    }

    #[test]
    fn reflect_tolerates_malformed_stdin() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        seed_events_file(tmp.path());
        // Garbage stdin falls back to default payload → stop_hook_active=false.
        let outcome = evaluate_reflect_prompt(cwd, "not-json-at-all");
        assert!(matches!(outcome, ReflectOutcome::Prompt(_)));
    }

    // ── SessionStart inject ──────────────────────────────────────────────────

    #[test]
    fn compose_session_start_context_none_when_all_empty() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(compose_session_start_context(tmp.path()).is_none());
    }

    #[test]
    fn compose_session_start_context_skips_empty_files() {
        let tmp = tempfile::tempdir().unwrap();
        // Whitespace-only file must be treated as empty.
        fs::write(tmp.path().join("MEMORY.md"), "   \n\n").unwrap();
        assert!(compose_session_start_context(tmp.path()).is_none());
    }

    #[test]
    fn compose_session_start_context_includes_present_files() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            tmp.path().join("USER.md"),
            "# USER.md\n### prefer Vietnamese responses\ntags: language\n",
        )
        .unwrap();
        fs::write(
            tmp.path().join("LESSONS.md"),
            "# LESSONS.md\n### editing migrations\nrun sqlx prepare after\n",
        )
        .unwrap();
        // MEMORY.md intentionally missing.

        let ctx = compose_session_start_context(tmp.path()).expect("ctx");
        assert!(ctx.contains("hoangsa-memory"));
        assert!(ctx.contains("USER.md"));
        assert!(ctx.contains("prefer Vietnamese responses"));
        assert!(ctx.contains("LESSONS.md"));
        assert!(ctx.contains("editing migrations"));
        assert!(
            !ctx.contains("─── MEMORY.md"),
            "missing file must not produce a header"
        );
    }

    #[test]
    fn state_clear_removes_reflect_sentinel() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_str().unwrap();
        let sentinel = reflect_sentinel_path(cwd);
        fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
        fs::write(&sentinel, "").unwrap();
        seed_events_file(tmp.path());

        cmd_state_clear(cwd);

        assert!(!sentinel.exists(), "sentinel must be wiped on SessionStart");
        assert!(
            !enforcement_events_path(cwd).exists(),
            "events file must be wiped on SessionStart"
        );
    }

    #[test]
    fn test_flag_value_found() {
        let args = vec!["--event", "impact", "--file", "foo.rs"];
        assert_eq!(flag_value(&args, "--event"), Some("impact"));
        assert_eq!(flag_value(&args, "--file"), Some("foo.rs"));
    }

    #[test]
    fn test_flag_value_not_found() {
        let args = vec!["--event", "impact"];
        assert_eq!(flag_value(&args, "--file"), None);
    }

    #[test]
    fn test_flag_value_at_end() {
        let args = vec!["--event"];
        assert_eq!(flag_value(&args, "--event"), None);
    }

    #[test]
    fn test_chrono_now_format() {
        let ts = chrono_now();
        assert!(ts.ends_with('Z'));
        let num_part = &ts[..ts.len() - 1];
        assert!(num_part.parse::<u64>().is_ok());
    }

    // ── tally_transcript ─────────────────────────────────────────────────────

    #[test]
    fn tally_transcript_sums_assistant_usage_only() {
        use std::io::Write as _;
        let tmp = tempfile::NamedTempFile::new().expect("tmp");
        let mut f = tmp.reopen().expect("reopen");
        // Two assistant lines with usage, one user line without.
        writeln!(
            f,
            r#"{{"type":"user","message":{{"role":"user","content":"hi"}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":100,"output_tokens":50,"cache_read_input_tokens":10,"cache_creation_input_tokens":5}}}}}}"#
        )
        .unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":200,"output_tokens":75,"cache_read_input_tokens":20,"cache_creation_input_tokens":0}}}}}}"#
        )
        .unwrap();

        let t = tally_transcript(tmp.path()).expect("tally");
        assert_eq!(t.input, 300);
        assert_eq!(t.output, 125);
        assert_eq!(t.cache_read, 30);
        assert_eq!(t.cache_creation, 5);
        assert_eq!(t.turns, 2);
        assert_eq!(t.total(), 460);
    }

    #[test]
    fn tally_transcript_missing_file_returns_none() {
        assert!(tally_transcript(Path::new("/nonexistent/path/transcript.jsonl")).is_none());
    }

    #[test]
    fn tally_transcript_tolerates_malformed_lines() {
        use std::io::Write as _;
        let tmp = tempfile::NamedTempFile::new().expect("tmp");
        let mut f = tmp.reopen().expect("reopen");
        writeln!(f, "not json").unwrap();
        writeln!(
            f,
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":10,"output_tokens":20}}}}}}"#
        )
        .unwrap();
        let t = tally_transcript(tmp.path()).expect("tally");
        assert_eq!(t.input, 10);
        assert_eq!(t.output, 20);
        assert_eq!(t.turns, 1);
    }

    // ── parse_git_add_files ──────────────────────────────────────────────────

    #[test]
    fn parse_git_add_files_simple() {
        assert_eq!(
            parse_git_add_files("git add foo.log").unwrap(),
            vec!["foo.log".to_string()]
        );
    }

    #[test]
    fn parse_git_add_files_multiple() {
        assert_eq!(
            parse_git_add_files("git add foo.log bar.txt baz/qux.rs").unwrap(),
            vec!["foo.log".to_string(), "bar.txt".to_string(), "baz/qux.rs".to_string()]
        );
    }

    #[test]
    fn parse_git_add_files_leading_whitespace() {
        assert_eq!(
            parse_git_add_files("   git add foo.log").unwrap(),
            vec!["foo.log".to_string()]
        );
    }

    #[test]
    fn parse_git_add_files_not_git_add() {
        assert!(parse_git_add_files("git commit -m 'hi'").is_none());
        assert!(parse_git_add_files("git status").is_none());
        assert!(parse_git_add_files("echo git add foo").is_none());
        assert!(parse_git_add_files("gitadd foo").is_none()); // no space
    }

    #[test]
    fn parse_git_add_files_skips_force() {
        assert!(parse_git_add_files("git add -f foo.log").is_none());
        assert!(parse_git_add_files("git add --force foo.log").is_none());
    }

    #[test]
    fn parse_git_add_files_skips_all() {
        assert!(parse_git_add_files("git add -A").is_none());
        assert!(parse_git_add_files("git add --all").is_none());
        assert!(parse_git_add_files("git add .").is_none());
    }

    #[test]
    fn parse_git_add_files_empty_args() {
        assert!(parse_git_add_files("git add").is_none());
        assert!(parse_git_add_files("git add   ").is_none());
    }

    #[test]
    fn parse_git_add_files_skips_other_flags() {
        // -v is a real git-add flag; not covered by another rule — just pass through the files.
        assert_eq!(
            parse_git_add_files("git add -v foo.log").unwrap(),
            vec!["foo.log".to_string()]
        );
    }

    // ── gitignore_block_reason ───────────────────────────────────────────────

    #[test]
    fn gitignore_block_reason_none_when_clean() {
        let files = vec!["a.rs".to_string(), "b.rs".to_string()];
        assert!(gitignore_block_reason(&files, |_| false).is_none());
    }

    #[test]
    fn gitignore_block_reason_blocks_on_any_ignored() {
        let files = vec!["a.rs".to_string(), "foo.log".to_string()];
        let reason = gitignore_block_reason(&files, |f| f.ends_with(".log")).expect("should block");
        assert!(reason.contains("foo.log"), "reason should name the ignored file");
        assert!(!reason.contains("a.rs"), "reason should not name clean files");
        assert!(reason.contains("no-git-add-ignored"), "reason should cite the rule id");
    }

    #[test]
    fn gitignore_block_reason_lists_all_ignored() {
        let files = vec!["a.log".to_string(), "b.rs".to_string(), "c.log".to_string()];
        let reason = gitignore_block_reason(&files, |f| f.ends_with(".log")).expect("should block");
        assert!(reason.contains("a.log"));
        assert!(reason.contains("c.log"));
        assert!(!reason.contains("b.rs"));
    }

    #[test]
    fn gitignore_block_reason_none_for_empty_files() {
        let files: Vec<String> = vec![];
        assert!(gitignore_block_reason(&files, |_| true).is_none());
    }
}
