//! `hoangsa-memory archive` subcommands — ingest, status, topics, search, curate.
//!
//! Conversation mining and exchange-pair chunking used to live here;
//! the core pipeline now sits in [`hoangsa_memory_retrieve::archive`] so the MCP
//! daemon can run ingests in-process (reusing its ChromaDB sidecar)
//! instead of each hook-spawned CLI subprocess starting its own. This
//! file is now a thin CLI shell: argument parsing, advisory flock,
//! stdout formatting.

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
        /// Bypass the "already-ingested" skip and re-ingest every matching
        /// session. Chunks are upserted; orphan chunks for shifted chunk
        /// boundaries are cleaned up by an explicit chroma delete before
        /// the fresh upsert. Hooks (PreCompact / SessionEnd) pass this
        /// flag so sessions that grew since last ingest actually get
        /// their new turns.
        #[arg(long)]
        refresh: bool,
        /// Cap ingest at the N most recently modified session files
        /// (across all projects). Useful for bounded first-run
        /// backfills on a machine with hundreds of legacy transcripts.
        /// When the tracker is empty and this flag is not set, an
        /// implicit cap of `INITIAL_INGEST_LIMIT` is applied
        /// automatically; pass `--limit 0` to opt out.
        #[arg(long)]
        limit: Option<usize>,
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
    /// Remove archived sessions. Either purge by age (`--older-than 30d`)
    /// or nuke everything (`--all`). Frees up tracker rows; ChromaDB
    /// chunks for the removed sessions are deleted by best-effort metadata
    /// query — failures leave orphans but never abort the purge.
    Purge {
        /// Retention window. Accepts `Nd` (days), `Nh` (hours), or
        /// `Nm` (minutes). Sessions ingested before `now - duration`
        /// are deleted. Mutually exclusive with `--all`.
        #[arg(long, value_name = "DURATION", conflicts_with = "all")]
        older_than: Option<String>,
        /// Delete every archived session.
        #[arg(long, conflicts_with = "older_than")]
        all: bool,
        /// Print what would happen without modifying anything.
        #[arg(long)]
        dry_run: bool,
    },
}

use anyhow::{Context, Result, bail};
use hoangsa_memory_retrieve::archive::{IngestOpts, run_ingest};
use hoangsa_memory_store::{ArchiveTracker, ChromaCol, ChromaStore, StoreRoot};

// ---------------------------------------------------------------------------
// ingest command — thin CLI wrapper around `hoangsa_memory_retrieve::archive::run_ingest`
// ---------------------------------------------------------------------------

