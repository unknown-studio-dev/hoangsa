//! Archive session tracker, backed by SQLite.
//!
//! Tracks which conversation sessions have been ingested into the ChromaDB
//! `thoth_archive` collection. Verbatim content lives in ChromaDB; this DB
//! only stores lightweight metadata to avoid re-processing and to support
//! spatial queries (project / topic).
//!
//! ```sql
//! CREATE TABLE archive_sessions (
//!     session_id  TEXT PRIMARY KEY,
//!     project     TEXT NOT NULL DEFAULT '',
//!     topic       TEXT NOT NULL DEFAULT '',
//!     ingested_at INTEGER NOT NULL,
//!     turn_count  INTEGER NOT NULL DEFAULT 0,
//!     curated     INTEGER NOT NULL DEFAULT 0
//! );
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, params};
use thoth_core::{Error, Result};

fn store(e: impl std::fmt::Display) -> Error {
    Error::Store(format!("archive: {e}"))
}

/// Summary of an ingested session.
#[derive(Debug, Clone)]
pub struct ArchiveSession {
    /// Unique session identifier.
    pub session_id: String,
    /// Project name (git remote or directory name).
    pub project: String,
    /// User-assigned or auto-detected topic.
    pub topic: String,
    /// Unix timestamp (seconds) when the session was ingested.
    pub ingested_at: i64,
    /// Number of conversation turns ingested.
    pub turn_count: i64,
    /// Whether facts/lessons have been extracted from this session.
    pub curated: bool,
}

/// Topic with aggregated counts.
#[derive(Debug, Clone)]
pub struct TopicSummary {
    /// Topic name.
    pub topic: String,
    /// Number of sessions with this topic.
    pub session_count: i64,
    /// Total turns across all sessions with this topic.
    pub total_turns: i64,
}

/// Handle to the archive session tracker.
///
/// Cheap to clone; the [`Connection`] is shared behind an [`Arc<Mutex<_>>`].
#[derive(Clone)]
pub struct ArchiveTracker {
    conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl ArchiveTracker {
    /// Open (or create) the tracker at `path`.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let path2 = path.clone();

        let conn = tokio::task::spawn_blocking(move || -> Result<Connection> {
            let c = Connection::open(&path2).map_err(store)?;
            c.execute_batch(
                "PRAGMA journal_mode = WAL;
                 PRAGMA synchronous = NORMAL;
                 CREATE TABLE IF NOT EXISTS archive_sessions (
                     session_id  TEXT PRIMARY KEY,
                     project     TEXT NOT NULL DEFAULT '',
                     topic       TEXT NOT NULL DEFAULT '',
                     ingested_at INTEGER NOT NULL,
                     turn_count  INTEGER NOT NULL DEFAULT 0,
                     curated     INTEGER NOT NULL DEFAULT 0
                 );",
            )
            .map_err(store)?;
            Ok(c)
        })
        .await
        .map_err(|e| Error::Store(format!("archive spawn: {e}")))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    /// Record a session as ingested.
    pub fn upsert_session(
        &self,
        session_id: &str,
        project: &str,
        topic: &str,
        turn_count: i64,
    ) -> Result<()> {
        let conn = self.conn.lock();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        conn.execute(
            "INSERT INTO archive_sessions (session_id, project, topic, ingested_at, turn_count)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(session_id) DO UPDATE SET
                 project = excluded.project,
                 topic = excluded.topic,
                 turn_count = excluded.turn_count",
            params![session_id, project, topic, now, turn_count],
        )
        .map_err(store)?;
        Ok(())
    }

    /// Check whether a session has already been ingested.
    pub fn is_ingested(&self, session_id: &str) -> Result<bool> {
        let conn = self.conn.lock();
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM archive_sessions WHERE session_id = ?1)",
                params![session_id],
                |row| row.get(0),
            )
            .map_err(store)?;
        Ok(exists)
    }

    /// Mark a session as curated (facts/lessons extracted).
    pub fn mark_curated(&self, session_id: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "UPDATE archive_sessions SET curated = 1 WHERE session_id = ?1",
            params![session_id],
        )
        .map_err(store)?;
        Ok(())
    }

    /// Get all uncurated sessions.
    pub fn uncurated_sessions(&self) -> Result<Vec<ArchiveSession>> {
        let conn = self.conn.lock();
        let mut stmt = conn
            .prepare(
                "SELECT session_id, project, topic, ingested_at, turn_count, curated
                 FROM archive_sessions WHERE curated = 0
                 ORDER BY ingested_at DESC",
            )
            .map_err(store)?;
        let rows = stmt
            .query_map([], |row| {
                Ok(ArchiveSession {
                    session_id: row.get(0)?,
                    project: row.get(1)?,
                    topic: row.get(2)?,
                    ingested_at: row.get(3)?,
                    turn_count: row.get(4)?,
                    curated: row.get::<_, i64>(5)? != 0,
                })
            })
            .map_err(store)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(store)
    }

    /// List topics with session and turn counts, optionally filtered by project.
    pub fn topics(&self, project: Option<&str>) -> Result<Vec<TopicSummary>> {
        let conn = self.conn.lock();
        let mut out = Vec::new();
        match project {
            Some(p) => {
                let mut stmt = conn
                    .prepare(
                        "SELECT topic, COUNT(*) as cnt, SUM(turn_count) as turns
                         FROM archive_sessions WHERE project = ?1
                         GROUP BY topic ORDER BY turns DESC",
                    )
                    .map_err(store)?;
                let rows = stmt
                    .query_map(params![p], |row| {
                        Ok(TopicSummary {
                            topic: row.get(0)?,
                            session_count: row.get(1)?,
                            total_turns: row.get(2)?,
                        })
                    })
                    .map_err(store)?;
                for r in rows {
                    out.push(r.map_err(store)?);
                }
            }
            None => {
                let mut stmt = conn
                    .prepare(
                        "SELECT topic, COUNT(*) as cnt, SUM(turn_count) as turns
                         FROM archive_sessions
                         GROUP BY topic ORDER BY turns DESC",
                    )
                    .map_err(store)?;
                let rows = stmt
                    .query_map([], |row| {
                        Ok(TopicSummary {
                            topic: row.get(0)?,
                            session_count: row.get(1)?,
                            total_turns: row.get(2)?,
                        })
                    })
                    .map_err(store)?;
                for r in rows {
                    out.push(r.map_err(store)?);
                }
            }
        }
        Ok(out)
    }

    /// Overall status: total sessions, total turns, curated count.
    pub fn status(&self) -> Result<(i64, i64, i64)> {
        let conn = self.conn.lock();
        let (sessions, turns, curated) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(turn_count),0), COALESCE(SUM(curated),0)
                 FROM archive_sessions",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .map_err(store)?;
        Ok((sessions, turns, curated))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("archive.db");
        let tracker = ArchiveTracker::open(&db).await.unwrap();

        tracker
            .upsert_session("s1", "thoth", "memory-arch", 42)
            .unwrap();
        assert!(tracker.is_ingested("s1").unwrap());
        assert!(!tracker.is_ingested("s2").unwrap());

        let (sessions, turns, curated) = tracker.status().unwrap();
        assert_eq!(sessions, 1);
        assert_eq!(turns, 42);
        assert_eq!(curated, 0);

        tracker.mark_curated("s1").unwrap();
        let (_, _, curated) = tracker.status().unwrap();
        assert_eq!(curated, 1);

        let topics = tracker.topics(Some("thoth")).unwrap();
        assert_eq!(topics.len(), 1);
        assert_eq!(topics[0].topic, "memory-arch");
    }
}
