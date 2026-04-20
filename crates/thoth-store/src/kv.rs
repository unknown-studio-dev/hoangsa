//! Key-value / graph store backed by [`redb`].
//!
//! Five logical tables live in a single `kv.redb` file:
//!
//! - `nodes`          — graph nodes, keyed by fully-qualified name.
//! - `edges`          — graph edges, keyed by `"<src>|<kind>|<dst>"`.
//! - `edges_by_dst`   — reverse edge index, keyed by `"<dst>|<kind>|<src>"`.
//!   Value is the same JSON-encoded `EdgeRow` as `edges` so a reverse
//!   lookup is a single range-scan + decode (no point-lookup back into
//!   `edges`). The cost is 2× edge-row storage; at thoth's scale that's
//!   negligible and we save the O(|EDGES|) table scan that `edges_to`
//!   used to do.
//! - `symbols`— symbol → `(path, line_start, line_end)` lookups.
//! - `meta`   — free-form metadata (config, cursor positions, ...).
//!
//! Values are opaque byte strings; callers are expected to pick their own
//! serialization (we use JSON for human-friendly debugging in early
//! milestones). redb is synchronous, so every public method wraps the actual
//! I/O in [`tokio::task::spawn_blocking`].

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{Deserialize, Serialize};
use thoth_core::{Error, Result};

// ---- table definitions ------------------------------------------------------

const NODES: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("nodes");
const EDGES: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("edges");
const EDGES_BY_DST: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("edges_by_dst");
const SYMBOLS: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("symbols");
const META: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("meta");

/// Meta-key flag recording that every existing row in `edges` has been
/// mirrored into `edges_by_dst`. Set once per database (the first open
/// that sees the reverse table) and checked on every subsequent open so
/// the backfill only runs once.
const REV_EDGES_BACKFILLED: &str = "schema::edges_by_dst::backfilled";

// ---- public payload types ---------------------------------------------------

/// Symbol location record written by the indexer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolRow {
    /// Fully qualified name (e.g. `my_crate::module::fn_name`).
    pub fqn: String,
    /// Absolute file path.
    pub path: PathBuf,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line (inclusive).
    pub end_line: u32,
    /// Coarse kind (`"function"`, `"type"`, `"trait"`, ...).
    pub kind: String,
}

/// Graph node (stored in the `nodes` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeRow {
    /// Node id — typically an FQN.
    pub id: String,
    /// Node kind tag.
    pub kind: String,
    /// Optional JSON payload with extra per-node data.
    pub payload: serde_json::Value,
}

/// Graph edge (stored in the `edges` table).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeRow {
    /// Source node id.
    pub src: String,
    /// Destination node id.
    pub dst: String,
    /// Edge kind (e.g. `"calls"`, `"imports"`, `"defines"`).
    pub kind: String,
    /// Optional JSON payload.
    pub payload: serde_json::Value,
}

/// BFS direction for [`KvStore::graph_bfs`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BfsDir {
    /// Follow outgoing edges (src → dst).
    Out,
    /// Follow incoming edges (dst ← src).
    In,
    /// Union of both.
    Both,
}

impl BfsDir {
    fn walks_out(self) -> bool {
        matches!(self, BfsDir::Out | BfsDir::Both)
    }
    fn walks_in(self) -> bool {
        matches!(self, BfsDir::In | BfsDir::Both)
    }
}

// ---- handle ----------------------------------------------------------------

/// Handle to the redb-backed KV + graph store.
///
/// Cheap to clone; the underlying [`Database`] is shared behind an [`Arc`].
#[derive(Clone)]
pub struct KvStore {
    db: Arc<Database>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl KvStore {
    /// Open (or create) the store at `path` (a file, not a directory).
    ///
    /// On first open the required tables are created so every subsequent
    /// read is guaranteed to find them.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let db_path = path.clone();
        let db = tokio::task::spawn_blocking(move || -> Result<Database> {
            let db = Database::create(&db_path).map_err(store)?;
            // Ensure all tables exist, and — if this is a DB that predates
            // the reverse edge index — mirror every existing forward edge
            // into `edges_by_dst`. The backfill is gated on a meta flag so
            // subsequent opens skip it even on a warm DB.
            let wtxn = db.begin_write().map_err(store)?;
            {
                let _ = wtxn.open_table(NODES).map_err(store)?;
                let _ = wtxn.open_table(EDGES).map_err(store)?;
                let _ = wtxn.open_table(EDGES_BY_DST).map_err(store)?;
                let _ = wtxn.open_table(SYMBOLS).map_err(store)?;
                let _ = wtxn.open_table(META).map_err(store)?;

                let meta = wtxn.open_table(META).map_err(store)?;
                let already_backfilled = meta.get(REV_EDGES_BACKFILLED).map_err(store)?.is_some();
                drop(meta);

                if !already_backfilled {
                    let edges = wtxn.open_table(EDGES).map_err(store)?;
                    let rows: Vec<(String, Vec<u8>)> = edges
                        .iter()
                        .map_err(store)?
                        .filter_map(|r| r.ok())
                        .map(|(k, v)| (k.value().to_string(), v.value().to_vec()))
                        .collect();
                    drop(edges);

                    let mut rev = wtxn.open_table(EDGES_BY_DST).map_err(store)?;
                    for (fwd_key, value) in &rows {
                        if let Some(rev_key) = forward_to_reverse_key(fwd_key) {
                            rev.insert(rev_key.as_str(), value.as_slice())
                                .map_err(store)?;
                        }
                    }
                    drop(rev);

                    let mut meta = wtxn.open_table(META).map_err(store)?;
                    meta.insert(REV_EDGES_BACKFILLED, &b"1"[..])
                        .map_err(store)?;
                }
            }
            wtxn.commit().map_err(store)?;
            Ok(db)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))??;

        Ok(Self {
            db: Arc::new(db),
            path,
        })
    }

