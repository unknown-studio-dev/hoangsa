//! Archive ingest core — extracted from `hoangsa-memory::archive_cmd` so the
//! MCP daemon and the CLI can share it.
//!
//! The memory story is the whole point of living here: each CLI
//! `archive ingest` subprocess previously spun up its own Python
//! ChromaDB sidecar (~500 MB RSS). When the MCP daemon routes ingests
//! through [`run_ingest`] it reuses its lazy-initialised
//! [`hoangsa_memory_store::EmbeddedVectorStore`], so concurrent hook
//! fires no longer pile up sidecars — and, as of Phase 2, there is no
//! Python sidecar to pile up in the first place.
//!
//! The parsing / chunking / topic-detection pipeline is verbatim from
//! `archive_cmd.rs` — conversation mining still follows the MemPalace
//! exchange-pair pattern (user turn + assistant response = one chunk)
//! with per-chunk topic detection, noise stripping, and tool-use
//! formatting.

#![allow(missing_docs)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use hoangsa_memory_store::{ArchiveTracker, VectorCol};

// ---------------------------------------------------------------------------
// constants
// ---------------------------------------------------------------------------

const CHUNK_SIZE: usize = 800;
const MIN_CHUNK_SIZE: usize = 30;
pub const BATCH_SIZE: usize = 100;

/// Cap applied to `archive ingest` on first run (tracker DB empty). A
/// long-lived developer machine can have hundreds of `.jsonl` transcripts
/// in `~/.claude/projects/`; ingesting them all on the very first hook
/// fire would stall a session for minutes and bury the more useful
/// recent transcripts behind retention limits. 30 is enough to give
/// recall something to work with on day one without overwhelming the
/// archive. Pass `--limit 0` (or any explicit `--limit`) to opt out.
pub const INITIAL_INGEST_LIMIT: usize = 30;

/// Retention cap applied at the end of every ingest. Oldest rows get
/// dropped from the tracker; their ChromaDB chunks are cleaned
/// best-effort.
const MAX_ARCHIVE_SESSIONS: i64 = 500;

// ---------------------------------------------------------------------------
// noise stripping
// ---------------------------------------------------------------------------

const NOISE_TAGS: &[&str] = &[
    "system-reminder",
    "command-message",
    "command-name",
    "task-notification",
    "user-prompt-submit-hook",
    "hook_output",
    "local-command-caveat",
    "local-command-stdout",
    "command-args",
];

