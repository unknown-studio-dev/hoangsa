//! `thoth memory` subcommands — show, edit, fact, lesson.

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
}

use anyhow::Result;
use hoangsa_memory_core::{Fact, Lesson, MemoryKind, MemoryMeta};
use hoangsa_memory_store::StoreRoot;

pub async fn run_show(root: &Path) -> Result<()> {
    // `memory show` is pure-filesystem (no redb) so strictly it doesn't
    // need the daemon to avoid lock conflicts. We still prefer the daemon
    // when available because the MCP server is the single writer — reading
    // through it guarantees we see the same view Claude Code sees.
    if let Some(mut d) = crate::daemon::DaemonClient::try_connect(root).await {
        let result = d.call("memory_show", serde_json::json!({})).await?;
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
                "memory_remember_fact",
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
                "memory_remember_lesson",
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
