//! `thoth memory` subcommands — show, edit, fact, lesson, forget, pending, etc.

use std::path::Path;

#[derive(clap::Subcommand, Debug)]
pub enum MemoryCmd {
    /// Print `MEMORY.md` and `LESSONS.md`.
    Show,
    /// Open `MEMORY.md` in `$EDITOR`.
    Edit,
    /// Append a new fact to `MEMORY.md`.
    Fact {
        /// Optional comma-separated tags (`--tags a,b,c`).
        #[arg(long)]
        tags: Option<String>,
        /// Fact text (joined with spaces).
        #[arg(required = true)]
        text: Vec<String>,
    },
    /// Append a new lesson to `LESSONS.md`.
    Lesson {
        /// Trigger pattern — when this lesson should fire.
        #[arg(long, required = true)]
        when: String,
        /// Advice / rule / warning (joined with spaces).
        #[arg(required = true)]
        advice: Vec<String>,
    },
    /// Run the forgetting pass (TTL + capacity eviction over the episodic log).
    Forget,
    /// Mode::Full: ask the synthesizer to critique recent outcomes and
    /// append any high-value proposed lessons to `LESSONS.md`.
    Nudge {
        /// How many recent episodes to consider. `0` uses the default (64).
        #[arg(long, default_value_t = 0)]
        window: usize,
    },
    /// List entries staged in `MEMORY.pending.md` / `LESSONS.pending.md`.
    Pending,
    /// Promote a staged fact or lesson into the canonical markdown file.
    Promote {
        /// `fact` or `lesson`.
        #[arg(value_parser = ["fact", "lesson"])]
        kind: String,
        /// 0-based index shown by `thoth memory pending`.
        index: usize,
    },
    /// Drop a staged fact or lesson without promoting it.
    Reject {
        /// `fact` or `lesson`.
        #[arg(value_parser = ["fact", "lesson"])]
        kind: String,
        /// 0-based index shown by `thoth memory pending`.
        index: usize,
        /// Optional reason — recorded in memory-history.jsonl.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Tail the memory-history.jsonl audit log.
    Log {
        /// Return only the latest N entries.
        #[arg(long)]
        limit: Option<usize>,
    },
    /// One-shot triage of legacy MEMORY.md / LESSONS.md into the
    /// three-surface taxonomy (MEMORY / LESSONS / USER). Classifies each
    /// entry as Keep / Move-to-USER.md / Drop per the DESIGN-SPEC §REQ-09
    /// heuristics and applies via the audit-logged replace / remove /
    /// `append_preference` verbs.
    Migrate {
        /// Skip the interactive confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Use an LLM (claude CLI / Anthropic API) to classify entries
        /// instead of the offline keyword heuristic. Falls back to the
        /// heuristic on per-entry LLM failures.
        #[arg(long)]
        llm: bool,
        /// Backend for `--llm`: `cli` (default), `api`, or `auto`.
        #[arg(long, default_value = "cli")]
        llm_backend: String,
        /// Model name passed to the backend. Defaults to Haiku — cheap and
        /// fast for bulk classification.
        #[arg(long, default_value = "claude-haiku-4-5")]
        llm_model: String,
    },
}

use anyhow::Result;
use thoth_core::{Fact, Lesson, MemoryKind, MemoryMeta};
use thoth_memory::MemoryManager;
use thoth_store::StoreRoot;
use thoth_store::markdown::MarkdownStore;

use crate::{SynthKind, build_synth};

pub async fn run_show(root: &Path) -> Result<()> {
    // `memory show` is pure-filesystem (no redb) so strictly it doesn't
    // need the daemon to avoid lock conflicts. We still prefer the daemon
    // when available because the MCP server is the single writer — reading
    // through it guarantees we see the same view Claude Code sees.
    if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d.call("thoth_memory_show", serde_json::json!({})).await?;
        if crate::daemon::tool_is_error(&result) {
            anyhow::bail!("{}", crate::daemon::tool_text(&result));
        }
        println!("{}", crate::daemon::tool_text(&result));
        return Ok(());
    }