fn strip_noise(text: &str) -> String {
    let mut result = text.to_string();
    for tag in NOISE_TAGS {
        loop {
            let open = format!("<{tag}");
            let close = format!("</{tag}>");
            let Some(start) = result.find(&open) else {
                break;
            };
            if let Some(end) = result[start..].find(&close) {
                let remove_end = start + end + close.len();
                // eat trailing newline
                let remove_end = if result.as_bytes().get(remove_end) == Some(&b'\n') {
                    remove_end + 1
                } else {
                    remove_end
                };
                result.replace_range(start..remove_end, "");
            } else {
                // unclosed tag — remove to end of line
                let line_end = result[start..]
                    .find('\n')
                    .map(|i| start + i + 1)
                    .unwrap_or(result.len());
                result.replace_range(start..line_end, "");
            }
        }
    }

    // Hook-run chrome: "Ran 2 Stop hook", "Ran 1 PreToolUse hook", etc.
    result = result
        .lines()
        .filter(|line| {
            let trimmed = line.trim().trim_start_matches("> ");
            if trimmed.starts_with("Ran ") {
                !(trimmed.ends_with(" hook") || trimmed.ends_with(" hooks"))
            } else {
                true
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Noise line prefixes
    const NOISE_PREFIXES: &[&str] = &[
        "CURRENT TIME:",
        "VERIFIED FACTS (do not contradict)",
        "AGENT SPECIALIZATION:",
        "Checking verified facts...",
        "Injecting timestamp...",
        "Starting background pipeline...",
        "Checking emotional weights...",
        "Auto-save reminder...",
        "Checking pipeline...",
    ];
    result = result
        .lines()
        .filter(|line| {
            let trimmed = line.trim().trim_start_matches("> ");
            !NOISE_PREFIXES.iter().any(|p| trimmed.starts_with(p))
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Collapsed output: "… +N lines" and "[N tokens] (ctrl+o to expand)"
    result = result
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.starts_with("…") && trimmed.contains("lines"))
        })
        .collect::<Vec<_>>()
        .join("\n");

    // Remove "[N tokens] (ctrl+o to expand)"
    while let Some(start) = result.find("[") {
        if let Some(end) = result[start..].find("(ctrl+o to expand)") {
            let bracket_end = start + end + "(ctrl+o to expand)".len();
            // Check it looks like "[123 tokens] (ctrl+o ...)"
            let inner = &result[start + 1..start + end];
            if inner.contains("tokens") {
                result.replace_range(start..bracket_end, "");
                continue;
            }
        }
        break;
    }

    // Collapse runs of blank lines
    let mut prev_blank = false;
    let mut collapsed = String::with_capacity(result.len());
    for line in result.lines() {
        let blank = line.trim().is_empty();
        if blank && prev_blank {
            continue;
        }
        if !collapsed.is_empty() {
            collapsed.push('\n');
        }
        collapsed.push_str(line);
        prev_blank = blank;
    }

    collapsed.trim().to_string()
}

// ---------------------------------------------------------------------------
// tool use / tool result formatting
// ---------------------------------------------------------------------------

/// Truncate `s` to at most `max_bytes` bytes, snapping back to the
/// previous char boundary so a multi-byte codepoint (em-dash, accented
/// letter, emoji) is never split — slicing a `&str` on a non-boundary
/// byte panics.
fn truncate_on_char_boundary(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

fn format_tool_use(block: &serde_json::Value) -> Option<String> {
    let name = block.get("name")?.as_str()?;
    let input = block
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    Some(match name {
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cmd = truncate_on_char_boundary(cmd, 200);
            format!("[Bash] {cmd}")
        }
        "Read" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let offset = input.get("offset").and_then(|v| v.as_u64());
            let limit = input.get("limit").and_then(|v| v.as_u64());
            match (offset, limit) {
                (Some(o), Some(l)) => format!("[Read {path}:{o}-{}]", o + l),
                _ => format!("[Read {path}]"),
            }
        }
        "Grep" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            let target = input
                .get("path")
                .or_else(|| input.get("glob"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            format!("[Grep] {pattern} in {target}")
        }
        "Glob" => {
            let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
            format!("[Glob] {pattern}")
        }
        "Edit" | "Write" => {
            let path = input
                .get("file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            format!("[{name} {path}]")
        }
        _ => {
            let summary = serde_json::to_string(&input).unwrap_or_default();
            let summary = if summary.len() > 200 {
                format!("{}...", truncate_on_char_boundary(&summary, 200))
            } else {
                summary
            };
            format!("[{name}] {summary}")
        }
    })
}

fn format_tool_result(content: &serde_json::Value, tool_name: &str) -> Option<String> {
    // Read/Edit/Write results omitted (content is in code/git)
    if matches!(tool_name, "Read" | "Edit" | "Write") {
        return None;
    }

    let text = if let Some(s) = content.as_str() {
        s.to_string()
    } else if let Some(arr) = content.as_array() {
        arr.iter()
            .filter_map(|b| {
                if b.get("type")?.as_str()? == "text" {
                    b.get("text")?.as_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        return None;
    };

    let text = text.trim();
    if text.is_empty() {
        return None;
    }

    let lines: Vec<&str> = text.lines().collect();

    Some(match tool_name {
        "Bash" => {
            let n = 20;
            if lines.len() <= n * 2 {
                lines
                    .iter()
                    .map(|l| format!("→ {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                let head: Vec<_> = lines[..n].iter().map(|l| format!("→ {l}")).collect();
                let tail: Vec<_> = lines[lines.len() - n..]
                    .iter()
                    .map(|l| format!("→ {l}"))
                    .collect();
                let omitted = lines.len() - 2 * n;
                format!(
                    "{}\n→ ... [{omitted} lines omitted] ...\n{}",
                    head.join("\n"),
                    tail.join("\n")
                )
            }
        }
        "Grep" | "Glob" => {
            let cap = 20;
            if lines.len() <= cap {
                lines
                    .iter()
                    .map(|l| format!("→ {l}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            } else {
                let kept: Vec<_> = lines[..cap].iter().map(|l| format!("→ {l}")).collect();
                let remaining = lines.len() - cap;
                format!("{}\n→ ... [{remaining} more matches]", kept.join("\n"))
            }
        }
        _ => {
            if text.len() > 2048 {
                format!(
                    "→ {}... [truncated, {} chars]",
                    truncate_on_char_boundary(text, 2048),
                    text.len()
                )
            } else {
                format!("→ {text}")
            }
        }
    })
}

// ---------------------------------------------------------------------------
// topic detection (per-chunk keyword scoring)
// ---------------------------------------------------------------------------

fn detect_topic(text: &str) -> String {
    let lower = text.to_lowercase();
    let sample = truncate_on_char_boundary(&lower, 3000);

    let keywords: &[(&str, &[&str])] = &[
        (
            "technical",
            &[
                "code",
                "python",
                "rust",
                "function",
                "bug",
                "error",
                "api",
                "database",
                "server",
                "deploy",
                "git",
                "test",
                "debug",
                "refactor",
                "compile",
                "build",
                "cargo",
                "npm",
                "typescript",
                "javascript",
            ],
        ),
        (
            "architecture",
            &[
                "architecture",
                "design",
                "pattern",
                "structure",
                "schema",
                "interface",
                "module",
                "component",
                "service",
                "layer",
                "crate",
            ],
        ),
        (
            "planning",
            &[
                "plan",
                "roadmap",
                "milestone",
                "deadline",
                "priority",
                "sprint",
                "backlog",
                "scope",
                "requirement",
                "spec",
                "todo",
            ],
        ),
        (
            "decisions",
            &[
                "decided",
                "chose",
                "picked",
                "switched",
                "migrated",
                "replaced",
                "trade-off",
                "alternative",
                "option",
                "approach",
                "instead",
            ],
        ),
        (
            "problems",
            &[
                "problem",
                "issue",
                "broken",
                "failed",
                "crash",
                "stuck",
                "workaround",
                "fix",
                "solved",
                "resolved",
            ],
        ),
    ];

    let mut best = ("general", 0usize);
    for (topic, kws) in keywords {
        let score: usize = kws.iter().filter(|kw| sample.contains(**kw)).count();
        if score > best.1 {
            best = (topic, score);
        }
    }
    best.0.to_string()
}

// ---------------------------------------------------------------------------
// exchange-pair chunking
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ExchangeChunk {
    pub content: String,
    pub chunk_index: usize,
    pub topic: String,
}

pub fn chunk_exchanges(turns: &[Turn]) -> Vec<ExchangeChunk> {
    let mut chunks = Vec::new();
    let mut i = 0;

    while i < turns.len() {
        let turn = &turns[i];
        if turn.role == "user" {
            let mut content = turn.text.clone();

            // Pair with following assistant turn(s)
            let mut j = i + 1;
            while j < turns.len() && turns[j].role == "assistant" {
                content.push_str("\n\n");
                content.push_str(&turns[j].text);
                j += 1;
            }

            // Split into CHUNK_SIZE pieces if too large
            if content.len() > CHUNK_SIZE {
                let mut offset = 0;
                while offset < content.len() {
                    // Snap `end` down to the nearest UTF-8 char boundary.
                    // `(offset + CHUNK_SIZE).min(content.len())` can
                    // land mid-codepoint (box-drawing glyphs like `─`
                    // are 3 bytes), and slicing mid-codepoint panics.
                    let mut end = (offset + CHUNK_SIZE).min(content.len());
                    while end > offset && !content.is_char_boundary(end) {
                        end -= 1;
                    }
                    // Try to break at a paragraph boundary
                    let slice = &content[offset..end];
                    let break_at = if end < content.len() {
                        slice
                            .rfind("\n\n")
                            .or_else(|| slice.rfind('\n'))
                            .map(|p| offset + p + 1)
                            .unwrap_or(end)
                    } else {
                        end
                    };
                    let part = content[offset..break_at].trim();
                    if part.len() >= MIN_CHUNK_SIZE {
                        let topic = detect_topic(part);
                        chunks.push(ExchangeChunk {
                            content: part.to_string(),
                            chunk_index: chunks.len(),
                            topic,
                        });
                    }
                    offset = break_at;
                }
            } else if content.trim().len() >= MIN_CHUNK_SIZE {
                let topic = detect_topic(&content);
                chunks.push(ExchangeChunk {
                    content: content.trim().to_string(),
                    chunk_index: chunks.len(),
                    topic,
                });
            }

            i = j;
        } else {
            // Orphan assistant turn (no preceding user turn) — still chunk it
            let content = turn.text.trim();
            if content.len() >= MIN_CHUNK_SIZE {
                let topic = detect_topic(content);
                chunks.push(ExchangeChunk {
                    content: content.to_string(),
                    chunk_index: chunks.len(),
                    topic,
                });
            }
            i += 1;
        }
    }

    chunks
}

// ---------------------------------------------------------------------------
// JSONL parsing
// ---------------------------------------------------------------------------

pub struct Turn {
    pub role: String,
    pub text: String,
    pub timestamp: Option<i64>,
}

pub async fn parse_conversation(path: &Path) -> Result<Vec<Turn>> {
    let content = tokio::fs::read_to_string(path)
        .await
        .context("reading conversation file")?;

    let mut turns = Vec::new();
    let mut tool_use_map: HashMap<String, String> = HashMap::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let role = match entry_type {
            "user" | "assistant" => entry_type,
            "" => v.get("role").and_then(|r| r.as_str()).unwrap_or("unknown"),
            _ => continue,
        };

        if matches!(role, "system" | "tool") {
            continue;
        }

        let msg_content = v
            .get("message")
            .and_then(|m| m.get("content"))
            .or_else(|| v.get("content"));

        // Build tool_use_map from assistant messages
        if role == "assistant"
            && let Some(arr) = msg_content.and_then(|c| c.as_array())
        {
            for block in arr {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                    && let Some(id) = block.get("id").and_then(|v| v.as_str())
                {
                    let name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Unknown");
                    tool_use_map.insert(id.to_string(), name.to_string());
                }
            }
        }

        let text = extract_text_rich(msg_content, &tool_use_map);
        if text.is_empty() {
            continue;
        }

        let cleaned = strip_noise(&text);
        if cleaned.is_empty() {
            continue;
        }

        // Check if this is a tool_result-only user message
        let is_tool_only = msg_content
            .and_then(|c| c.as_array())
            .map(|arr| {
                arr.iter()
                    .all(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            })
            .unwrap_or(false);

        if is_tool_only
            && !turns.is_empty()
            && turns.last().map(|t: &Turn| t.role.as_str()) == Some("assistant")
        {
            // Append tool results to previous assistant turn
            let last = turns.last_mut().expect("non-empty checked above");
            last.text.push('\n');
            last.text.push_str(&cleaned);
            continue;
        }

        if role == "assistant"
            && !turns.is_empty()
            && turns.last().map(|t: &Turn| t.role.as_str()) == Some("assistant")
        {
            // Merge consecutive assistant turns (multi-turn tool loop)
            let last = turns.last_mut().expect("non-empty checked above");
            last.text.push('\n');
            last.text.push_str(&cleaned);
            continue;
        }

        let timestamp = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .and_then(chrono_parse_unix)
            .or_else(|| v.get("timestamp").and_then(|t| t.as_i64()));

        turns.push(Turn {
            role: role.to_string(),
            text: cleaned,
            timestamp,
        });
    }
    Ok(turns)
}

fn extract_text_rich(
    content: Option<&serde_json::Value>,
    tool_use_map: &HashMap<String, String>,
) -> String {
    let content = match content {
        Some(c) => c,
        None => return String::new(),
    };

    if let Some(s) = content.as_str() {
        return s.to_string();
    }

    if let Some(arr) = content.as_array() {
        let mut parts = Vec::new();
        for block in arr {
            let block_type = block.get("type").and_then(|t| t.as_str());
            match block_type {
                Some("text") => {
                    if let Some(t) = block.get("text").and_then(|t| t.as_str()) {
                        parts.push(t.to_string());
                    }
                }
                Some("tool_use") => {
                    if let Some(formatted) = format_tool_use(block) {
                        parts.push(formatted);
                    }
                }
                Some("tool_result") => {
                    let tid = block
                        .get("tool_use_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let tname = tool_use_map
                        .get(tid)
                        .map(|s| s.as_str())
                        .unwrap_or("Unknown");
                    let result_content = block
                        .get("content")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);
                    if let Some(formatted) = format_tool_result(&result_content, tname) {
                        parts.push(formatted);
                    }
                }
                _ => {}
            }
        }
        return parts.join("\n").trim().to_string();
    }

    if let Some(s) = content.get("text").and_then(|t| t.as_str()) {
        return s.to_string();
    }

    String::new()
}

// ---------------------------------------------------------------------------
// helpers (moved verbatim from archive_cmd)
// ---------------------------------------------------------------------------

pub fn home_claude_sessions() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot determine home directory")?;
    Ok(home.join(".claude").join("projects"))
}

pub fn decode_project_name(encoded: &str) -> String {
    encoded.replace('-', "/")
}

pub fn infer_topic(turns: &[Turn]) -> String {
    turns
        .iter()
        .find(|t| t.role == "user")
        .map(|t| {
            t.text
                .chars()
                .take(60)
                .collect::<String>()
                .split_whitespace()
                .take(6)
                .collect::<Vec<_>>()
                .join("-")
                .to_lowercase()
        })
        .unwrap_or_else(|| "unknown".to_string())
}

pub fn chrono_parse_unix(s: &str) -> Option<i64> {
    let s = s.trim().trim_end_matches('Z');
    let parts: Vec<&str> = s.splitn(2, 'T').collect();
    if parts.len() != 2 {
        return None;
    }
    let date_parts: Vec<&str> = parts[0].split('-').collect();
    if date_parts.len() != 3 {
        return None;
    }
    let y: i64 = date_parts[0].parse().ok()?;
    let m: i64 = date_parts[1].parse().ok()?;
    let d: i64 = date_parts[2].parse().ok()?;
    let time_part = parts[1].split('.').next()?;
    let time_parts: Vec<&str> = time_part.split(':').collect();
    if time_parts.len() != 3 {
        return None;
    }
    let h: i64 = time_parts[0].parse().ok()?;
    let min: i64 = time_parts[1].parse().ok()?;
    let sec: i64 = time_parts[2].parse().ok()?;
    let days = (y - 1970) * 365 + (y - 1969) / 4 - (y - 1901) / 100 + (y - 1601) / 400;
    let month_days = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let md = month_days.get((m - 1) as usize).copied().unwrap_or(0);
    let leap = if m > 2 && y % 4 == 0 && (y % 100 != 0 || y % 400 == 0) {
        1
    } else {
        0
    };
    Some((days + md + d - 1 + leap) * 86400 + h * 3600 + min * 60 + sec)
}

// ---------------------------------------------------------------------------
// public ingest API
// ---------------------------------------------------------------------------

/// Options for a single [`run_ingest`] invocation. All fields are
/// optional; defaults match the `hoangsa-memory archive ingest` CLI
/// defaults.
#[derive(Debug, Default)]
pub struct IngestOpts {
    pub project_filter: Option<String>,
    pub topic_override: Option<String>,
    pub refresh: bool,
    pub limit: Option<usize>,
}

/// Totals produced by a single [`run_ingest`] run — suitable for
/// surfacing to stdout (CLI) or an MCP `ToolOutput` payload.
#[derive(Debug, Default, Clone, Copy)]
pub struct IngestStats {
    pub total_sessions: u64,
    pub total_chunks: u64,
    pub skipped: u64,
    pub retention_trimmed: u64,
    pub retention_vector_cleaned: u64,
}

/// Ingest conversation sessions from Claude Code into the archive.
///
/// Pulled out of `cmd_archive_ingest` so the MCP daemon can run this
/// inside its existing process (reusing its lazy vector-store handle)
/// instead of each hook-spawned CLI subprocess starting its own. The
/// flock, tracker open, and vector collection open are caller
/// responsibilities — this function takes already-opened handles.
///
/// Returns aggregate [`IngestStats`]. All per-session progress is
/// reported via `tracing` (originally stdout; stdout/stderr belongs to
/// the CLI, not library code).
pub async fn run_ingest(
    tracker: &ArchiveTracker,
    col: &dyn VectorCol,
    opts: IngestOpts,
) -> Result<IngestStats> {
    let IngestOpts {
        project_filter,
        topic_override,
        refresh,
        limit,
    } = opts;

    let sessions_root = home_claude_sessions()?;
    if !sessions_root.is_dir() {
        bail!("No Claude sessions found at {}", sessions_root.display());
    }

    // First-run cap: if the tracker is empty and the caller didn't
    // pass an explicit limit, fall back to INITIAL_INGEST_LIMIT.
    // `Some(0)` means "no cap" (explicit opt-out).
    let effective_limit: Option<usize> = match limit {
        Some(0) => None,
        Some(n) => Some(n),
        None => {
            let (existing_sessions, _, _) = tracker.status()?;
            if existing_sessions == 0 {
                Some(INITIAL_INGEST_LIMIT)
            } else {
                None
            }
        }
    };

    let mut stats = IngestStats::default();

    // Collect every (project, session_id, path, mtime) tuple across
    // all project dirs up front, so we can apply a global most-recent
    // cap when `effective_limit` is set. Without the cross-project
    // sort, a cap of 30 could be blown entirely on one noisy project
    // while the current one gets nothing.
    struct Candidate {
        project_name: String,
        session_id: String,
        path: PathBuf,
        mtime: std::time::SystemTime,
    }

    let mut project_dirs: Vec<_> = std::fs::read_dir(&sessions_root)
        .context("reading Claude projects dir")?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    project_dirs.sort_by_key(|e| e.file_name());

    let mut candidates: Vec<Candidate> = Vec::new();
    for project_entry in project_dirs {
        let project_name = decode_project_name(&project_entry.file_name().to_string_lossy());
        if let Some(filter) = project_filter.as_deref()
            && project_name != filter
        {
            continue;
        }

        // New layout: JSONL files directly in project dir.
        if let Ok(rd) = std::fs::read_dir(project_entry.path()) {
            for entry in rd.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".jsonl") {
                    let path = entry.path();
                    let mtime = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    candidates.push(Candidate {
                        project_name: project_name.clone(),
                        session_id: name.trim_end_matches(".jsonl").to_string(),
                        path,
                        mtime,
                    });
                }
            }
        }

        // Old layout: sessions/<id>/conversation.jsonl
        let sessions_dir = project_entry.path().join("sessions");
        if sessions_dir.is_dir()
            && let Ok(rd) = std::fs::read_dir(&sessions_dir)
        {
            for entry in rd.filter_map(|e| e.ok()) {
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let session_id = entry.file_name().to_string_lossy().to_string();
                    let convo_file = entry.path().join("conversation.jsonl");
                    if convo_file.is_file() {
                        let mtime = convo_file
                            .metadata()
                            .and_then(|m| m.modified())
                            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                        candidates.push(Candidate {
                            project_name: project_name.clone(),
                            session_id,
                            path: convo_file,
                            mtime,
                        });
                    }
                }
            }
        }
    }

    // Most-recent-first global ordering — mtime descending.
    candidates.sort_by(|a, b| b.mtime.cmp(&a.mtime));

    if let Some(cap) = effective_limit
        && candidates.len() > cap
    {
        let dropped = candidates.len() - cap;
        candidates.truncate(cap);
        tracing::info!(
            cap,
            dropped,
            "archive ingest: limit applied — keeping most-recent session files"
        );
    }

    // Iterate in (project, session_id) order so logs read naturally
    // even though selection used mtime.
    candidates.sort_by(|a, b| {
        a.project_name
            .cmp(&b.project_name)
            .then_with(|| a.session_id.cmp(&b.session_id))
    });

    for Candidate {
        project_name,
        session_id,
        path: convo_file,
        ..
    } in candidates
    {
        if !refresh && tracker.is_ingested(&session_id)? {
            stats.skipped += 1;
            continue;
        }
        // Idempotency: hash the raw transcript bytes up-front and skip
        // the entire session (even in refresh mode) when the hash
        // matches what we last ingested. Without this, PreCompact +
        // SessionEnd hooks force every session file through
        // parse → chunk → embed on every fire even when the bytes
        // haven't changed — the re-embed loop that preceded the 164GB
        // disk-fill incident (see RESEARCH.md).
        let raw_bytes = match tokio::fs::read(&convo_file).await {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(session = %session_id, error = %e, "skipping unreadable session file");
                continue;
            }
        };
        let content_hash = blake3::hash(&raw_bytes).to_hex().to_string();
        if tracker.is_ingested(&session_id)?
            && tracker.content_hash(&session_id)?.as_deref() == Some(content_hash.as_str())
        {
            stats.skipped += 1;
            continue;
        }
        // In refresh mode, drop any pre-existing chunks for this
        // session so shifted chunk boundaries don't leave orphans
        // alongside the freshly upserted rows.
        if refresh && tracker.is_ingested(&session_id)? {
            let filter = serde_json::json!({ "session_id": { "$eq": session_id } });
            if let Err(e) = col.delete_by_filter(filter).await {
                tracing::warn!(
                    session = %session_id,
                    error = %e,
                    "refresh: vector delete failed — chunks will be upserted but orphans may remain",
                );
            }
        }

        let turns = match parse_conversation(&convo_file).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(session = %session_id, error = %e, "skipping unparseable session");
                continue;
            }
        };

        if turns.is_empty() {
            continue;
        }

        let chunks = chunk_exchanges(&turns);
        if chunks.is_empty() {
            continue;
        }

        let session_topic = topic_override
            .clone()
            .unwrap_or_else(|| infer_topic(&turns));

        let mut ids = Vec::with_capacity(chunks.len());
        let mut documents = Vec::with_capacity(chunks.len());
        let mut metadatas = Vec::with_capacity(chunks.len());

        for chunk in &chunks {
            // Deterministic ID: blake3(session_id + chunk_index)
            let hash_input = format!("{session_id}:{}", chunk.chunk_index);
            let hash = blake3::hash(hash_input.as_bytes()).to_hex().to_string();
            ids.push(format!("cx_{}", &hash[..24]));
            documents.push(chunk.content.clone());

            let mut meta = std::collections::HashMap::new();
            meta.insert(
                "session_id".to_string(),
                serde_json::Value::String(session_id.clone()),
            );
            meta.insert(
                "chunk_index".to_string(),
                serde_json::Value::Number(serde_json::Number::from(chunk.chunk_index as i64)),
            );
            meta.insert(
                "topic".to_string(),
                serde_json::Value::String(chunk.topic.clone()),
            );
            meta.insert(
                "session_topic".to_string(),
                serde_json::Value::String(session_topic.clone()),
            );
            meta.insert(
                "project".to_string(),
                serde_json::Value::String(project_name.clone()),
            );
            meta.insert(
                "ingest_mode".to_string(),
                serde_json::Value::String("exchange_pair".to_string()),
            );
            metadatas.push(meta);
        }

        // Upsert in batches
        for start in (0..ids.len()).step_by(BATCH_SIZE) {
            let end = (start + BATCH_SIZE).min(ids.len());
            col.upsert(
                ids[start..end].to_vec(),
                Some(documents[start..end].to_vec()),
                Some(metadatas[start..end].to_vec()),
            )
            .await
            .with_context(|| format!("upserting session {session_id}"))?;
        }

        tracker.upsert_session(
            &session_id,
            &project_name,
            &session_topic,
            chunks.len() as i64,
            &content_hash,
        )?;
        stats.total_sessions += 1;
        stats.total_chunks += chunks.len() as u64;
        tracing::info!(
            session = %session_id,
            project = %project_name,
            chunks = chunks.len(),
            topic = %session_topic,
            "archive ingest: session upserted"
        );
    }

    // Retention cap: keep only the most recent MAX_ARCHIVE_SESSIONS.
    // Oldest rows get dropped from the tracker; their vector chunks
    // are cleaned best-effort against the SAME `col` handle so we
    // don't pay the embedder-init cost twice.
    let (sessions, _, _) = tracker.status()?;
    if sessions > MAX_ARCHIVE_SESSIONS {
        let excess = sessions - MAX_ARCHIVE_SESSIONS;
        let to_drop = tracker.oldest_sessions(excess)?;
        let mut vector_cleaned = 0u64;
        for sid in &to_drop {
            tracker.delete_session(sid)?;
            let filter = serde_json::json!({ "session_id": { "$eq": sid } });
            if col.delete_by_filter(filter).await.is_ok() {
                vector_cleaned += 1;
            }
        }
        stats.retention_trimmed = to_drop.len() as u64;
        stats.retention_vector_cleaned = vector_cleaned;
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_on_char_boundary_snaps_back_on_multibyte() {
        // `—` (U+2014 EM DASH) is 3 bytes. If max_bytes lands inside it,
        // we must snap back to the previous boundary — the original bug
        // was a raw `&s[..max_bytes]` which panicked here.
        let s = "ab—cd";
        assert_eq!(truncate_on_char_boundary(s, 3), "ab");
        assert_eq!(truncate_on_char_boundary(s, 4), "ab");
        assert_eq!(truncate_on_char_boundary(s, 5), "ab—");
        assert_eq!(truncate_on_char_boundary(s, 100), s);
    }

    #[test]
    fn truncate_on_char_boundary_handles_the_real_panic_input() {
        // Regression: repro of the archive.rs:184 panic where byte 200
        // fell inside an em-dash in a Bash command string.
        let prefix = "x".repeat(198);
        let s = format!("{prefix}—tail");
        assert_eq!(s.len(), 198 + 3 + 4);
        let out = truncate_on_char_boundary(&s, 200);
        assert_eq!(out.len(), 198);
        assert!(out.is_char_boundary(out.len()));
    }

    #[test]
    fn format_tool_use_bash_with_multibyte_does_not_panic() {
        let prefix = "x".repeat(198);
        let cmd = format!("{prefix}— trailing comment past 200");
        let block = serde_json::json!({
            "name": "Bash",
            "input": { "command": cmd },
        });
        let out = format_tool_use(&block).unwrap();
        assert!(out.starts_with("[Bash] "));
    }
}