/// Ingest conversation sessions from Claude Code into the archive.
///
/// Acquires the advisory flock so concurrent hook fires (PreCompact /
/// SessionEnd from multiple Claude Code sessions) don't pile up Python
/// ChromaDB sidecars, opens the tracker + collection, then delegates
/// to [`hoangsa_memory_retrieve::archive::run_ingest`] for the actual work.
pub async fn cmd_archive_ingest(
    root: &Path,
    project_filter: Option<&str>,
    topic_override: Option<&str>,
    refresh: bool,
    limit: Option<usize>,
) -> Result<()> {
    // Advisory flock — PreCompact/SessionEnd hooks fire-and-forget a detached
    // ingest subprocess, and each one spins up its own ChromaDB Python sidecar
    // (~500 MB RSS per process). Without serialisation, two concurrent Claude
    // Code sessions (or a compact event while a previous ingest still runs)
    // pile up sidecars and exhaust memory. If the lock is held we exit
    // cleanly — the running ingest will pick up any new turns on its next
    // pass anyway, so there's nothing to retry.
    let _lock: Option<std::fs::File> = match acquire_ingest_lock() {
        Ok(Some(l)) => Some(l),
        Ok(None) => {
            eprintln!("hoangsa-memory: another archive ingest is running; skipping.");
            return Ok(());
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to acquire archive ingest lock; proceeding anyway");
            None
        }
    };

    let tracker = open_tracker(root).await?;
    let col = open_archive_chroma(root).await?;

    let opts = IngestOpts {
        project_filter: project_filter.map(str::to_string),
        topic_override: topic_override.map(str::to_string),
        refresh,
        limit,
    };

    let stats = run_ingest(&tracker, &col, opts).await?;

    println!(
        "\nIngested {total_sessions} sessions ({total_chunks} chunks), skipped {skipped} already-ingested.",
        total_sessions = stats.total_sessions,
        total_chunks = stats.total_chunks,
        skipped = stats.skipped,
    );

    if stats.retention_trimmed > 0 {
        const MAX_ARCHIVE_SESSIONS: i64 = 500;
        println!(
            "Retention: trimmed {} oldest session(s), cleaned {} from ChromaDB (cap = {MAX_ARCHIVE_SESSIONS}).",
            stats.retention_trimmed, stats.retention_chroma_cleaned,
        );
    }
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
            conditions.into_iter().next().unwrap_or(serde_json::Value::Null)
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

/// Parse a short duration like `30d`, `12h`, `45m` into seconds.
fn parse_duration(s: &str) -> Result<i64> {
    let s = s.trim();
    let (num, unit) = s
        .find(|c: char| !c.is_ascii_digit())
        .map(|i| s.split_at(i))
        .ok_or_else(|| anyhow::anyhow!("missing unit (expected d/h/m): {s}"))?;
    let n: i64 = num
        .parse()
        .with_context(|| format!("invalid number in duration: {s}"))?;
    let secs = match unit {
        "d" => n * 86400,
        "h" => n * 3600,
        "m" => n * 60,
        other => bail!("unknown duration unit {other:?} (expected d/h/m)"),
    };
    Ok(secs)
}

/// Drop archived sessions by age or wipe them all.
pub async fn cmd_archive_purge(
    root: &Path,
    older_than: Option<&str>,
    all: bool,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let tracker = open_tracker(root).await?;

    // Resolve target: either "< cutoff" or "everything".
    let (label, ids) = if all {
        let all_ids = if dry_run {
            tracker.oldest_sessions(i64::MAX).unwrap_or_default()
        } else {
            tracker.purge_all()?
        };
        ("all".to_string(), all_ids)
    } else if let Some(spec) = older_than {
        let secs = parse_duration(spec)?;
        let cutoff = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
            - secs;
        let ids = if dry_run {
            tracker.sessions_older_than(cutoff)?
        } else {
            tracker.purge_older_than(cutoff)?
        };
        (format!("older than {spec} (unix cutoff = {cutoff})"), ids)
    } else {
        bail!("specify --older-than <dur> or --all");
    };

    if dry_run {
        println!(
            "dry-run: would purge {} session(s) ({label}). Re-run without --dry-run.",
            ids.len()
        );
        return Ok(());
    }

    // Best-effort ChromaDB cleanup for the removed sessions.
    let chroma_removed = match open_archive_chroma(root).await {
        Ok(col) => {
            let mut removed = 0u64;
            for sid in &ids {
                let filter = serde_json::json!({ "session_id": { "$eq": sid } });
                if let Err(e) = col.delete_by_filter(filter).await {
                    tracing::warn!(session = %sid, error = %e, "chroma delete failed");
                } else {
                    removed += 1;
                }
            }
            removed
        }
        Err(e) => {
            tracing::warn!(error = %e, "chroma unreachable — leaving archive chunks orphaned");
            0
        }
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "removed_sessions": ids.len(),
                "chroma_sessions_cleaned": chroma_removed,
                "ids": ids,
            }))?
        );
    } else {
        println!(
            "Purged {} session(s) from tracker, {} from ChromaDB.",
            ids.len(),
            chroma_removed
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Try to acquire the process-wide archive-ingest advisory lock at
/// `~/.hoangsa/memory/archive-ingest.lock`. Returns `Ok(Some(file))` when
/// the lock is held by this process (caller keeps the handle alive until
/// ingest finishes), `Ok(None)` when another process already holds it, and
/// `Err` when the lockfile itself can't be opened.
fn acquire_ingest_lock() -> Result<Option<std::fs::File>> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .context("cannot determine home directory for archive-ingest lock")?;
    let dir = PathBuf::from(home).join(".hoangsa").join("memory");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create {} for ingest lock", dir.display()))?;
    let path = dir.join("archive-ingest.lock");

    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("open ingest lock {}", path.display()))?;

    // `try_lock` returns `Err(TryLockError::WouldBlock)` when contended and
    // `Err(TryLockError::Error(_))` on a real I/O failure. We collapse both
    // into `Ok(None)` — at worst we skip this ingest run, which is safe:
    // the next PreCompact/SessionEnd hook will retry.
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(_) => Ok(None),
    }
}

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
        .ensure_collection("hoangsa_memory_archive")
        .await
        .context("ensuring hoangsa_memory_archive collection in ChromaDB")?;
    Ok(col)
}

async fn load_chroma_data_path(root: &Path) -> String {
    let cfg = hoangsa_memory_retrieve::ChromaConfig::load_or_default(root).await;
    cfg.data_path
        .unwrap_or_else(|| StoreRoot::chroma_path(root).to_string_lossy().to_string())
}