    // No daemon — read the files directly. We deliberately do NOT call
    // `StoreRoot::open` here: that would acquire the redb lock just to
    // read two markdown files, and collide with a daemon that raced us.
    for name in ["MEMORY.md", "LESSONS.md"] {
        let p = root.join(name);
        println!("─── {name} ───");
        match tokio::fs::read_to_string(&p).await {
            Ok(s) => println!("{s}"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                println!("(not found)");
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

pub async fn run_edit(root: &Path) -> Result<()> {
    // `memory edit` only touches MEMORY.md on disk — no redb access needed.
    // We intentionally skip `StoreRoot::open` so it can run even when the
    // MCP daemon owns the database lock.
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    // Ensure the root exists; otherwise the editor would create the file
    // in a non-existent parent and fail confusingly.
    if !root.exists() {
        anyhow::bail!("{} not found — run `thoth init` first", root.display());
    }
    let path = root.join("MEMORY.md");
    if !path.exists() {
        tokio::fs::write(&path, "# MEMORY.md\n").await?;
    }
    let status = tokio::process::Command::new(&editor)
        .arg(&path)
        .status()
        .await?;
    if !status.success() {
        anyhow::bail!("{editor} exited with {status}");
    }
    Ok(())
}

pub async fn run_fact(root: &Path, text: String, tags: Option<String>) -> Result<()> {
    let text = text.trim().to_string();
    if text.is_empty() {
        anyhow::bail!("fact text must not be empty");
    }
    let tags = tags
        .map(|s| {
            s.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d
            .call(
                "thoth_remember_fact",
                serde_json::json!({ "text": text, "tags": tags }),
            )
            .await?;
        if crate::daemon::tool_is_error(&result) {
            anyhow::bail!("{}", crate::daemon::tool_text(&result));
        }
        println!("{}", crate::daemon::tool_text(&result));
        return Ok(());
    }

    let store = StoreRoot::open(root).await?;
    let fact = Fact {
        meta: MemoryMeta::new(MemoryKind::Semantic),
        text,
        tags,
        scope: Default::default(),
    };
    store.markdown.append_fact(&fact).await?;
    println!(
        "fact appended to {}",
        store.path.join("MEMORY.md").display()
    );
    Ok(())
}

pub async fn run_lesson(root: &Path, when: String, advice: String) -> Result<()> {
    let when = when.trim().to_string();
    let advice = advice.trim().to_string();
    if when.is_empty() || advice.is_empty() {
        anyhow::bail!("both --when and advice text must be non-empty");
    }

    if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d
            .call(
                "thoth_remember_lesson",
                serde_json::json!({ "trigger": when, "advice": advice }),
            )
            .await?;
        if crate::daemon::tool_is_error(&result) {
            anyhow::bail!("{}", crate::daemon::tool_text(&result));
        }
        println!("{}", crate::daemon::tool_text(&result));
        return Ok(());
    }

    let store = StoreRoot::open(root).await?;
    let lesson = Lesson {
        meta: MemoryMeta::new(MemoryKind::Reflective),
        trigger: when,
        advice,
        success_count: 0,
        failure_count: 0,
        enforcement: Default::default(),
        suggested_enforcement: None,
        block_message: None,
    };
    store.markdown.append_lesson(&lesson).await?;
    println!(
        "lesson appended to {}",
        store.path.join("LESSONS.md").display()
    );
    Ok(())
}

pub async fn run_forget(root: &Path, json: bool) -> Result<()> {
    if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d.call("thoth_memory_forget", serde_json::json!({})).await?;
        if crate::daemon::tool_is_error(&result) {
            anyhow::bail!("{}", crate::daemon::tool_text(&result));
        }
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&crate::daemon::tool_data(&result))?
            );
        } else {
            println!("{}", crate::daemon::tool_text(&result));
        }
        return Ok(());
    }