    // --- meta -----------------------------------------------------------

    /// Store a meta value (config, cursor, ...).
    pub async fn put_meta(&self, key: impl Into<String>, value: &[u8]) -> Result<()> {
        let db = self.db.clone();
        let key = key.into();
        let value = value.to_vec();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut t = wtxn.open_table(META).map_err(store)?;
                t.insert(key.as_str(), value.as_slice()).map_err(store)?;
            }
            wtxn.commit().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Fetch a previously stored meta value.
    pub async fn get_meta(&self, key: impl Into<String>) -> Result<Option<Vec<u8>>> {
        let db = self.db.clone();
        let key = key.into();
        tokio::task::spawn_blocking(move || -> Result<Option<Vec<u8>>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(META).map_err(store)?;
            Ok(t.get(key.as_str())
                .map_err(store)?
                .map(|g| g.value().to_vec()))
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Remove a meta value. No-op if the key was never set. Returns
    /// whether a row was actually removed.
    pub async fn delete_meta(&self, key: impl Into<String>) -> Result<bool> {
        let db = self.db.clone();
        let key = key.into();
        tokio::task::spawn_blocking(move || -> Result<bool> {
            let wtxn = db.begin_write().map_err(store)?;
            let removed = {
                let mut t = wtxn.open_table(META).map_err(store)?;
                t.remove(key.as_str()).map_err(store)?.is_some()
            };
            wtxn.commit().map_err(store)?;
            Ok(removed)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    // --- symbols --------------------------------------------------------

    /// Insert or replace a symbol row. Key is the FQN.
    pub async fn put_symbol(&self, row: SymbolRow) -> Result<()> {
        let db = self.db.clone();
        let bytes = serde_json::to_vec(&row)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut t = wtxn.open_table(SYMBOLS).map_err(store)?;
                t.insert(row.fqn.as_str(), bytes.as_slice())
                    .map_err(store)?;
            }
            wtxn.commit().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Insert many symbol rows in a single redb transaction.
    pub async fn put_symbols_batch(&self, rows: Vec<SymbolRow>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut t = wtxn.open_table(SYMBOLS).map_err(store)?;
                for row in &rows {
                    let bytes = serde_json::to_vec(row)?;
                    t.insert(row.fqn.as_str(), bytes.as_slice())
                        .map_err(store)?;
                }
            }
            wtxn.commit().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Fetch a symbol row by FQN.
    pub async fn get_symbol(&self, fqn: impl Into<String>) -> Result<Option<SymbolRow>> {
        let db = self.db.clone();
        let fqn = fqn.into();
        tokio::task::spawn_blocking(move || -> Result<Option<SymbolRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(SYMBOLS).map_err(store)?;
            let Some(g) = t.get(fqn.as_str()).map_err(store)? else {
                return Ok(None);
            };
            let row: SymbolRow = serde_json::from_slice(g.value())?;
            Ok(Some(row))
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// List every symbol whose FQN starts with `prefix`.
    pub async fn symbols_with_prefix(&self, prefix: impl Into<String>) -> Result<Vec<SymbolRow>> {
        let db = self.db.clone();
        let prefix = prefix.into();
        tokio::task::spawn_blocking(move || -> Result<Vec<SymbolRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(SYMBOLS).map_err(store)?;
            let mut out = Vec::new();
            // Use a range scan starting at `prefix` and stop as soon as the
            // key no longer shares the prefix — O(k) instead of O(N).
            for entry in t.range(prefix.as_str()..).map_err(store)? {
                let (k, v) = entry.map_err(store)?;
                if !k.value().starts_with(prefix.as_str()) {
                    break;
                }
                out.push(serde_json::from_slice(v.value())?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    // --- graph ----------------------------------------------------------

    /// Upsert a graph node.
    pub async fn put_node(&self, row: NodeRow) -> Result<()> {
        let db = self.db.clone();
        let bytes = serde_json::to_vec(&row)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut t = wtxn.open_table(NODES).map_err(store)?;
                t.insert(row.id.as_str(), bytes.as_slice()).map_err(store)?;
            }
            wtxn.commit().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Upsert a graph edge.
    ///
    /// Writes to both the forward table (`edges`, key `"<src>|<kind>|<dst>"`)
    /// and the reverse index (`edges_by_dst`, key `"<dst>|<kind>|<src>"`)
    /// in the same transaction so the two indexes can never disagree on
    /// whether an edge exists.
    pub async fn put_edge(&self, row: EdgeRow) -> Result<()> {
        let db = self.db.clone();
        let fwd_key = format!("{}|{}|{}", row.src, row.kind, row.dst);
        let rev_key = format!("{}|{}|{}", row.dst, row.kind, row.src);
        let bytes = serde_json::to_vec(&row)?;
        tokio::task::spawn_blocking(move || -> Result<()> {
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut fwd = wtxn.open_table(EDGES).map_err(store)?;
                fwd.insert(fwd_key.as_str(), bytes.as_slice())
                    .map_err(store)?;
                let mut rev = wtxn.open_table(EDGES_BY_DST).map_err(store)?;
                rev.insert(rev_key.as_str(), bytes.as_slice())
                    .map_err(store)?;
            }
            wtxn.commit().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Insert many graph nodes in a single redb transaction.
    pub async fn put_nodes_batch(&self, rows: Vec<NodeRow>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut t = wtxn.open_table(NODES).map_err(store)?;
                for row in &rows {
                    let bytes = serde_json::to_vec(row)?;
                    t.insert(row.id.as_str(), bytes.as_slice()).map_err(store)?;
                }
            }
            wtxn.commit().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Insert many graph edges in a single redb transaction. Updates both
    /// the forward and reverse edge tables atomically.
    pub async fn put_edges_batch(&self, rows: Vec<EdgeRow>) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut fwd = wtxn.open_table(EDGES).map_err(store)?;
                let mut rev = wtxn.open_table(EDGES_BY_DST).map_err(store)?;
                for row in &rows {
                    let fwd_key = format!("{}|{}|{}", row.src, row.kind, row.dst);
                    let rev_key = format!("{}|{}|{}", row.dst, row.kind, row.src);
                    let bytes = serde_json::to_vec(row)?;
                    fwd.insert(fwd_key.as_str(), bytes.as_slice())
                        .map_err(store)?;
                    rev.insert(rev_key.as_str(), bytes.as_slice())
                        .map_err(store)?;
                }
            }
            wtxn.commit().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// List every outgoing edge from `src`.
    pub async fn edges_from(&self, src: impl Into<String>) -> Result<Vec<EdgeRow>> {
        let db = self.db.clone();
        let src = src.into();
        tokio::task::spawn_blocking(move || -> Result<Vec<EdgeRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(EDGES).map_err(store)?;
            // Edge keys are `"<src>|<kind>|<dst>"`. Use a range scan over
            // `"<src>|".."<src>|\u{10FFFF}"` (max scalar value in UTF-8)
            // so this is O(matches), not O(|EDGES|).
            let lo = format!("{src}|");
            let hi = format!("{src}|\u{10FFFF}");
            let mut out = Vec::new();
            for entry in t.range(lo.as_str()..hi.as_str()).map_err(store)? {
                let (_k, v) = entry.map_err(store)?;
                out.push(serde_json::from_slice(v.value())?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// List every incoming edge to `dst` via the `edges_by_dst` reverse
    /// index — O(matches) range scan, symmetric with [`Self::edges_from`].
    pub async fn edges_to(&self, dst: impl Into<String>) -> Result<Vec<EdgeRow>> {
        let db = self.db.clone();
        let dst = dst.into();
        tokio::task::spawn_blocking(move || -> Result<Vec<EdgeRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(EDGES_BY_DST).map_err(store)?;
            let lo = format!("{dst}|");
            let hi = format!("{dst}|\u{10FFFF}");
            let mut out = Vec::new();
            for entry in t.range(lo.as_str()..hi.as_str()).map_err(store)? {
                let (_k, v) = entry.map_err(store)?;
                out.push(serde_json::from_slice(v.value())?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Delete every symbol row whose `path` matches `path`, returning the
    /// FQNs that were removed so the caller can also prune the graph nodes
    /// and edges that referenced them.
    ///
    /// There is no secondary index on `path` — this walks the whole symbols
    /// table. Fine at our scale; if it ever isn't we can add a `path → fqn`
    /// side table.
    pub async fn delete_symbols_by_path(&self, path: impl AsRef<Path>) -> Result<Vec<String>> {
        let db = self.db.clone();
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(SYMBOLS).map_err(store)?;
            let mut keys: Vec<String> = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (k, v) = entry.map_err(store)?;
                let row: SymbolRow = match serde_json::from_slice(v.value()) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if row.path == path {
                    keys.push(k.value().to_string());
                }
            }
            drop(t);
            drop(rtxn);

            if keys.is_empty() {
                return Ok(keys);
            }
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut t = wtxn.open_table(SYMBOLS).map_err(store)?;
                for k in &keys {
                    t.remove(k.as_str()).map_err(store)?;
                }
            }
            wtxn.commit().map_err(store)?;
            Ok(keys)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Every symbol row whose declared path equals `path`. Same O(table-scan)
    /// cost as [`Self::delete_symbols_by_path`]; fine while the number of
    /// indexed symbols stays in the low millions. Returned order matches the
    /// underlying key sort (FQN), which is stable across calls.
    pub async fn symbols_for_path(&self, path: impl AsRef<Path>) -> Result<Vec<SymbolRow>> {
        let db = self.db.clone();
        let path = path.as_ref().to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<Vec<SymbolRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(SYMBOLS).map_err(store)?;
            let mut out = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (_k, v) = entry.map_err(store)?;
                let row: SymbolRow = match serde_json::from_slice(v.value()) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if row.path == path {
                    out.push(row);
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Like [`Self::symbols_for_path`] but tolerates the three flavors of
    /// path a caller might hand us:
    ///
    /// - an absolute path (`/home/u/proj/cli/src/cmd/rule.rs`),
    /// - a cwd-relative path (`cli/src/cmd/rule.rs`) — what `git diff`
    ///   emits,
    /// - a `./`-prefixed path (`./cli/src/cmd/rule.rs`) — what some index
    ///   runs store when the indexer was invoked with `.`.
    ///
    /// Match rule: strip leading `./` + redundant `.` components from both
    /// sides, then exact or either-suffix of the resulting component
    /// sequence. Ordered: the caller gets every match, deduped on the
    /// stored row's path so we never return a single symbol twice.
    ///
    /// Used by `detect_changes` — diff hunks arrive with whatever path
    /// git printed, but the symbols table could have been populated with
    /// any of the three forms depending on how the indexer was invoked.
    /// A strict-equality query is right ~none of the time in practice;
    /// this helper shifts the burden off the caller.
    pub async fn symbols_for_path_like(&self, path: impl AsRef<Path>) -> Result<Vec<SymbolRow>> {
        let db = self.db.clone();
        let needle = normalize_path(path.as_ref());
        tokio::task::spawn_blocking(move || -> Result<Vec<SymbolRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(SYMBOLS).map_err(store)?;
            let mut out = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (_k, v) = entry.map_err(store)?;
                let row: SymbolRow = match serde_json::from_slice(v.value()) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if path_matches_flexible(&row.path, &needle) {
                    out.push(row);
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Delete every graph node whose JSON payload `path` field matches
    /// `path`. Returns the list of node ids removed (so the caller can clean
    /// up the edges that touch them).
    pub async fn delete_nodes_by_path(&self, path: impl AsRef<Path>) -> Result<Vec<String>> {
        let db = self.db.clone();
        let path_str = path.as_ref().to_string_lossy().into_owned();
        tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(NODES).map_err(store)?;
            let mut keys: Vec<String> = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (k, v) = entry.map_err(store)?;
                let row: NodeRow = match serde_json::from_slice(v.value()) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let p = row.payload.get("path").and_then(|x| x.as_str());
                if p == Some(path_str.as_str()) {
                    keys.push(k.value().to_string());
                }
            }
            drop(t);
            drop(rtxn);

            if keys.is_empty() {
                return Ok(keys);
            }
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut t = wtxn.open_table(NODES).map_err(store)?;
                for k in &keys {
                    t.remove(k.as_str()).map_err(store)?;
                }
            }
            wtxn.commit().map_err(store)?;
            Ok(keys)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Delete every edge whose `src` or `dst` appears in `ids`. Keeps the
    /// forward and reverse edge tables consistent: every key removed from
    /// `edges` has its mirror removed from `edges_by_dst` in the same
    /// write transaction.
    pub async fn delete_edges_touching(&self, ids: &[String]) -> Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let db = self.db.clone();
        let ids: std::collections::HashSet<String> = ids.iter().cloned().collect();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(EDGES).map_err(store)?;
            // For every doomed edge we need both keys: the forward key is
            // the row's own key in `edges`, the reverse key is derived from
            // the decoded `(src, kind, dst)`.
            let mut to_drop: Vec<(String, String)> = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (k, v) = entry.map_err(store)?;
                let row: EdgeRow = match serde_json::from_slice(v.value()) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                if ids.contains(&row.src) || ids.contains(&row.dst) {
                    let fwd = k.value().to_string();
                    let rev = format!("{}|{}|{}", row.dst, row.kind, row.src);
                    to_drop.push((fwd, rev));
                }
            }
            drop(t);
            drop(rtxn);

            if to_drop.is_empty() {
                return Ok(0);
            }
            let n = to_drop.len();
            let wtxn = db.begin_write().map_err(store)?;
            {
                let mut fwd = wtxn.open_table(EDGES).map_err(store)?;
                let mut rev = wtxn.open_table(EDGES_BY_DST).map_err(store)?;
                for (fk, rk) in &to_drop {
                    fwd.remove(fk.as_str()).map_err(store)?;
                    rev.remove(rk.as_str()).map_err(store)?;
                }
            }
            wtxn.commit().map_err(store)?;
            Ok(n)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Get a node by id.
    pub async fn get_node(&self, id: impl Into<String>) -> Result<Option<NodeRow>> {
        let db = self.db.clone();
        let id = id.into();
        tokio::task::spawn_blocking(move || -> Result<Option<NodeRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(NODES).map_err(store)?;
            let Some(g) = t.get(id.as_str()).map_err(store)? else {
                return Ok(None);
            };
            Ok(Some(serde_json::from_slice(g.value())?))
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Single-round-trip BFS over the graph.
    ///
    /// Equivalent to repeatedly calling [`Self::edges_from`] / [`Self::edges_to`]
    /// plus [`Self::get_node`] per frontier item, but runs the entire walk
    /// inside one `spawn_blocking` + one redb read transaction. At depth 8
    /// over a 50-node subgraph this collapses ~150 round trips into one
    /// and keeps the snapshot coherent across the whole traversal.
    ///
    /// - `start` is never included in the output.
    /// - `kinds = None` walks every [`EdgeRow::kind`]; otherwise only edges
    ///   whose tag matches one of the supplied strings are followed.
    /// - Returns `(row, depth)` pairs in BFS discovery order so callers can
    ///   group by distance without re-sorting.
    pub async fn graph_bfs(
        &self,
        start: String,
        depth: usize,
        dir: BfsDir,
        kinds: Option<Vec<String>>,
    ) -> Result<Vec<(NodeRow, usize)>> {
        if depth == 0 {
            return Ok(Vec::new());
        }
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<(NodeRow, usize)>> {
            let rtxn = db.begin_read().map_err(store)?;
            let nodes = rtxn.open_table(NODES).map_err(store)?;
            let edges = rtxn.open_table(EDGES).map_err(store)?;
            let edges_rev = rtxn.open_table(EDGES_BY_DST).map_err(store)?;

            // Decode a 3-part `"a|b|c"` key. Used for both forward keys
            // (`"<src>|<kind>|<dst>"`) and reverse keys (`"<dst>|<kind>|<src>"`).
            // The kind tags we actually write never contain `|` (see
            // EdgeKind::tag) so a strict 3-way split is safe.
            let decode = |k: &str| -> Option<(String, String, String)> {
                let (a, rest) = k.split_once('|')?;
                let (b, c) = rest.split_once('|')?;
                Some((a.to_string(), b.to_string(), c.to_string()))
            };
            let kind_ok = |k: &str| {
                kinds
                    .as_ref()
                    .is_none_or(|allow| allow.iter().any(|a| a == k))
            };

            let mut seen: std::collections::HashSet<String> =
                std::collections::HashSet::from([start.clone()]);
            let mut frontier: std::collections::VecDeque<(String, usize)> =
                std::collections::VecDeque::from([(start, 0)]);
            let mut out: Vec<(NodeRow, usize)> = Vec::new();

            // Step helper: emit every neighbour id reachable from `cur` via
            // the configured direction + kind filter.
            let step = |cur: &str| -> Result<Vec<String>> {
                let mut ids = Vec::new();
                if dir.walks_out() {
                    // Prefix scan: every key starting with "<cur>|".
                    let lo = format!("{cur}|");
                    let hi = format!("{cur}|\u{10FFFF}");
                    for entry in edges.range(lo.as_str()..hi.as_str()).map_err(store)? {
                        let (k, _v) = entry.map_err(store)?;
                        if let Some((_src, kind, dst)) = decode(k.value())
                            && kind_ok(&kind)
                        {
                            ids.push(dst);
                        }
                    }
                }
                if dir.walks_in() {
                    // Reverse index: edges_by_dst keys are
                    // "<dst>|<kind>|<src>" so the same prefix-range trick
                    // that `edges_from` uses gives us O(matches) here.
                    let lo = format!("{cur}|");
                    let hi = format!("{cur}|\u{10FFFF}");
                    for entry in edges_rev.range(lo.as_str()..hi.as_str()).map_err(store)? {
                        let (k, _v) = entry.map_err(store)?;
                        if let Some((_dst, kind, src)) = decode(k.value())
                            && kind_ok(&kind)
                        {
                            ids.push(src);
                        }
                    }
                }
                Ok(ids)
            };

            while let Some((cur, d)) = frontier.pop_front() {
                if d >= depth {
                    continue;
                }
                for nid in step(&cur)? {
                    if !seen.insert(nid.clone()) {
                        continue;
                    }
                    // Node resolution reuses the same read txn — no extra
                    // spawn_blocking, no snapshot churn.
                    if let Some(g) = nodes.get(nid.as_str()).map_err(store)? {
                        let row: NodeRow = serde_json::from_slice(g.value())?;
                        out.push((row, d + 1));
                    }
                    frontier.push_back((nid, d + 1));
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Return every graph node whose payload `path` field matches `path`.
    ///
    /// There is no secondary index on `path` — this walks the whole nodes
    /// table. Fine at our scale (called at most `top_k` times per query) and
    /// symmetric with [`Self::delete_nodes_by_path`].
    pub async fn nodes_for_path(&self, path: impl AsRef<Path>) -> Result<Vec<NodeRow>> {
        let db = self.db.clone();
        let path_str = path.as_ref().to_string_lossy().into_owned();
        tokio::task::spawn_blocking(move || -> Result<Vec<NodeRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(NODES).map_err(store)?;
            let mut out = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (_k, v) = entry.map_err(store)?;
                let row: NodeRow = match serde_json::from_slice(v.value()) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let p = row.payload.get("path").and_then(|x| x.as_str());
                if p == Some(path_str.as_str()) {
                    out.push(row);
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Find every node whose FQN ends with `needle` on a `::` component
    /// boundary.
    ///
    /// The graph keys nodes by `module::name` — a bare filestem plus the
    /// symbol's simple name — while callers often pass in fully qualified
    /// paths they inferred from the source tree (`cli::cmd::rule::foo`)
    /// or the simple name alone (`foo`). Rather than fail on the exact
    /// miss, `impact` / `symbol_context` fall back to this scan and
    /// promote a unique hit to the canonical form. O(|NODES|) table scan
    /// — fine while `nodes` stays in the low millions.
    pub async fn find_nodes_by_suffix(&self, needle: &str) -> Result<Vec<NodeRow>> {
        let db = self.db.clone();
        let needle = needle.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<NodeRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(NODES).map_err(store)?;
            let mut out = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (k, v) = entry.map_err(store)?;
                let key = k.value();
                // Either direction of `::`-boundary suffix overlap matches,
                // covering both calling conventions:
                //   needle "cmd_enforce"           → key "hook::cmd_enforce"
                //   needle "cli::cmd::rule::foo"   → key "rule::foo"
                let needle_is_suffix_of_key = key
                    .strip_suffix(needle.as_str())
                    .is_some_and(|prefix| prefix.ends_with("::"));
                let key_is_suffix_of_needle = needle
                    .strip_suffix(key)
                    .is_some_and(|prefix| prefix.ends_with("::"));
                if (key == needle || needle_is_suffix_of_key || key_is_suffix_of_needle)
                    && let Ok(row) = serde_json::from_slice::<NodeRow>(v.value())
                {
                    out.push(row);
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Like [`Self::nodes_for_path`] but tolerant of absolute /
    /// cwd-relative / `./`-prefixed variants — symmetric with
    /// [`Self::symbols_for_path_like`].
    pub async fn nodes_for_path_like(&self, path: impl AsRef<Path>) -> Result<Vec<NodeRow>> {
        let db = self.db.clone();
        let needle = normalize_path(path.as_ref());
        tokio::task::spawn_blocking(move || -> Result<Vec<NodeRow>> {
            let rtxn = db.begin_read().map_err(store)?;
            let t = rtxn.open_table(NODES).map_err(store)?;
            let mut out = Vec::new();
            for entry in t.iter().map_err(store)? {
                let (_k, v) = entry.map_err(store)?;
                let row: NodeRow = match serde_json::from_slice(v.value()) {
                    Ok(r) => r,
                    Err(_) => continue,
                };
                let stored = match row.payload.get("path").and_then(|x| x.as_str()) {
                    Some(s) => Path::new(s).to_path_buf(),
                    None => continue,
                };
                if path_matches_flexible(&stored, &needle) {
                    out.push(row);
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }
}

// ---- helpers ---------------------------------------------------------------

fn store<E: std::fmt::Display>(e: E) -> Error {
    Error::Store(e.to_string())
}

/// Flip a forward edge key (`"<src>|<kind>|<dst>"`) into the reverse
/// shape (`"<dst>|<kind>|<src>"`) used by `edges_by_dst`. Returns `None`
/// if the input isn't a 3-part key — those get skipped during backfill.
fn forward_to_reverse_key(fwd: &str) -> Option<String> {
    let (src, rest) = fwd.split_once('|')?;
    let (kind, dst) = rest.split_once('|')?;
    Some(format!("{dst}|{kind}|{src}"))
}

/// Drop `.` components and parent-less `./` so `./a/b` and `a/b` compare
/// equal. Does not resolve `..` — keep that literal so we never cross
/// boundaries implicitly.
fn normalize_path(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => continue,
            _ => out.push(c.as_os_str()),
        }
    }
    out
}

/// True if `stored` and `needle` refer to the same file after stripping
/// `./` components — either exact equality, or either path is a
/// trailing-component suffix of the other.
///
/// Suffix-on-components (not suffix-on-string) so `foo/rule.rs` does not
/// match `foorule.rs` and `src/rule.rs` does not match `other_src/rule.rs`.
fn path_matches_flexible(stored: &Path, needle: &Path) -> bool {
    let s = normalize_path(stored);
    let n = needle.to_path_buf();
    if s == n {
        return true;
    }
    let s_comps: Vec<_> = s.components().collect();
    let n_comps: Vec<_> = n.components().collect();
    if s_comps.is_empty() || n_comps.is_empty() {
        return false;
    }
    let (shorter, longer) = if n_comps.len() <= s_comps.len() {
        (&n_comps, &s_comps)
    } else {
        (&s_comps, &n_comps)
    };
    longer[longer.len() - shorter.len()..] == shorter[..]
}

// ---- tests -----------------------------------------------------------------

#[cfg(test)]
mod prefix_scan_tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_symbol(fqn: &str) -> SymbolRow {
        SymbolRow {
            fqn: fqn.to_string(),
            path: PathBuf::from("/fake/path.rs"),
            start_line: 1,
            end_line: 10,
            kind: "function".to_string(),
        }
    }

    async fn open_store() -> (TempDir, KvStore) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("kv.redb");
        let store = KvStore::open(&db_path).await.unwrap();
        (dir, store)
    }

    /// Suffix / relative / absolute path forms must all hit the same
    /// symbol row so `detect_changes` works regardless of whether the
    /// indexer was invoked with `.`, `./`, or an absolute path. Mirror
    /// the realistic case: stored path is absolute, needle is the
    /// cwd-relative form `git diff` prints.
    #[tokio::test]
    async fn symbols_for_path_like_matches_all_flavours() {
        let (_dir, store) = open_store().await;
        let row = SymbolRow {
            fqn: "rule::Rule".to_string(),
            path: PathBuf::from("/abs/proj/cli/src/cmd/rule.rs"),
            start_line: 16,
            end_line: 31,
            kind: "type".to_string(),
        };
        store.put_symbol(row).await.unwrap();

        // Three flavours a caller might pass in.
        for needle in [
            "/abs/proj/cli/src/cmd/rule.rs",
            "cli/src/cmd/rule.rs",
            "./cli/src/cmd/rule.rs",
        ] {
            let hits = store
                .symbols_for_path_like(PathBuf::from(needle))
                .await
                .unwrap();
            assert_eq!(
                hits.len(),
                1,
                "needle {needle:?} should match the stored absolute path"
            );
        }

        // A component-disjoint basename collision must NOT match.
        let hits = store
            .symbols_for_path_like(PathBuf::from("other/rule.rs"))
            .await
            .unwrap();
        assert!(
            hits.is_empty(),
            "basename-only match would be a false positive"
        );
    }

    /// Suffix FQN lookup used by `impact`/`symbol_context` when the
    /// user supplies a more-qualified path than the graph key.
    #[tokio::test]
    async fn find_nodes_by_suffix_respects_module_boundary() {
        let (_dir, store) = open_store().await;
        store
            .put_nodes_batch(vec![
                NodeRow {
                    id: "hook::cmd_enforce".to_string(),
                    kind: "function".to_string(),
                    payload: serde_json::json!({"path": "hook.rs", "line": 10}),
                },
                NodeRow {
                    id: "rule::cmd_enforce".to_string(),
                    kind: "function".to_string(),
                    payload: serde_json::json!({"path": "rule.rs", "line": 20}),
                },
                // Same trailing chars but NOT on a module boundary —
                // must not match `cmd_enforce`.
                NodeRow {
                    id: "x::xcmd_enforce".to_string(),
                    kind: "function".to_string(),
                    payload: serde_json::json!({"path": "x.rs", "line": 5}),
                },
            ])
            .await
            .unwrap();

        // Qualified suffix → unique hit.
        let hits = store.find_nodes_by_suffix("hook::cmd_enforce").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, "hook::cmd_enforce");

        // Bare leaf → both real matches, not the `xcmd_enforce` red herring.
        let hits = store.find_nodes_by_suffix("cmd_enforce").await.unwrap();
        let mut ids: Vec<_> = hits.into_iter().map(|n| n.id).collect();
        ids.sort();
        assert_eq!(ids, vec!["hook::cmd_enforce", "rule::cmd_enforce"]);
    }

    /// REQ-02: prefix_scan_early_exit — `symbols_with_prefix` must stop as
    /// soon as the sorted key range exits the prefix, returning only foo:*
    /// keys when bar:* keys are also present.
    #[tokio::test]
    async fn prefix_scan_early_exit() {
        let (_dir, store) = open_store().await;

        // Insert keys in a mix of prefixes; redb stores them sorted by key.
        for key in &["foo:1", "foo:2", "foo:3", "bar:1", "bar:2"] {
            store.put_symbol(make_symbol(key)).await.unwrap();
        }

        let results = store.symbols_with_prefix("foo:").await.unwrap();

        // Only foo: keys must be returned — no bar: keys.
        for row in &results {
            assert!(
                row.fqn.starts_with("foo:"),
                "unexpected key in results: {}",
                row.fqn
            );
        }

        // Exactly 3 results (foo:1, foo:2, foo:3).
        assert_eq!(
            results.len(),
            3,
            "expected 3 foo: keys, got {}",
            results.len()
        );
    }

    fn edge(src: &str, kind: &str, dst: &str) -> EdgeRow {
        EdgeRow {
            src: src.into(),
            dst: dst.into(),
            kind: kind.into(),
            payload: serde_json::json!({}),
        }
    }

    /// `edges_to` must use the reverse index and return every incoming
    /// edge to `dst`, with no false positives from sibling dst names that
    /// happen to share a prefix.
    #[tokio::test]
    async fn edges_to_range_scan_is_exact() {
        let (_dir, kv) = open_store().await;

        kv.put_edges_batch(vec![
            edge("a", "calls", "target"),
            edge("b", "calls", "target"),
            edge("c", "imports", "target"),
            // Same-prefix red herring: `target2` must not leak into the
            // result for `target`.
            edge("a", "calls", "target2"),
            edge("unrelated", "calls", "other"),
        ])
        .await
        .unwrap();

        let mut srcs: Vec<String> = kv
            .edges_to("target")
            .await
            .unwrap()
            .into_iter()
            .map(|e| format!("{}/{}", e.src, e.kind))
            .collect();
        srcs.sort();
        assert_eq!(
            srcs,
            vec![
                "a/calls".to_string(),
                "b/calls".to_string(),
                "c/imports".to_string(),
            ]
        );
    }

    /// Deleting edges through `delete_edges_touching` must purge both
    /// tables so a subsequent `edges_to` returns no ghost rows.
    #[tokio::test]
    async fn delete_edges_touching_purges_reverse_index() {
        let (_dir, kv) = open_store().await;

        kv.put_edges_batch(vec![
            edge("a", "calls", "target"),
            edge("b", "calls", "target"),
        ])
        .await
        .unwrap();

        let removed = kv.delete_edges_touching(&["a".to_string()]).await.unwrap();
        assert_eq!(removed, 1);

        let remaining: Vec<String> = kv
            .edges_to("target")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.src)
            .collect();
        assert_eq!(remaining, vec!["b".to_string()]);
    }

    /// A DB created before `edges_by_dst` existed only has rows in
    /// `edges`. Opening it must backfill the reverse index so `edges_to`
    /// still returns every incoming edge on the first post-upgrade call.
    #[tokio::test]
    async fn open_backfills_reverse_index_from_legacy_edges() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("kv.redb");

        // Simulate a legacy DB: only the forward `edges` table has rows,
        // and the backfill-done flag is cleared so the next `open` re-
        // runs the migration path.
        {
            let kv = KvStore::open(&db_path).await.unwrap();
            kv.put_edges_batch(vec![
                edge("legacy_a", "calls", "legacy_target"),
                edge("legacy_b", "imports", "legacy_target"),
            ])
            .await
            .unwrap();

            let db = kv.db.clone();
            tokio::task::spawn_blocking(move || {
                let wtxn = db.begin_write().unwrap();
                {
                    let mut rev = wtxn.open_table(EDGES_BY_DST).unwrap();
                    rev.retain(|_, _| false).unwrap();
                    let mut meta = wtxn.open_table(META).unwrap();
                    meta.remove(REV_EDGES_BACKFILLED).unwrap();
                }
                wtxn.commit().unwrap();
            })
            .await
            .unwrap();
        }

        // Reopen — backfill runs here.
        let kv = KvStore::open(&db_path).await.unwrap();
        let mut got: Vec<String> = kv
            .edges_to("legacy_target")
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.src)
            .collect();
        got.sort();
        assert_eq!(got, vec!["legacy_a".to_string(), "legacy_b".to_string()]);
    }
}
