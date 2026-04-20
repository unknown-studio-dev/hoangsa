//! Append-only episodic log, backed by SQLite + FTS5.
//!
//! Every [`Event`] observed by Thoth is serialized to JSON and appended here,
//! alongside a contentless FTS5 index so lessons and reflective memories can
//! later grep the log for relevant past experiences.
//!
//! Schema:
//!
//! ```sql
//! CREATE TABLE episodes(
//!     id               INTEGER PRIMARY KEY,
//!     event_id         TEXT NOT NULL,   -- uuid from Event when present
//!     kind             TEXT NOT NULL,   -- "file_changed", "query_issued", ...
//!     at_unix_ns       INTEGER NOT NULL,
//!     payload          TEXT NOT NULL,   -- serde_json of Event
//!     salience         REAL NOT NULL DEFAULT 1.0,
//!     access_count     INTEGER NOT NULL DEFAULT 0,
//!     last_accessed_ns INTEGER             -- NULL until first bump
//! );
//! CREATE VIRTUAL TABLE episodes_fts USING fts5(
//!     kind, payload,
//!     content='episodes', content_rowid='id',
//!     tokenize='porter unicode61'
//! );
//! ```
//!
//! Triggers keep `episodes_fts` in sync. `salience`, `access_count`, and
//! `last_accessed_ns` are populated by the retriever + memory manager to
//! support DESIGN §9's decay-based retention formula:
//!
//! ```text
//! effective = salience · exp(-λ·days_idle) · ln(e + access_count)
//! ```
//!
//! The columns are added by a one-shot migration in [`EpisodeLog::open`],
//! so existing stores upgrade in place.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use rusqlite::{Connection, params};
use thoth_core::{Error, Event, EventId, Result};
use time::OffsetDateTime;

/// A single row returned from an episode search.
#[derive(Debug, Clone)]
pub struct EpisodeHit {
    /// Row id.
    pub id: i64,
    /// Event kind tag.
    pub kind: String,
    /// Timestamp.
    pub at: OffsetDateTime,
    /// Deserialized event.
    pub event: Event,
}

/// A knowledge graph triple with temporal validity.
#[derive(Debug, Clone, serde::Serialize)]
#[allow(dead_code, missing_docs)]
pub struct KgTriple {
    pub id: i64,
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub valid_from: Option<String>,
    pub valid_to: Option<String>,
    pub confidence: f64,
    pub source: Option<String>,
}

/// A verbatim conversation turn stored in `episodes.db`.
#[derive(Debug, Clone)]
pub struct Turn {
    /// Row id.
    pub id: i64,
    /// Session identifier (opaque string from Claude Code).
    pub session_id: String,
    /// Position within the session (1-based).
    pub turn_number: i64,
    /// "user" or "assistant".
    pub role: String,
    /// Verbatim content.
    pub content: String,
    /// Timestamp.
    pub at: OffsetDateTime,
}