    let mm = MemoryManager::open(root).await?;
    let report = mm.forget_pass().await?;
    if json {
        let v = serde_json::json!({
            "episodes_ttl": report.episodes_ttl,
            "episodes_cap": report.episodes_cap,
            "lessons_dropped": report.lessons_dropped,
            "lessons_quarantined": report.lessons_quarantined,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "forget pass: episodes_ttl={} episodes_cap={} lessons_dropped={} lessons_quarantined={}",
            report.episodes_ttl,
            report.episodes_cap,
            report.lessons_dropped,
            report.lessons_quarantined
        );
    }
    Ok(())
}

pub async fn run_pending(root: &Path, json: bool) -> Result<()> {
    let md = MarkdownStore::open(root).await?;
    let facts = md.read_pending_facts().await?;
    let lessons = md.read_pending_lessons().await?;
    if json {
        let v = serde_json::json!({
            "facts": facts.iter().enumerate().map(|(i, f)| {
                serde_json::json!({ "index": i, "text": f.text, "tags": f.tags })
            }).collect::<Vec<_>>(),
            "lessons": lessons.iter().enumerate().map(|(i, l)| {
                serde_json::json!({ "index": i, "trigger": l.trigger, "advice": l.advice })
            }).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!("── pending facts ({}) ──", facts.len());
        for (i, f) in facts.iter().enumerate() {
            let first = f.text.lines().next().unwrap_or("").trim();
            println!("[{i}] {first}");
        }
        println!("\n── pending lessons ({}) ──", lessons.len());
        for (i, l) in lessons.iter().enumerate() {
            println!("[{i}] {}", l.trigger);
        }
        if facts.is_empty() && lessons.is_empty() {
            println!("(no pending entries)");
        }
    }
    Ok(())
}

pub async fn run_promote(root: &Path, kind: &str, index: usize, json: bool) -> Result<()> {
    let md = MarkdownStore::open(root).await?;
    let (title, ok) = match kind {
        "fact" => match md.promote_pending_fact(index).await? {
            Some(f) => (f.text.lines().next().unwrap_or("").trim().to_string(), true),
            None => (String::new(), false),
        },
        "lesson" => match md.promote_pending_lesson(index).await? {
            Some(l) => (l.trigger, true),
            None => (String::new(), false),
        },
        other => anyhow::bail!("unknown kind: {other} (expected `fact` or `lesson`)"),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "kind": kind,
                "index": index,
                "promoted": ok,
                "title": title,
            }))?
        );
    } else if ok {
        println!("promoted {kind} [{index}]: {title}");
    } else {
        println!("no pending {kind} at index {index}");
    }
    Ok(())
}

pub async fn run_reject(
    root: &Path,
    kind: &str,
    index: usize,
    reason: Option<&str>,
    json: bool,
) -> Result<()> {
    let md = MarkdownStore::open(root).await?;
    let (title, ok) = match kind {
        "fact" => match md.reject_pending_fact(index, reason).await? {
            Some(f) => (f.text.lines().next().unwrap_or("").trim().to_string(), true),
            None => (String::new(), false),
        },
        "lesson" => match md.reject_pending_lesson(index, reason).await? {
            Some(l) => (l.trigger, true),
            None => (String::new(), false),
        },
        other => anyhow::bail!("unknown kind: {other} (expected `fact` or `lesson`)"),
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "kind": kind,
                "index": index,
                "rejected": ok,
                "title": title,
                "reason": reason,
            }))?
        );
    } else if ok {
        println!("rejected {kind} [{index}]: {title}");
    } else {
        println!("no pending {kind} at index {index}");
    }
    Ok(())
}

pub async fn run_log(root: &Path, limit: Option<usize>, json: bool) -> Result<()> {
    let md = MarkdownStore::open(root).await?;
    let mut entries = md.read_history().await?;
    if let Some(n) = limit
        && entries.len() > n
    {
        let skip = entries.len() - n;
        entries.drain(..skip);
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for e in &entries {
            println!("{}  {:<14} {:<7} {}", e.at_rfc3339, e.op, e.kind, e.title);
        }
        if entries.is_empty() {
            println!("(no history yet)");
        }
    }
    Ok(())
}

