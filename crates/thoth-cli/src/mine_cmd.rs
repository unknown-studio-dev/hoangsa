//! `thoth mine` — ingest Claude Code conversation JSONL files into
//! Thoth's episodic memory store. Extracts user/assistant turns and
//! indexes them for recall.

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use serde_json::Value;

pub async fn run_mine(root: &Path, source: &Path, json_output: bool) -> anyhow::Result<()> {
    let paths = discover_jsonl(source)?;
    if paths.is_empty() {
        bail!("no .jsonl files found under {}", source.display());
    }

    let store = thoth_store::StoreRoot::open(root).await?;
    let mut total_turns = 0u64;
    let mut total_sessions = 0u64;

    for path in &paths {
        match ingest_session(root, &store, path).await {
            Ok(count) => {
                total_sessions += 1;
                total_turns += count;
                if !json_output {
                    eprintln!(
                        "  {} — {count} turns",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "  SKIP {} — {e}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
            }
        }
    }

    if json_output {
        println!(
            "{}",
            serde_json::json!({
                "sessions": total_sessions,
                "turns": total_turns,
            })
        );
    } else {
        println!("Ingested {total_turns} turns from {total_sessions} sessions.");
    }
    Ok(())
}

fn discover_jsonl(source: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if source.is_file() && source.extension().is_some_and(|e| e == "jsonl") {
        return Ok(vec![source.to_path_buf()]);
    }
    if !source.is_dir() {
        bail!("{} is not a file or directory", source.display());
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(source).context("read dir")? {
        let entry = entry?;
        let p = entry.path();
        if p.extension().is_some_and(|e| e == "jsonl") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}

async fn ingest_session(
    _root: &Path,
    store: &thoth_store::StoreRoot,
    path: &Path,
) -> anyhow::Result<u64> {
    let session_id = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let content = tokio::fs::read_to_string(path).await?;
    let mut count = 0u64;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let obj: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let entry_type = obj.get("type").and_then(Value::as_str).unwrap_or("");

        match entry_type {
            "user" => {
                if obj.get("isMeta").and_then(Value::as_bool).unwrap_or(false) {
                    continue;
                }
                let text = extract_message_text(&obj);
                if text.is_empty() || text.len() < 5 {
                    continue;
                }
                // Skip slash commands
                if text.starts_with('/') || text.starts_with("<command") {
                    continue;
                }
                store
                    .episodes
                    .append_turn(session_id.clone(), "user".to_string(), text)
                    .await?;
                count += 1;
            }
            "assistant" => {
                let text = extract_message_text(&obj);
                if text.is_empty() || text.len() < 10 {
                    continue;
                }
                // Truncate very long assistant responses
                let text = if text.len() > 2000 {
                    format!("{}…", &text[..2000])
                } else {
                    text
                };
                store
                    .episodes
                    .append_turn(session_id.clone(), "assistant".to_string(), text)
                    .await?;
                count += 1;
            }
            _ => {}
        }
    }
    Ok(count)
}

fn extract_message_text(obj: &Value) -> String {
    let msg = match obj.get("message") {
        Some(m) => m,
        None => return String::new(),
    };
    let content = match msg.get("content") {
        Some(c) => c,
        None => return String::new(),
    };
    match content {
        Value::String(s) => s.trim().to_string(),
        Value::Array(arr) => {
            let mut parts = Vec::new();
            for item in arr {
                if let Some(text) = item.get("text").and_then(Value::as_str) {
                    parts.push(text.trim());
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}
