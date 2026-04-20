//! `thoth archive` subcommands — ingest, status, topics, search, curate.
//!
//! Conversation mining follows the MemPalace exchange-pair pattern:
//! user turn + AI response = one chunk. Chunks are embedded into ChromaDB
//! with per-chunk topic detection, noise stripping, and tool-use formatting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(clap::Subcommand, Debug)]
pub enum ArchiveCmd {
    /// Ingest conversation sessions from Claude Code into ChromaDB.
    Ingest {
        /// Only ingest sessions from this project.
        #[arg(long)]
        project: Option<String>,
        /// Override the auto-detected topic for all ingested sessions.
        #[arg(long)]
        topic: Option<String>,
    },
    /// Show archive summary (session count, turn count, curated count).
    Status,
    /// List topics with session and turn counts.
    Topics {
        /// Filter by project.
        #[arg(long)]
        project: Option<String>,
    },
    /// Semantic search across archived conversations.
    Search {
        /// Maximum results to return.
        #[arg(short = 'k', long, default_value_t = 10)]
        top_k: usize,
        /// Filter by project.
        #[arg(long)]
        project: Option<String>,
        /// Filter by topic.
        #[arg(long)]
        topic: Option<String>,
        /// Query text.
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Extract facts/lessons from uncurated archived sessions via LLM.
    Curate {
        /// LLM backend: `auto`, `cli`, or `api`.
        #[arg(long, default_value = "")]
        backend: String,
        /// LLM model name.
        #[arg(long, default_value = "")]
        model: String,
        /// Maximum number of sessions to curate in one run.
        #[arg(long, default_value_t = 5)]
        max_sessions: usize,
    },
}

use anyhow::{Context, Result, bail};
use thoth_store::{ArchiveTracker, ChromaCol, ChromaStore, StoreRoot};

// ---------------------------------------------------------------------------
// constants
// ---------------------------------------------------------------------------

const CHUNK_SIZE: usize = 800;
const MIN_CHUNK_SIZE: usize = 30;
const BATCH_SIZE: usize = 100;

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

fn format_tool_use(block: &serde_json::Value) -> Option<String> {
    let name = block.get("name")?.as_str()?;
    let input = block
        .get("input")
        .cloned()
        .unwrap_or(serde_json::Value::Null);

    Some(match name {
        "Bash" => {
            let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let cmd = if cmd.len() > 200 { &cmd[..200] } else { cmd };
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
                format!("{}...", &summary[..200])
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
                format!("→ {}... [truncated, {} chars]", &text[..2048], text.len())
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
    let sample = if lower.len() > 3000 {
        &lower[..3000]
    } else {
        &lower
    };

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
struct ExchangeChunk {
    content: String,
    chunk_index: usize,
    topic: String,
}

fn chunk_exchanges(turns: &[Turn]) -> Vec<ExchangeChunk> {
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
                    let end = (offset + CHUNK_SIZE).min(content.len());
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

struct Turn {
    role: String,
    text: String,
    #[allow(dead_code)]
    timestamp: Option<i64>,
}

async fn parse_conversation(path: &Path) -> Result<Vec<Turn>> {
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
            let last = turns.last_mut().unwrap();
            last.text.push('\n');
            last.text.push_str(&cleaned);
            continue;
        }

        if role == "assistant"
            && !turns.is_empty()
            && turns.last().map(|t: &Turn| t.role.as_str()) == Some("assistant")
        {
            // Merge consecutive assistant turns (multi-turn tool loop)
            let last = turns.last_mut().unwrap();
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
// ingest command
// ---------------------------------------------------------------------------

/// Ingest conversation sessions from Claude Code into the archive.
///
/// Scans `~/.claude/projects/*/sessions/*/conversation.jsonl`, parses
/// each session into exchange-pair chunks (user + AI = 1 unit), strips
/// noise, formats tool use, detects topic per chunk, and upserts into
/// ChromaDB `thoth_archive`.
pub async fn cmd_archive_ingest(
    root: &Path,
    project_filter: Option<&str>,
    topic_override: Option<&str>,
) -> Result<()> {
    let tracker = open_tracker(root).await?;
    let col = open_archive_chroma(root).await?;

    let sessions_root = home_claude_sessions()?;
    if !sessions_root.is_dir() {
        bail!("No Claude sessions found at {}", sessions_root.display());
    }

    let mut total_sessions = 0u64;
    let mut total_chunks = 0u64;
    let mut skipped = 0u64;

    let mut project_dirs: Vec<_> = std::fs::read_dir(&sessions_root)
        .context("reading Claude projects dir")?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .collect();
    project_dirs.sort_by_key(|e| e.file_name());

    for project_entry in project_dirs {
        let project_name = decode_project_name(&project_entry.file_name().to_string_lossy());
        if let Some(filter) = project_filter
            && project_name != filter
        {
            continue;
        }

        let mut convo_files: Vec<(String, PathBuf)> = Vec::new();

        // New layout: JSONL files directly in project dir.
        if let Ok(rd) = std::fs::read_dir(project_entry.path()) {
            for entry in rd.filter_map(|e| e.ok()) {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".jsonl") {
                    let session_id = name.trim_end_matches(".jsonl").to_string();
                    convo_files.push((session_id, entry.path()));
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
                        convo_files.push((session_id, convo_file));
                    }
                }
            }
        }

        convo_files.sort_by(|a, b| a.0.cmp(&b.0));

        for (session_id, convo_file) in convo_files {
            if tracker.is_ingested(&session_id)? {
                skipped += 1;
                continue;
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
                .map(|t| t.to_string())
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
            )?;
            total_sessions += 1;
            total_chunks += chunks.len() as u64;
            println!(
                "  + {session_id} ({project_name}) → {} chunks [{}]",
                chunks.len(),
                session_topic
            );
        }
    }

    println!(
        "\nIngested {total_sessions} sessions ({total_chunks} chunks), skipped {skipped} already-ingested."
    );
    Ok(())
}

/// Print archive status.
pub async fn cmd_archive_status(root: &Path, json: bool) -> Result<()> {
    let tracker = open_tracker(root).await?;
    let (sessions, turns, curated) = tracker.status()?;

    if json {
        let obj = serde_json::json!({
            "sessions": sessions,
            "chunks": turns,
            "curated": curated,
        });
        println!("{}", serde_json::to_string_pretty(&obj)?);
    } else {
        println!("Archive: {sessions} sessions, {turns} chunks ({curated} curated)");
    }
    Ok(())
}

/// List topics.
pub async fn cmd_archive_topics(root: &Path, project: Option<&str>, json: bool) -> Result<()> {
    let tracker = open_tracker(root).await?;
    let topics = tracker.topics(project)?;

    if json {
        let arr: Vec<_> = topics
            .iter()
            .map(|t| {
                serde_json::json!({
                    "topic": t.topic,
                    "sessions": t.session_count,
                    "chunks": t.total_turns,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else {
        if topics.is_empty() {
            println!("No topics found.");
            return Ok(());
        }
        for t in &topics {
            println!(
                "  {:<30} {} sessions, {} chunks",
                t.topic, t.session_count, t.total_turns
            );
        }
    }
    Ok(())
}

/// Semantic search across the archive with neighbor expansion.
pub async fn cmd_archive_search(
    root: &Path,
    query: &str,
    top_k: usize,
    project: Option<&str>,
    topic: Option<&str>,
    json: bool,
) -> Result<()> {
    let col = open_archive_chroma(root).await?;

    let mut filter = None;
    if project.is_some() || topic.is_some() {
        let mut conditions = Vec::new();
        if let Some(p) = project {
            conditions.push(serde_json::json!({"project": {"$eq": p}}));
        }
        if let Some(t) = topic {
            conditions.push(serde_json::json!({"topic": {"$eq": t}}));
        }
        filter = Some(if conditions.len() == 1 {
            conditions.into_iter().next().unwrap()
        } else {
            serde_json::json!({"$and": conditions})
        });
    }

    // Over-fetch for neighbor expansion
    let hits = col.query_text(query, top_k * 2, filter).await?;

    // Neighbor expansion: for each hit, fetch ±1 adjacent chunks
    let mut expanded_hits = Vec::new();
    let mut seen_sessions: HashMap<String, usize> = HashMap::new();

    for h in &hits {
        let session_id = h
            .metadata
            .as_ref()
            .and_then(|m| m.get("session_id"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let chunk_index = h
            .metadata
            .as_ref()
            .and_then(|m| m.get("chunk_index"))
            .and_then(|v| v.as_i64())
            .unwrap_or(-1);

        // Skip if we've seen too many from this session
        let count = seen_sessions.entry(session_id.to_string()).or_insert(0);
        if *count >= 3 {
            continue;
        }
        *count += 1;

        // Try to fetch neighbor chunks for context
        let mut context_text = h.document.clone().unwrap_or_default();
        if chunk_index >= 0 && !session_id.is_empty() {
            for offset in [-1i64, 1] {
                let neighbor_idx = chunk_index + offset;
                if neighbor_idx < 0 {
                    continue;
                }
                let neighbor_filter = serde_json::json!({
                    "$and": [
                        {"session_id": {"$eq": session_id}},
                        {"chunk_index": {"$eq": neighbor_idx}}
                    ]
                });
                if let Ok(neighbors) = col.query_text("", 1, Some(neighbor_filter)).await {
                    for n in &neighbors {
                        if let Some(doc) = &n.document {
                            if offset < 0 {
                                context_text = format!("{doc}\n\n---\n\n{context_text}");
                            } else {
                                context_text = format!("{context_text}\n\n---\n\n{doc}");
                            }
                        }
                    }
                }
            }
        }

        expanded_hits.push((h, context_text));
        if expanded_hits.len() >= top_k {
            break;
        }
    }

    if json {
        let arr: Vec<_> = expanded_hits
            .iter()
            .map(|(h, ctx)| {
                serde_json::json!({
                    "id": h.id,
                    "distance": h.distance,
                    "text": ctx,
                    "metadata": h.metadata,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&arr)?);
    } else {
        if expanded_hits.is_empty() {
            println!("No results.");
            return Ok(());
        }
        for (h, ctx) in &expanded_hits {
            let session = h
                .metadata
                .as_ref()
                .and_then(|m| m.get("session_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let topic = h
                .metadata
                .as_ref()
                .and_then(|m| m.get("topic"))
                .and_then(|v| v.as_str())
                .unwrap_or("?");
            let preview = ctx.chars().take(200).collect::<String>();
            println!("  [{topic}] (d={:.3}, session={session})", h.distance);
            println!("    {preview}");
            println!();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

async fn open_tracker(root: &Path) -> Result<ArchiveTracker> {
    let path = StoreRoot::archive_path(root);
    ArchiveTracker::open(&path)
        .await
        .context("opening archive tracker")
}

async fn open_archive_chroma(root: &Path) -> Result<ChromaCol> {
    let path = load_chroma_data_path(root).await;
    let store = ChromaStore::open(&path)
        .await
        .context("starting ChromaDB sidecar")?;
    let (col, _info) = store
        .ensure_collection("thoth_archive")
        .await
        .context("ensuring thoth_archive collection in ChromaDB")?;
    Ok(col)
}

async fn load_chroma_data_path(root: &Path) -> String {
    let cfg = thoth_retrieve::ChromaConfig::load_or_default(root).await;
    cfg.data_path
        .unwrap_or_else(|| StoreRoot::chroma_path(root).to_string_lossy().to_string())
}

fn home_claude_sessions() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("cannot determine home directory")?;
    Ok(home.join(".claude").join("projects"))
}

fn decode_project_name(encoded: &str) -> String {
    encoded.replace('-', "/")
}

fn infer_topic(turns: &[Turn]) -> String {
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

fn chrono_parse_unix(s: &str) -> Option<i64> {
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
// curate
// ---------------------------------------------------------------------------

/// Extract facts/lessons from uncurated archive sessions via LLM.
pub async fn cmd_archive_curate(
    root: &Path,
    backend: &str,
    model: &str,
    max_sessions: usize,
) -> Result<()> {
    use thoth_memory::background_review::{parse_review_response, persist_review};

    let tracker = open_tracker(root).await?;
    let uncurated = tracker.uncurated_sessions()?;

    if uncurated.is_empty() {
        println!("No uncurated sessions.");
        return Ok(());
    }

    let col = open_archive_chroma(root).await?;

    let to_process = uncurated.into_iter().take(max_sessions);
    let mut total_facts = 0usize;
    let mut total_lessons = 0usize;

    for session in to_process {
        let filter = serde_json::json!({"session_id": {"$eq": session.session_id}});
        let hits = col
            .query_text("", session.turn_count.min(50) as usize, Some(filter))
            .await;

        let turns_text = match hits {
            Ok(hits) if !hits.is_empty() => {
                // Sort by chunk_index for coherent reading order
                let mut indexed: Vec<_> = hits
                    .iter()
                    .map(|h| {
                        let idx = h
                            .metadata
                            .as_ref()
                            .and_then(|m| m.get("chunk_index"))
                            .and_then(|v| v.as_i64())
                            .unwrap_or(0);
                        (idx, h)
                    })
                    .collect();
                indexed.sort_by_key(|(idx, _)| *idx);

                indexed
                    .iter()
                    .filter_map(|(_, h)| h.document.as_deref())
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n")
            }
            _ => {
                tracing::debug!(session = %session.session_id, "no turns found, skipping");
                continue;
            }
        };

        let truncated = if turns_text.len() > 8000 {
            &turns_text[..8000]
        } else {
            &turns_text
        };

        let prompt = render_curation_prompt(&session.project, &session.topic, truncated);

        let response = match crate::review::call_backend(&prompt, backend, model).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(session = %session.session_id, error = %e, "LLM call failed");
                continue;
            }
        };

        let output = match parse_review_response(&response) {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(session = %session.session_id, error = %e, "failed to parse LLM response");
                continue;
            }
        };

        let fact_count = output.facts.len();
        let lesson_count = output.lessons.len();

        match persist_review(root, output).await {
            Ok(report) => {
                total_facts += report.facts_added;
                total_lessons += report.lessons_added;
            }
            Err(e) => {
                tracing::warn!(session = %session.session_id, error = %e, "persist failed");
                continue;
            }
        }

        tracker.mark_curated(&session.session_id)?;
        println!(
            "  curated session {} (project={}, topic={}): {} facts, {} lessons",
            session.session_id, session.project, session.topic, fact_count, lesson_count
        );
    }

    println!("Curation complete: {total_facts} facts, {total_lessons} lessons extracted.");
    Ok(())
}

fn render_curation_prompt(project: &str, topic: &str, conversation: &str) -> String {
    format!(
        r#"You are a memory curator. Below is a verbatim conversation from project "{project}" (topic: "{topic}"). Extract durable knowledge worth remembering across future sessions.

## Conversation
{conversation}

## Instructions
Return ONLY valid JSON (no markdown fences, no commentary):
{{"facts":[{{"text":"...","tags":["..."]}}],"lessons":[{{"trigger":"...","advice":"..."}}],"skills":[]}}

Quality gates — only include entries that:
- Save a future session at least one round-trip
- Encode a decision, convention, or non-obvious pattern
- Are specific and actionable

Cap output at 3 facts + 3 lessons per conversation. Prefer precision over volume.

If nothing is worth saving, return: {{"facts":[],"lessons":[],"skills":[]}}"#
    )
}