/// `thoth curate` — maintenance pass over memory. Runs the forget pass (TTL +
/// capacity + quarantine), reports reflection debt, and checks for lesson
/// clusters. With `--quiet` stays silent unless something actionable turned up.
pub async fn run_curate(root: &std::path::Path, quiet: bool) -> anyhow::Result<()> {
    if !root.exists() {
        if !quiet {
            println!("(no .thoth/ at {} — nothing to curate)", root.display());
        }
        return Ok(());
    }

    let mut findings: Vec<String> = Vec::new();

    // Forget pass. Prefer the daemon so we don't collide with the
    // MCP server's exclusive redb lock; fall back to opening the
    // store directly.
    let forget_report = if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        match d.call("thoth_memory_forget", serde_json::json!({})).await {
            Ok(res) if !crate::daemon::tool_is_error(&res) => {
                // Suppress no-op passes — a stale daemon binary may still
                // send the legacy always-non-empty text, so we check the
                // structured `data` counters instead of the text.
                if crate::hooks::forget_has_drops(&crate::daemon::tool_data(&res)) {
                    Some(crate::daemon::tool_text(&res).to_string())
                } else {
                    None
                }
            }
            Ok(res) => {
                findings.push(format!("forget failed: {}", crate::daemon::tool_text(&res)));
                None
            }
            Err(e) => {
                findings.push(format!("forget failed: {e}"));
                None
            }
        }
    } else {
        // Direct store access — needed when no daemon is alive.
        // `forget_pass` returns a detailed report; only surface it
        // when something was actually dropped.
        match thoth_memory::MemoryManager::open(root).await {
            Ok(m) => match m.forget_pass().await {
                Ok(r) => {
                    // Include every counter — a quarantine that didn't
                    // touch any episode is still a finding worth
                    // surfacing. Suppress entirely when all four are
                    // zero to match the daemon path's no-op silence.
                    let total =
                        r.episodes_ttl + r.episodes_cap + r.lessons_dropped + r.lessons_quarantined;
                    if total > 0 {
                        Some(format!(
                            "forget pass: episodes_ttl={} episodes_cap={} lessons_dropped={} lessons_quarantined={}",
                            r.episodes_ttl,
                            r.episodes_cap,
                            r.lessons_dropped,
                            r.lessons_quarantined
                        ))
                    } else {
                        None
                    }
                }
                Err(e) => {
                    findings.push(format!("forget failed: {e}"));
                    None
                }
            },
            Err(e) => {
                findings.push(format!("open store failed: {e}"));
                None
            }
        }
    };
    if let Some(s) = forget_report {
        let trimmed = s.trim();
        if !trimmed.is_empty() {
            findings.push(trimmed.to_string());
        }
    }

    // Reflection debt. Sync compute keeps this cheap (~1 ms).
    let disc = thoth_memory::DisciplineConfig::load_or_default(root).await;
    let debt = thoth_memory::ReflectionDebt::compute(root).await;
    if debt.should_nudge(&disc) {
        findings.push(debt.render());
    }

    // Lesson-cluster detection. Five or more lessons that share enough
    // trigger tokens (Jaccard ≥ 0.4) probably want to collapse into a
    // single skill — we surface each cluster with a ready-to-paste
    // `thoth_skill_propose` suggestion so the user can act on it.
    // Best-effort: a read failure only degrades this pass; curate must
    // still report debt + forget findings.
    match thoth_store::markdown::MarkdownStore::open(root).await {
        Ok(md) => match md.read_lessons().await {
            Ok(lessons) => {
                let clusters = thoth_memory::detect_clusters(
                    &lessons,
                    thoth_memory::DEFAULT_CLUSTER_MIN_SIZE,
                    thoth_memory::DEFAULT_CLUSTER_JACCARD,
                );
                for c in clusters {
                    // Shortened sample (first 2 triggers) so curate output
                    // stays scannable on a ≥5-member cluster.
                    let sample: Vec<String> = c
                        .triggers
                        .iter()
                        .take(2)
                        .map(|t| format!("\"{t}\""))
                        .collect();
                    let slug_hint = c
                        .shared_tokens
                        .iter()
                        .take(3)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("-");
                    let slug_hint = if slug_hint.is_empty() {
                        "lesson-cluster".to_string()
                    } else {
                        slug_hint
                    };
                    findings.push(format!(
                        "lesson cluster: {} lessons share triggers (e.g. {}). \
                         Consider `thoth_skill_propose {{slug: \"{slug_hint}\", \
                         source_triggers: [{}, …]}}`.",
                        c.triggers.len(),
                        sample.join(", "),
                        sample.join(", ")
                    ));
                }
            }
            Err(e) => findings.push(format!("lesson-cluster read failed: {e}")),
        },
        Err(e) => findings.push(format!("lesson-cluster open failed: {e}")),
    }

    if findings.is_empty() {
        if !quiet {
            println!(
                "curate: nothing to flag (debt={}, no TTL work)",
                debt.debt()
            );
        }
        return Ok(());
    }

    println!("### Curator findings");
    for f in &findings {
        println!("- {f}");
    }
    Ok(())
}

pub async fn run_nudge(
    root: &Path,
    window: usize,
    json: bool,
    synth_kind: Option<SynthKind>,
) -> Result<()> {
    let Some(synth) = build_synth(synth_kind)? else {
        anyhow::bail!(
            "`thoth memory nudge` requires --synth <provider> (and the matching feature)"
        );
    };
    let mm = MemoryManager::open(root).await?;
    let report = mm.nudge(synth.as_ref(), window).await?;
    if json {
        let v = serde_json::json!({
            "facts_added": report.facts_added,
            "lessons_added": report.lessons_added,
            "skills_added": report.skills_added,
        });
        println!("{}", serde_json::to_string_pretty(&v)?);
    } else {
        println!(
            "nudge: facts_added={} lessons_added={} skills_added={}",
            report.facts_added, report.lessons_added, report.skills_added
        );
    }
    Ok(())
}