/// Handle to the SQLite-backed episodic log.
///
/// Cheap to clone; the [`Connection`] is shared behind an [`Arc<Mutex<_>>`].
#[derive(Clone)]
pub struct EpisodeLog {
    conn: Arc<Mutex<Connection>>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl EpisodeLog {
    /// Open (or create) the log at `path` (a `.sqlite` file).
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let path2 = path.clone();

        let conn = tokio::task::spawn_blocking(move || -> Result<Connection> {
            let c = Connection::open(&path2).map_err(store)?;
            // Pragmas for a write-heavy append log.
            c.pragma_update(None, "journal_mode", "WAL")
                .map_err(store)?;
            c.pragma_update(None, "synchronous", "NORMAL")
                .map_err(store)?;
            c.pragma_update(None, "foreign_keys", "ON").map_err(store)?;

            c.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS episodes (
                    id               INTEGER PRIMARY KEY,
                    event_id         TEXT NOT NULL,
                    kind             TEXT NOT NULL,
                    at_unix_ns       INTEGER NOT NULL,
                    payload          TEXT NOT NULL,
                    salience         REAL NOT NULL DEFAULT 1.0,
                    access_count     INTEGER NOT NULL DEFAULT 0,
                    last_accessed_ns INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_episodes_at ON episodes(at_unix_ns);
                CREATE INDEX IF NOT EXISTS idx_episodes_kind ON episodes(kind);

                CREATE VIRTUAL TABLE IF NOT EXISTS episodes_fts USING fts5(
                    kind, payload,
                    content='episodes',
                    content_rowid='id',
                    tokenize='porter unicode61'
                );

                CREATE TRIGGER IF NOT EXISTS episodes_ai AFTER INSERT ON episodes BEGIN
                    INSERT INTO episodes_fts(rowid, kind, payload)
                    VALUES (new.id, new.kind, new.payload);
                END;
                CREATE TRIGGER IF NOT EXISTS episodes_ad AFTER DELETE ON episodes BEGIN
                    INSERT INTO episodes_fts(episodes_fts, rowid, kind, payload)
                    VALUES ('delete', old.id, old.kind, old.payload);
                END;
                "#,
            )
            .map_err(store)?;

            // Forward-migrate any store that was created before the decay
            // columns existed. `ALTER TABLE ... ADD COLUMN` errors out if
            // the column is already there, so gate on `PRAGMA table_info`.
            ensure_decay_columns(&c)?;

            // Knowledge graph — temporal entity-relationship triples.
            c.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS kg_triples (
                    id           INTEGER PRIMARY KEY,
                    subject      TEXT NOT NULL,
                    predicate    TEXT NOT NULL,
                    object       TEXT NOT NULL,
                    valid_from   TEXT,
                    valid_to     TEXT,
                    confidence   REAL NOT NULL DEFAULT 1.0,
                    source       TEXT,
                    created_at   INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_kg_subject
                    ON kg_triples(subject);
                CREATE INDEX IF NOT EXISTS idx_kg_object
                    ON kg_triples(object);
                CREATE INDEX IF NOT EXISTS idx_kg_predicate
                    ON kg_triples(predicate);
                "#,
            )
            .map_err(store)?;

            // Conversation turns — verbatim archival of user/assistant exchanges.
            c.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS turns (
                    id           INTEGER PRIMARY KEY,
                    session_id   TEXT NOT NULL,
                    turn_number  INTEGER NOT NULL,
                    role         TEXT NOT NULL,
                    content      TEXT NOT NULL,
                    at_unix_ns   INTEGER NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_turns_session
                    ON turns(session_id, turn_number);

                CREATE VIRTUAL TABLE IF NOT EXISTS turns_fts USING fts5(
                    content,
                    content='turns',
                    content_rowid='id',
                    tokenize='porter unicode61'
                );
                CREATE TRIGGER IF NOT EXISTS turns_ai AFTER INSERT ON turns BEGIN
                    INSERT INTO turns_fts(rowid, content)
                    VALUES (new.id, new.content);
                END;
                CREATE TRIGGER IF NOT EXISTS turns_ad AFTER DELETE ON turns BEGIN
                    INSERT INTO turns_fts(turns_fts, rowid, content)
                    VALUES ('delete', old.id, old.content);
                END;
                "#,
            )
            .map_err(store)?;

            // Schema version tracking table.
            c.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS _thoth_meta (
                    key   TEXT PRIMARY KEY,
                    value TEXT NOT NULL
                );
                INSERT OR IGNORE INTO _thoth_meta(key, value) VALUES ('schema_version', '1');
                "#,
            )
            .map_err(store)?;

            check_schema_version(&c)?;

            Ok(c)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            path,
        })
    }

    /// Convenience: append a `QueryIssued` event for `text` with a fresh
    /// id and the current UTC timestamp. Exists so callers that only want
    /// to satisfy `thoth-gate`'s recall window (e.g. `thoth query` on the
    /// direct-store path) don't need to pull `uuid` / `time` into their
    /// own Cargo manifest just to mint one event.
    pub async fn log_query_issued(&self, text: String) -> Result<i64> {
        let ev = Event::QueryIssued {
            id: EventId::new_v4(),
            text,
            at: OffsetDateTime::now_utc(),
        };
        self.append(&ev).await
    }

    /// Append an event. Returns the autoincrementing row id.
    pub async fn append(&self, ev: &Event) -> Result<i64> {
        let conn = self.conn.clone();
        let kind = event_kind_tag(ev).to_string();
        let event_id = event_id_of(ev).map(|u| u.to_string()).unwrap_or_default();
        let at_ns = event_at_ns(ev);
        let payload = serde_json::to_string(ev)?;

        tokio::task::spawn_blocking(move || -> Result<i64> {
            let c = conn.lock();
            c.execute(
                r#"INSERT INTO episodes(event_id, kind, at_unix_ns, payload)
                   VALUES (?1, ?2, ?3, ?4)"#,
                params![event_id, kind, at_ns, payload],
            )
            .map_err(store)?;
            Ok(c.last_insert_rowid())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Return the most recent `k` events, newest first.
    pub async fn recent(&self, k: usize) -> Result<Vec<EpisodeHit>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<EpisodeHit>> {
            let c = conn.lock();
            let mut stmt = c
                .prepare(
                    "SELECT id, kind, at_unix_ns, payload FROM episodes \
                     ORDER BY id DESC LIMIT ?1",
                )
                .map_err(store)?;
            let mut rows = stmt.query(params![k as i64]).map_err(store)?;

            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(store)? {
                out.push(row_to_hit(r)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// FTS5 search over `kind` + `payload`. Pass a raw FTS5 MATCH expression.
    pub async fn search(&self, match_expr: impl Into<String>, k: usize) -> Result<Vec<EpisodeHit>> {
        let conn = self.conn.clone();
        let m = match_expr.into();
        tokio::task::spawn_blocking(move || -> Result<Vec<EpisodeHit>> {
            let c = conn.lock();
            let mut stmt = c
                .prepare(
                    "SELECT e.id, e.kind, e.at_unix_ns, e.payload \
                     FROM episodes_fts f JOIN episodes e ON e.id = f.rowid \
                     WHERE episodes_fts MATCH ?1 \
                     ORDER BY rank LIMIT ?2",
                )
                .map_err(store)?;
            let mut rows = stmt.query(params![m, k as i64]).map_err(store)?;

            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(store)? {
                out.push(row_to_hit(r)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Total number of stored events.
    pub async fn count(&self) -> Result<i64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<i64> {
            let c = conn.lock();
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM episodes", [], |r| r.get(0))
                .map_err(store)?;
            Ok(n)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Delete every event older than `cutoff_unix_ns`. Returns the number of
    /// rows removed. The FTS mirror is cleaned up via the trigger.
    pub async fn delete_older_than(&self, cutoff_unix_ns: i64) -> Result<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<u64> {
            let c = conn.lock();
            let n = c
                .execute(
                    "DELETE FROM episodes WHERE at_unix_ns < ?1",
                    params![cutoff_unix_ns],
                )
                .map_err(store)?;
            Ok(n as u64)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Record that episode `row_id` was retrieved — bumps `access_count`
    /// by 1 and refreshes `last_accessed_ns` to `now_ns`.
    ///
    /// Silently returns `Ok(())` if `row_id` no longer exists; the retriever
    /// shouldn't bubble a 404 back to the caller just because a decay pass
    /// raced ahead of it.
    pub async fn bump_access_by_id(&self, row_id: i64, now_ns: i64) -> Result<()> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let c = conn.lock();
            c.execute(
                "UPDATE episodes \
                 SET access_count = access_count + 1, \
                     last_accessed_ns = ?2 \
                 WHERE id = ?1",
                params![row_id, now_ns],
            )
            .map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Stream out every episode's decay metadata, one per row. Used by the
    /// forget pass to compute `effective_retention_score` and drop rows
    /// that fall below the configured floor.
    ///
    /// Returned tuples are `(row_id, salience, access_count,
    /// last_accessed_ns_or_at_ns)` — if `last_accessed_ns` is NULL we fall
    /// back to the row's `at_unix_ns` so first-pass decay is calculated
    /// relative to when the episode was created, not to an impossible zero.
    pub async fn iter_with_decay_meta(&self) -> Result<Vec<(i64, f32, u64, i64)>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<(i64, f32, u64, i64)>> {
            let c = conn.lock();
            let mut stmt = c
                .prepare(
                    "SELECT id, salience, access_count, \
                            COALESCE(last_accessed_ns, at_unix_ns) \
                     FROM episodes",
                )
                .map_err(store)?;
            let mut rows = stmt.query([]).map_err(store)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(store)? {
                let id: i64 = r.get(0).map_err(store)?;
                let salience: f64 = r.get(1).map_err(store)?;
                let access_count: i64 = r.get(2).map_err(store)?;
                let last_ns: i64 = r.get(3).map_err(store)?;
                out.push((id, salience as f32, access_count.max(0) as u64, last_ns));
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Batch delete episodes by row id. Returns the number of rows removed.
    /// Used by the decay-based forget pass after it has computed which rows
    /// fall below the retention floor.
    pub async fn delete_by_ids(&self, ids: &[i64]) -> Result<u64> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.conn.clone();
        let ids: Vec<i64> = ids.to_vec();
        tokio::task::spawn_blocking(move || -> Result<u64> {
            let c = conn.lock();
            // Many-row DELETE using rarray-style parameter wouldn't work
            // without the bundled rarray feature, so chunk into fixed-size
            // batches and issue them with an explicit `IN (?, ?, ...)`.
            let mut total = 0u64;
            for chunk in ids.chunks(512) {
                let placeholders: Vec<&str> = chunk.iter().map(|_| "?").collect();
                let sql = format!(
                    "DELETE FROM episodes WHERE id IN ({})",
                    placeholders.join(",")
                );
                let params: Vec<&dyn rusqlite::ToSql> =
                    chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
                let n = c.execute(&sql, params.as_slice()).map_err(store)?;
                total += n as u64;
            }
            Ok(total)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Trim the log to at most `max` rows (newest kept). Returns the number
    /// of rows removed.
    pub async fn trim_to_capacity(&self, max: usize) -> Result<u64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<u64> {
            let c = conn.lock();
            let n = c
                .execute(
                    "DELETE FROM episodes \
                     WHERE id IN ( \
                         SELECT id FROM episodes \
                         ORDER BY id DESC \
                         LIMIT -1 OFFSET ?1 \
                     )",
                    params![max as i64],
                )
                .map_err(store)?;
            Ok(n as u64)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    // ---- knowledge graph ---------------------------------------------------

    /// Add a temporal triple. Returns the row id.
    #[allow(clippy::too_many_arguments)]
    pub async fn kg_add(
        &self,
        subject: String,
        predicate: String,
        object: String,
        valid_from: Option<String>,
        valid_to: Option<String>,
        confidence: f64,
        source: Option<String>,
    ) -> Result<i64> {
        let conn = self.conn.clone();
        let now_ns = OffsetDateTime::now_utc()
            .unix_timestamp_nanos()
            .min(i64::MAX as i128) as i64;
        tokio::task::spawn_blocking(move || -> Result<i64> {
            let c = conn.lock();
            c.execute(
                "INSERT INTO kg_triples(subject, predicate, object, valid_from, valid_to, confidence, source, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![subject, predicate, object, valid_from, valid_to, confidence, source, now_ns],
            )
            .map_err(store)?;
            Ok(c.last_insert_rowid())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Query all triples for an entity (as subject or object).
    /// If `as_of` is provided, filter to triples valid at that date.
    pub async fn kg_query(
        &self,
        entity: String,
        direction: String,
        as_of: Option<String>,
    ) -> Result<Vec<KgTriple>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<KgTriple>> {
            let c = conn.lock();
            let base = match direction.as_str() {
                "outgoing" => "SELECT id, subject, predicate, object, valid_from, valid_to, confidence, source FROM kg_triples WHERE subject = ?1",
                "incoming" => "SELECT id, subject, predicate, object, valid_from, valid_to, confidence, source FROM kg_triples WHERE object = ?1",
                _ => "SELECT id, subject, predicate, object, valid_from, valid_to, confidence, source FROM kg_triples WHERE subject = ?1 OR object = ?1",
            };
            let sql = if as_of.is_some() {
                format!(
                    "{base} AND (valid_from IS NULL OR valid_from <= ?2) AND (valid_to IS NULL OR valid_to >= ?2)"
                )
            } else {
                base.to_string()
            };
            let mut stmt = c.prepare(&sql).map_err(store)?;
            let mut rows = if let Some(ref date) = as_of {
                stmt.query(params![entity, date]).map_err(store)?
            } else {
                stmt.query(params![entity]).map_err(store)?
            };
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(store)? {
                out.push(row_to_triple(r)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Mark a triple as ended by setting valid_to.
    pub async fn kg_invalidate(
        &self,
        subject: String,
        predicate: String,
        object: String,
        ended: Option<String>,
    ) -> Result<u64> {
        let conn = self.conn.clone();
        let ended = ended.unwrap_or_else(|| {
            OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default()
        });
        tokio::task::spawn_blocking(move || -> Result<u64> {
            let c = conn.lock();
            let n = c
                .execute(
                    "UPDATE kg_triples SET valid_to = ?4 \
                     WHERE subject = ?1 AND predicate = ?2 AND object = ?3 AND valid_to IS NULL",
                    params![subject, predicate, object, ended],
                )
                .map_err(store)?;
            Ok(n as u64)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Chronological timeline of triples, optionally filtered by entity.
    pub async fn kg_timeline(&self, entity: Option<String>, limit: usize) -> Result<Vec<KgTriple>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<KgTriple>> {
            let c = conn.lock();
            let (sql, p): (String, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(ref ent) = entity {
                (
                    "SELECT id, subject, predicate, object, valid_from, valid_to, confidence, source \
                     FROM kg_triples WHERE subject = ?1 OR object = ?1 \
                     ORDER BY created_at DESC LIMIT ?2"
                        .to_string(),
                    vec![
                        Box::new(ent.clone()) as Box<dyn rusqlite::ToSql>,
                        Box::new(limit as i64),
                    ],
                )
            } else {
                (
                    "SELECT id, subject, predicate, object, valid_from, valid_to, confidence, source \
                     FROM kg_triples ORDER BY created_at DESC LIMIT ?1"
                        .to_string(),
                    vec![Box::new(limit as i64) as Box<dyn rusqlite::ToSql>],
                )
            };
            let mut stmt = c.prepare(&sql).map_err(store)?;
            let params_ref: Vec<&dyn rusqlite::ToSql> = p.iter().map(|b| b.as_ref()).collect();
            let mut rows = stmt.query(params_ref.as_slice()).map_err(store)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(store)? {
                out.push(row_to_triple(r)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// KG summary stats.
    pub async fn kg_stats(&self) -> Result<(i64, i64, i64)> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<(i64, i64, i64)> {
            let c = conn.lock();
            let total: i64 = c
                .query_row("SELECT COUNT(*) FROM kg_triples", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(store)?;
            let current: i64 = c
                .query_row(
                    "SELECT COUNT(*) FROM kg_triples WHERE valid_to IS NULL",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(store)?;
            let expired = total - current;
            Ok((total, current, expired))
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    // ---- conversation turns ------------------------------------------------

    /// Append a verbatim conversation turn. Returns the row id.
    pub async fn append_turn(
        &self,
        session_id: String,
        role: String,
        content: String,
    ) -> Result<i64> {
        let conn = self.conn.clone();
        let at_ns = OffsetDateTime::now_utc()
            .unix_timestamp_nanos()
            .min(i64::MAX as i128) as i64;

        tokio::task::spawn_blocking(move || -> Result<i64> {
            let c = conn.lock();
            let max_turn: i64 = c
                .query_row(
                    "SELECT COALESCE(MAX(turn_number), 0) FROM turns WHERE session_id = ?1",
                    params![session_id],
                    |r| r.get::<_, i64>(0),
                )
                .map_err(store)?;
            let turn_number = max_turn + 1;
            c.execute(
                "INSERT INTO turns(session_id, turn_number, role, content, at_unix_ns) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![session_id, turn_number, role, content, at_ns],
            )
            .map_err(store)?;
            Ok(c.last_insert_rowid())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Return the most recent `k` turns for a session, oldest first.
    pub async fn recent_turns(&self, session_id: String, k: usize) -> Result<Vec<Turn>> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<Turn>> {
            let c = conn.lock();
            let mut stmt = c
                .prepare(
                    "SELECT id, session_id, turn_number, role, content, at_unix_ns \
                     FROM turns WHERE session_id = ?1 \
                     ORDER BY turn_number DESC LIMIT ?2",
                )
                .map_err(store)?;
            let mut rows = stmt.query(params![session_id, k as i64]).map_err(store)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(store)? {
                out.push(row_to_turn(r)?);
            }
            out.reverse();
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// FTS5 search over turn content. Returns matches across all sessions.
    pub async fn search_turns(&self, match_expr: impl Into<String>, k: usize) -> Result<Vec<Turn>> {
        let conn = self.conn.clone();
        let m = match_expr.into();
        tokio::task::spawn_blocking(move || -> Result<Vec<Turn>> {
            let c = conn.lock();
            let mut stmt = c
                .prepare(
                    "SELECT t.id, t.session_id, t.turn_number, t.role, t.content, t.at_unix_ns \
                     FROM turns_fts f JOIN turns t ON t.id = f.rowid \
                     WHERE turns_fts MATCH ?1 \
                     ORDER BY rank LIMIT ?2",
                )
                .map_err(store)?;
            let mut rows = stmt.query(params![m, k as i64]).map_err(store)?;
            let mut out = Vec::new();
            while let Some(r) = rows.next().map_err(store)? {
                out.push(row_to_turn(r)?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Total number of turns across all sessions.
    pub async fn turn_count(&self) -> Result<i64> {
        let conn = self.conn.clone();
        tokio::task::spawn_blocking(move || -> Result<i64> {
            let c = conn.lock();
            let n: i64 = c
                .query_row("SELECT COUNT(*) FROM turns", [], |r| r.get(0))
                .map_err(store)?;
            Ok(n)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }
}

// ---- helpers ---------------------------------------------------------------

fn row_to_hit(r: &rusqlite::Row<'_>) -> Result<EpisodeHit> {
    let id: i64 = r.get(0).map_err(store)?;
    let kind: String = r.get(1).map_err(store)?;
    let at_ns: i64 = r.get(2).map_err(store)?;
    let payload: String = r.get(3).map_err(store)?;
    let event: Event = serde_json::from_str(&payload)?;
    let at = OffsetDateTime::from_unix_timestamp_nanos(at_ns as i128)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    Ok(EpisodeHit {
        id,
        kind,
        at,
        event,
    })
}

fn row_to_triple(r: &rusqlite::Row<'_>) -> Result<KgTriple> {
    Ok(KgTriple {
        id: r.get(0).map_err(store)?,
        subject: r.get(1).map_err(store)?,
        predicate: r.get(2).map_err(store)?,
        object: r.get(3).map_err(store)?,
        valid_from: r.get(4).map_err(store)?,
        valid_to: r.get(5).map_err(store)?,
        confidence: r.get(6).map_err(store)?,
        source: r.get(7).map_err(store)?,
    })
}

fn row_to_turn(r: &rusqlite::Row<'_>) -> Result<Turn> {
    let id: i64 = r.get(0).map_err(store)?;
    let session_id: String = r.get(1).map_err(store)?;
    let turn_number_raw: i64 = r.get(2).map_err(store)?;
    let role: String = r.get(3).map_err(store)?;
    let content: String = r.get(4).map_err(store)?;
    let at_ns: i64 = r.get(5).map_err(store)?;
    let turn_number = turn_number_raw;
    let at = OffsetDateTime::from_unix_timestamp_nanos(at_ns as i128)
        .unwrap_or(OffsetDateTime::UNIX_EPOCH);
    Ok(Turn {
        id,
        session_id,
        turn_number,
        role,
        content,
        at,
    })
}

fn event_kind_tag(ev: &Event) -> &'static str {
    match ev {
        Event::FileChanged { .. } => "file_changed",
        Event::FileDeleted { .. } => "file_deleted",
        Event::QueryIssued { .. } => "query_issued",
        Event::AnswerReturned { .. } => "answer_returned",
        Event::OutcomeObserved { .. } => "outcome_observed",
        Event::NudgeInvoked { .. } => "nudge_invoked",
    }
}

fn event_id_of(ev: &Event) -> Option<EventId> {
    match ev {
        Event::QueryIssued { id, .. }
        | Event::AnswerReturned { id, .. }
        | Event::NudgeInvoked { id, .. } => Some(*id),
        Event::OutcomeObserved { related_to, .. } => Some(*related_to),
        _ => None,
    }
}

fn event_at_ns(ev: &Event) -> i64 {
    let t = match ev {
        Event::FileChanged { at, .. }
        | Event::FileDeleted { at, .. }
        | Event::QueryIssued { at, .. }
        | Event::AnswerReturned { at, .. }
        | Event::OutcomeObserved { at, .. }
        | Event::NudgeInvoked { at, .. } => at,
    };
    t.unix_timestamp_nanos().min(i64::MAX as i128) as i64
}

fn store<E: std::fmt::Display>(e: E) -> Error {
    Error::Store(e.to_string())
}

/// The schema version this binary understands. Bump when making breaking
/// schema changes; older binaries will refuse to open a newer database.
const EXPECTED_SCHEMA_VERSION: u32 = 1;

/// Read `schema_version` from `_thoth_meta` and return an error if it is
/// higher than [`EXPECTED_SCHEMA_VERSION`]. This prevents an old binary from
/// silently corrupting a database that was created by a newer version.
fn check_schema_version(c: &Connection) -> Result<()> {
    let version_str: String = c
        .query_row(
            "SELECT value FROM _thoth_meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .map_err(store)?;
    let version: u32 = version_str.parse().map_err(|_| {
        Error::Store(format!(
            "episodes.db: invalid schema_version value '{version_str}'"
        ))
    })?;
    if version > EXPECTED_SCHEMA_VERSION {
        return Err(Error::Store(format!(
            "episodes.db schema version {version} is newer than this binary supports \
             (expected <= {EXPECTED_SCHEMA_VERSION}); please upgrade Thoth"
        )));
    }
    Ok(())
}

/// Idempotently add the decay-related columns to a pre-existing `episodes`
/// table. `CREATE TABLE IF NOT EXISTS` above already produces the new shape
/// on fresh stores, so this only fires on databases that were created
/// before the columns existed.
fn ensure_decay_columns(c: &Connection) -> Result<()> {
    let existing = existing_columns(c)?;
    if !existing.iter().any(|n| n == "salience") {
        c.execute_batch("ALTER TABLE episodes ADD COLUMN salience REAL NOT NULL DEFAULT 1.0")
            .map_err(store)?;
    }
    if !existing.iter().any(|n| n == "access_count") {
        c.execute_batch("ALTER TABLE episodes ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0")
            .map_err(store)?;
    }
    if !existing.iter().any(|n| n == "last_accessed_ns") {
        // Nullable: NULL means "never retrieved"; the forget pass falls
        // back to `at_unix_ns` in that case.
        c.execute_batch("ALTER TABLE episodes ADD COLUMN last_accessed_ns INTEGER")
            .map_err(store)?;
    }
    Ok(())
}

fn existing_columns(c: &Connection) -> Result<Vec<String>> {
    let mut stmt = c.prepare("PRAGMA table_info(episodes)").map_err(store)?;
    let mut rows = stmt.query([]).map_err(store)?;
    let mut out = Vec::new();
    while let Some(r) = rows.next().map_err(store)? {
        // PRAGMA table_info columns: cid, name, type, notnull, dflt_value, pk
        let name: String = r.get(1).map_err(store)?;
        out.push(name);
    }
    Ok(out)
}
