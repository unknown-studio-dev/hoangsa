//! Conversation-turn and archive tools.

use serde::Deserialize;
use serde_json::{Value, json};
use hoangsa_memory_store::StoreRoot;

use crate::proto::ToolOutput;

use super::Server;

impl Server {
    // ---- conversation turn tools ------------------------------------------

    pub(super) async fn tool_turn_save(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            session_id: String,
            role: String,
            content: String,
            /// Optional git HEAD sha captured at the moment of this turn.
            /// Lets future `memory_archive_search` answer "what was the
            /// code like when we decided X" without grepping `git log`.
            #[serde(default)]
            commit_sha: Option<String>,
            /// Optional list of file paths the caller associates with
            /// this turn (usually the files touched since the last turn).
            #[serde(default)]
            file_paths: Vec<String>,
        }
        let Args {
            session_id,
            role,
            content,
            commit_sha,
            file_paths,
        } = serde_json::from_value(args)?;

        let id = self
            .resources()
            .await?
            .store
            .episodes
            .append_turn(
                session_id.clone(),
                role.clone(),
                content,
                commit_sha.clone(),
                file_paths.clone(),
            )
            .await?;
        let commit_fragment = commit_sha
            .as_ref()
            .map(|s| format!(" @ {}", &s[..s.len().min(7)]))
            .unwrap_or_default();
        let files_fragment = if file_paths.is_empty() {
            String::new()
        } else {
            format!(" ({} file(s))", file_paths.len())
        };
        let text = format!(
            "saved turn #{id} ({role}) for session {session_id}{commit_fragment}{files_fragment}"
        );
        Ok(ToolOutput::new(
            json!({
                "id": id,
                "role": role,
                "commit_sha": commit_sha,
                "file_paths": file_paths,
            }),
            text,
        ))
    }

    pub(super) async fn tool_turns_search(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            query: String,
            #[serde(default)]
            top_k: Option<usize>,
        }
        let Args { query, top_k } = serde_json::from_value(args)?;
        let k = top_k.unwrap_or(10);

        let hits = self
            .resources()
            .await?
            .store
            .episodes
            .search_turns(&query, k)
            .await?;
        if hits.is_empty() {
            return Ok(ToolOutput::new(json!({"count": 0}), "no matching turns"));
        }

        let mut text = String::new();
        for t in &hits {
            let ts =
                t.at.format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
            // `commit_sha` / `file_paths` stay in `data` unconditionally;
            // in text we only surface them when present so unenriched
            // legacy turns don't get a trailing "@ _ (0 file(s))" tag.
            let commit_tag = t
                .commit_sha
                .as_ref()
                .map(|s| format!(" @ {}", &s[..s.len().min(7)]))
                .unwrap_or_default();
            let paths_tag = if t.file_paths.is_empty() {
                String::new()
            } else {
                format!(" files={}", t.file_paths.join(","))
            };
            text.push_str(&format!(
                "[{}] {} (turn {}, session {}){}{}\n{}\n---\n",
                ts,
                t.role,
                t.turn_number,
                &t.session_id[..t.session_id.len().min(8)],
                commit_tag,
                paths_tag,
                &t.content[..t.content.len().min(500)],
            ));
        }
        let data: Vec<Value> = hits
            .iter()
            .map(|t| {
                json!({
                    "id": t.id,
                    "session_id": t.session_id,
                    "turn_number": t.turn_number,
                    "role": t.role,
                    "commit_sha": t.commit_sha,
                    "file_paths": t.file_paths,
                })
            })
            .collect();
        Ok(ToolOutput::new(
            json!({"count": hits.len(), "turns": data}),
            text,
        ))
    }

    // ---- archive tools ---------------------------------------------------

    pub(super) async fn tool_archive_status(&self) -> anyhow::Result<ToolOutput> {
        let db_path = StoreRoot::archive_path(&self.inner.root);
        let tracker = hoangsa_memory_store::ArchiveTracker::open(&db_path).await?;
        let (sessions, turns, curated) = tracker.status()?;
        let data = json!({
            "sessions": sessions,
            "turns": turns,
            "curated": curated,
        });
        let text = format!("Archive: {sessions} sessions, {turns} turns ({curated} curated)");
        Ok(ToolOutput::new(data, text))
    }

    pub(super) async fn tool_archive_topics(&self, args: Value) -> anyhow::Result<ToolOutput> {
        let project = args.get("project").and_then(|v| v.as_str());
        let db_path = StoreRoot::archive_path(&self.inner.root);
        let tracker = hoangsa_memory_store::ArchiveTracker::open(&db_path).await?;
        let topics = tracker.topics(project)?;
        let arr: Vec<Value> = topics
            .iter()
            .map(|t| {
                json!({
                    "topic": t.topic,
                    "sessions": t.session_count,
                    "turns": t.total_turns,
                })
            })
            .collect();
        let text = if topics.is_empty() {
            "No topics found.".to_string()
        } else {
            topics
                .iter()
                .map(|t| {
                    format!(
                        "{}: {} sessions, {} turns",
                        t.topic, t.session_count, t.total_turns
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        Ok(ToolOutput::new(json!(arr), text))
    }

    pub(super) async fn tool_archive_search(&self, args: Value) -> anyhow::Result<ToolOutput> {
        let query = args
            .get("query")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        let top_k = args.get("top_k").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let project = args.get("project").and_then(|v| v.as_str());
        let topic = args.get("topic").and_then(|v| v.as_str());

        let col = self.open_archive_vector().await?;

        let mut filter = None;
        if project.is_some() || topic.is_some() {
            let mut conditions = Vec::new();
            if let Some(p) = project {
                conditions.push(json!({"project": {"$eq": p}}));
            }
            if let Some(t) = topic {
                conditions.push(json!({"topic": {"$eq": t}}));
            }
            filter = Some(if conditions.len() == 1 {
                conditions.into_iter().next().unwrap()
            } else {
                json!({"$and": conditions})
            });
        }

        let hits = col.query_text(query, top_k, filter).await?;
        let arr: Vec<Value> = hits
            .iter()
            .map(|h| {
                json!({
                    "id": h.id,
                    "distance": h.distance,
                    "text": h.document,
                    "metadata": h.metadata,
                })
            })
            .collect();
        let text = if hits.is_empty() {
            "No archive results.".to_string()
        } else {
            hits.iter()
                .map(|h| {
                    let topic = h
                        .metadata
                        .as_ref()
                        .and_then(|m| m.get("topic"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("?");
                    let preview = h
                        .document
                        .as_deref()
                        .unwrap_or("")
                        .chars()
                        .take(200)
                        .collect::<String>();
                    format!("[{topic}] (d={:.3}) {preview}", h.distance)
                })
                .collect::<Vec<_>>()
                .join("\n---\n")
        };
        Ok(ToolOutput::new(json!(arr), text))
    }

    /// Run an archive ingest inside the daemon process so the existing
    /// vector store handle (embedder + SQLite connection) is reused.
    /// This is the memory-pressure fix: PreCompact / SessionEnd hooks
    /// used to spawn a detached CLI which booted a fresh ~500 MB Python
    /// sidecar per invocation. Concurrent Claude Code sessions would
    /// pile those up and OOM the machine. Forwarding to this tool via
    /// the daemon socket keeps the embedder
    /// count at one.
    pub(super) async fn tool_archive_ingest(&self, args: Value) -> anyhow::Result<ToolOutput> {
        #[derive(Deserialize)]
        struct Args {
            #[serde(default)]
            project: Option<String>,
            #[serde(default)]
            topic: Option<String>,
            #[serde(default)]
            refresh: bool,
            #[serde(default)]
            limit: Option<usize>,
        }
        let Args {
            project,
            topic,
            refresh,
            limit,
        } = serde_json::from_value(args)?;

        let tracker_path = StoreRoot::archive_path(&self.inner.root);
        let tracker = hoangsa_memory_store::ArchiveTracker::open(&tracker_path).await?;

        // Daemon-side ingest requires the already-running vector
        // store. If it's not enabled we bail via ToolOutput::error so
        // the caller can fall back to spawning the CLI.
        let col = match self.get_vector_store().await {
            Some(_) => self.open_archive_vector().await?,
            None => return Ok(ToolOutput::error("vector store not enabled")),
        };

        let opts = hoangsa_memory_retrieve::archive::IngestOpts {
            project_filter: project,
            topic_override: topic,
            refresh,
            limit,
        };
        let stats =
            hoangsa_memory_retrieve::archive::run_ingest(&tracker, col.as_ref(), opts).await?;

        let text = format!(
            "Ingested {} sessions ({} chunks), skipped {} already-ingested. Retention trimmed {} session(s), cleaned {} from vector store.",
            stats.total_sessions,
            stats.total_chunks,
            stats.skipped,
            stats.retention_trimmed,
            stats.retention_vector_cleaned,
        );
        let data = json!({
            "total_sessions": stats.total_sessions,
            "total_chunks": stats.total_chunks,
            "skipped": stats.skipped,
            "retention_trimmed": stats.retention_trimmed,
            "retention_vector_cleaned": stats.retention_vector_cleaned,
        });
        Ok(ToolOutput::new(data, text))
    }
}
