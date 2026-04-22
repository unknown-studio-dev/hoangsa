//! In-process vector search — replaces the old ChromaDB Python sidecar.
//!
//! ## Why this module exists
//!
//! Until Phase 2 of `fix/memory-4bugs`, semantic search went through a
//! Python subprocess running `chromadb.PersistentClient`. That subprocess
//! carried ~500 MB RSS, loaded ONNX once per invocation, and could not
//! be closed cleanly (Chroma issues #5843, #5868 — see
//! `.hoangsa/sessions/fix/memory-4bugs/RESEARCH.md`). Each Claude Code
//! hook fire — and there are many — spawned a fresh one, which is how
//! the 164 GB disk-fill incident happened.
//!
//! This module replaces that with a Rust-native stack:
//!
//! - Embeddings via [`fastembed`] — ONNX runs in-process (`ort`), no
//!   Python, no venv. Default model is `multilingual-e5-small`
//!   (384-dim) so Vietnamese retrieval isn't degraded the way the old
//!   English-only `all-MiniLM-L6-v2` was.
//! - Vectors stored as raw f32 BLOBs in a per-project SQLite database
//!   (`vectors.sqlite` next to `archive_sessions.db`). Chosen over the
//!   `sqlite-vec` extension because current `sqlite-vec` is also
//!   brute-force (no HNSW yet) and the extension adds a native-binary
//!   distribution burden for no query-time speedup at our scale.
//! - Search is brute-force cosine. At the archive sizes we run at (tens
//!   of thousands of chunks) this is ~20 ms per query — well under the
//!   threshold where HNSW starts to pay for its index-build cost. When
//!   that changes, Phase 3 swaps the impl behind [`VectorStore`] to
//!   LanceDB without touching callers.
//!
//! ## Trait shape
//!
//! The trait pair — [`VectorStore`] (whole DB / connection) and
//! [`VectorCol`] (one collection within it) — mirrors the old
//! `ChromaStore` / `ChromaCol` pair so callers didn't have to change
//! conceptually. The metadata-filter DSL on
//! [`VectorCol::query_text`] / [`VectorCol::delete_by_filter`] also
//! mirrors ChromaDB's JSON filter (`{"field": {"$eq": v}}`, `{"$and": […]}`,
//! etc.) so we don't have to rewrite every existing call site. See
//! [`Filter::parse`] for exactly what's supported.
//!
//! ## e5 prefix discipline
//!
//! `multilingual-e5-small` expects its inputs to be tagged as either
//! `"query: …"` or `"passage: …"` at embed time. Without the tags
//! recall drops materially. Callers never see this — [`EmbeddedVectorCol`]
//! adds the right prefix itself depending on whether the text is being
//! stored (passage) or searched with (query).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use hoangsa_memory_core::{Error, Result};
use parking_lot::Mutex as PlMutex;
use rusqlite::{params, Connection};
use serde_json::Value;
use tokio::sync::Mutex as TokioMutex;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single hit returned from [`VectorCol::query_text`].
#[derive(Debug, Clone)]
pub struct VectorHit {
    /// Stable document identifier chosen by the caller at upsert time.
    pub id: String,
    /// Cosine distance in `[0.0, 2.0]` — lower is closer.
    pub distance: f32,
    /// Original document text when it was stored; `None` if the caller
    /// upserted without a document body.
    pub document: Option<String>,
    /// Arbitrary metadata JSON the caller attached at upsert; `None`
    /// if no metadata was stored for this row.
    pub metadata: Option<HashMap<String, Value>>,
}

/// Descriptive info about a collection handle.
#[derive(Debug, Clone)]
pub struct CollectionInfo {
    /// Stable identifier. For the embedded impl this is identical to
    /// the name — we keep the field separate so future backends
    /// (LanceDB, etc.) can return a server-assigned id without a
    /// breaking API change.
    pub id: String,
    /// Human-readable collection name, as passed to `ensure_collection`.
    pub name: String,
}

// ---------------------------------------------------------------------------
// Traits
// ---------------------------------------------------------------------------

/// Per-project vector store. One handle per `vectors.sqlite` file.
///
/// Collections live *inside* a store — opening or creating one hands
/// back a [`VectorCol`] that shares the underlying database connection
/// with the parent store. Holding the store alive across the lifetime
/// of the collection is the caller's responsibility.
#[async_trait]
pub trait VectorStore: Send + Sync {
    /// Get or create a collection by name. Idempotent: calling twice
    /// with the same name returns handles to the same rows.
    async fn ensure_collection(
        &self,
        name: &str,
    ) -> Result<(Arc<dyn VectorCol>, CollectionInfo)>;

    /// Cheap liveness check. Should return `Ok(true)` unless the
    /// embedder, the SQLite file, or both are known to be unusable.
    async fn health(&self) -> Result<bool>;
}

/// A resolved collection handle returned by
/// [`VectorStore::ensure_collection`].
#[async_trait]
pub trait VectorCol: Send + Sync {
    /// Upsert a batch of documents. All three vectors — `ids`,
    /// `documents`, `metadatas` — must be the same length when
    /// provided. Re-ingesting the same `id` overwrites the row.
    async fn upsert(
        &self,
        ids: Vec<String>,
        documents: Option<Vec<String>>,
        metadatas: Option<Vec<HashMap<String, Value>>>,
    ) -> Result<()>;

    /// Embed `text` and return the `n_results` closest rows. Ties on
    /// distance are broken by insertion order. `where_filter` follows
    /// the ChromaDB JSON filter shape (see [`Filter::parse`]); pass
    /// `None` to search the whole collection.
    async fn query_text(
        &self,
        text: &str,
        n_results: usize,
        where_filter: Option<Value>,
    ) -> Result<Vec<VectorHit>>;

    /// Delete exactly these ids; missing ids are silently ignored.
    async fn delete(&self, ids: Vec<String>) -> Result<()>;

    /// Delete every row whose metadata satisfies `where_filter`. Same
    /// JSON-DSL as [`VectorCol::query_text`]'s filter argument.
    async fn delete_by_filter(&self, where_filter: Value) -> Result<()>;

    /// Row count in this collection.
    async fn count(&self) -> Result<usize>;
}

// ---------------------------------------------------------------------------
// Filter DSL (Chroma-compatible subset)
// ---------------------------------------------------------------------------

/// Parsed representation of ChromaDB's JSON metadata-filter language.
///
/// We accept the subset callers in this workspace actually use — `$eq`,
/// `$ne`, `$in`, `$and`, `$or`. Anything else returns a clear error at
/// parse time so silent "no hits" regressions don't hide upstream bugs.
#[derive(Debug, Clone)]
enum Filter {
    /// `{ "field": { "$eq": value } }` or the shorthand `{ "field": value }`.
    Eq(String, Value),
    /// `{ "field": { "$ne": value } }`.
    Ne(String, Value),
    /// `{ "field": { "$in": [values…] } }`.
    In(String, Vec<Value>),
    /// `{ "$and": [ …subfilters… ] }`.
    And(Vec<Filter>),
    /// `{ "$or": [ …subfilters… ] }`.
    Or(Vec<Filter>),
}

impl Filter {
    fn parse(v: &Value) -> Result<Self> {
        let obj = v
            .as_object()
            .ok_or_else(|| Error::Store("vector filter: expected JSON object".into()))?;
        if obj.len() == 1 {
            let (k, inner) = obj.iter().next().expect("len == 1");
            if k == "$and" || k == "$or" {
                let arr = inner.as_array().ok_or_else(|| {
                    Error::Store(format!("vector filter: `{k}` expects an array"))
                })?;
                let subs: Result<Vec<_>> = arr.iter().map(Filter::parse).collect();
                return Ok(match k.as_str() {
                    "$and" => Filter::And(subs?),
                    "$or" => Filter::Or(subs?),
                    _ => unreachable!(),
                });
            }
            return Filter::parse_field(k, inner);
        }
        // Multi-key object = implicit AND across fields.
        let subs: Result<Vec<_>> = obj
            .iter()
            .map(|(k, inner)| Filter::parse_field(k, inner))
            .collect();
        Ok(Filter::And(subs?))
    }

    fn parse_field(field: &str, inner: &Value) -> Result<Self> {
        match inner {
            Value::Object(op) if op.len() == 1 => {
                let (op_name, op_val) = op.iter().next().expect("len == 1");
                match op_name.as_str() {
                    "$eq" => Ok(Filter::Eq(field.to_string(), op_val.clone())),
                    "$ne" => Ok(Filter::Ne(field.to_string(), op_val.clone())),
                    "$in" => {
                        let arr = op_val.as_array().ok_or_else(|| {
                            Error::Store(format!("vector filter: $in on `{field}` expects array"))
                        })?;
                        Ok(Filter::In(field.to_string(), arr.clone()))
                    }
                    other => Err(Error::Store(format!(
                        "vector filter: unsupported operator `{other}` on `{field}`"
                    ))),
                }
            }
            // Shorthand: { "field": "literal" } ≡ { "field": { "$eq": "literal" } }.
            literal => Ok(Filter::Eq(field.to_string(), literal.clone())),
        }
    }

    fn matches(&self, meta: &HashMap<String, Value>) -> bool {
        match self {
            Filter::Eq(k, v) => meta.get(k).map(|m| json_eq(m, v)).unwrap_or(false),
            Filter::Ne(k, v) => meta.get(k).map(|m| !json_eq(m, v)).unwrap_or(true),
            Filter::In(k, vs) => match meta.get(k) {
                Some(m) => vs.iter().any(|v| json_eq(m, v)),
                None => false,
            },
            Filter::And(subs) => subs.iter().all(|s| s.matches(meta)),
            Filter::Or(subs) => subs.iter().any(|s| s.matches(meta)),
        }
    }
}

/// JSON equality that treats integers and floats with the same numeric
/// value as equal — callers upsert `chunk_index` as `i64` but may query
/// with an `i32` literal that `serde_json` stored as a different
/// underlying variant.
fn json_eq(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

// ---------------------------------------------------------------------------
// Embedded implementation
// ---------------------------------------------------------------------------

/// Default embedding dimensionality for `multilingual-e5-small`.
const EMBED_DIM: usize = 384;

/// Default model. Multilingual (covers Vietnamese) and small enough
/// (~118 MB on disk, ~130 MB RSS once loaded) to comfortably live in
/// the MCP server process.
const DEFAULT_MODEL: EmbeddingModel = EmbeddingModel::MultilingualE5Small;

/// Resolve the directory fastembed should use to cache ONNX weights.
///
/// Resolution order:
///   1. `FASTEMBED_CACHE_DIR` — explicit user override, honored verbatim.
///   2. `HOANGSA_INSTALL_DIR/cache/fastembed` — when the env is set (the
///      installer scripts export it).
///   3. `$HOME/.hoangsa/cache/fastembed` — default on Unix / macOS.
///   4. `./.fastembed_cache` — last-resort, matches fastembed's own
///      default so behavior degrades gracefully on exotic setups.
///
/// Pinning the cache to a single shared directory is the difference
/// between "every project re-downloads 118 MB" and "download once per
/// user". fastembed's own default (`.fastembed_cache`) is relative to
/// CWD, which is the wrong shape for a multi-project CLI.
pub fn fastembed_cache_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("FASTEMBED_CACHE_DIR") {
        return PathBuf::from(p);
    }
    if let Some(root) = std::env::var_os("HOANGSA_INSTALL_DIR") {
        return PathBuf::from(root).join("cache").join("fastembed");
    }
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        return PathBuf::from(home)
            .join(".hoangsa")
            .join("cache")
            .join("fastembed");
    }
    PathBuf::from(".fastembed_cache")
}

/// Build the `InitOptions` we use everywhere the embedder is
/// constructed. Centralising this keeps the `prefetch` command and the
/// runtime `open` path in lockstep — otherwise they'd download into
/// different directories and the prefetch would do nothing.
fn default_init_options() -> InitOptions {
    InitOptions::new(DEFAULT_MODEL)
        .with_cache_dir(fastembed_cache_dir())
        .with_show_download_progress(true)
}

/// Download the default embedding model into the shared cache dir
/// without opening a SQLite file. Used by `hoangsa-memory prefetch-embed`
/// from the installer so the first real invocation doesn't stall for
/// 30–60 s on the HuggingFace fetch.
pub async fn prefetch_model() -> Result<()> {
    let cache_dir = fastembed_cache_dir();
    if let Err(e) = std::fs::create_dir_all(&cache_dir) {
        return Err(Error::Store(format!(
            "create fastembed cache dir {}: {e}",
            cache_dir.display()
        )));
    }
    tokio::task::spawn_blocking(|| TextEmbedding::try_new(default_init_options()))
        .await
        .map_err(|e| Error::Store(format!("prefetch join: {e}")))?
        .map_err(|e| Error::Store(format!("prefetch embedder init: {e}")))?;
    Ok(())
}

/// The concrete [`VectorStore`] backed by fastembed + SQLite.
pub struct EmbeddedVectorStore {
    inner: Arc<StoreInner>,
}

struct StoreInner {
    /// SQLite connection, guarded by a blocking mutex because
    /// `rusqlite::Connection: !Sync`. Writes are short and rare
    /// relative to embed time, so contention is negligible.
    db: PlMutex<Connection>,
    /// The fastembed session. `TextEmbedding::embed` takes `&mut self`,
    /// so we need exclusive access; we hold it under an async mutex so
    /// concurrent callers queue instead of blocking the runtime.
    embedder: TokioMutex<TextEmbedding>,
}

impl EmbeddedVectorStore {
    /// Open or create the vectors SQLite at `data_path` and initialise
    /// the embedding model. First call can be slow (~5–30 s) if
    /// fastembed needs to download model weights from HuggingFace; we
    /// ask it to print progress so the user can tell the process isn't
    /// hung.
    pub async fn open(data_path: &Path) -> Result<Self> {
        if let Some(parent) = data_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Store(format!("create vector dir: {e}")))?;
        }

        let db_path = data_path.to_path_buf();
        let db = tokio::task::spawn_blocking(move || open_sqlite(&db_path))
            .await
            .map_err(|e| Error::Store(format!("vector sqlite join: {e}")))??;

        let embedder = tokio::task::spawn_blocking(|| TextEmbedding::try_new(default_init_options()))
            .await
            .map_err(|e| Error::Store(format!("embedder init join: {e}")))?
            .map_err(|e| Error::Store(format!("embedder init: {e}")))?;

        Ok(Self {
            inner: Arc::new(StoreInner {
                db: PlMutex::new(db),
                embedder: TokioMutex::new(embedder),
            }),
        })
    }
}

#[async_trait]
impl VectorStore for EmbeddedVectorStore {
    async fn ensure_collection(
        &self,
        name: &str,
    ) -> Result<(Arc<dyn VectorCol>, CollectionInfo)> {
        let info = CollectionInfo {
            id: name.to_string(),
            name: name.to_string(),
        };
        let col: Arc<dyn VectorCol> = Arc::new(EmbeddedVectorCol {
            inner: self.inner.clone(),
            collection: name.to_string(),
        });
        Ok((col, info))
    }

    async fn health(&self) -> Result<bool> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<bool> {
            let db = inner.db.lock();
            let _: i64 = db
                .query_row("SELECT 1", [], |r| r.get(0))
                .map_err(|e| Error::Store(format!("vector health: {e}")))?;
            Ok(true)
        })
        .await
        .map_err(|e| Error::Store(format!("vector health join: {e}")))?
    }
}

/// A single collection inside an [`EmbeddedVectorStore`].
struct EmbeddedVectorCol {
    inner: Arc<StoreInner>,
    collection: String,
}

#[async_trait]
impl VectorCol for EmbeddedVectorCol {
    async fn upsert(
        &self,
        ids: Vec<String>,
        documents: Option<Vec<String>>,
        metadatas: Option<Vec<HashMap<String, Value>>>,
    ) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let n = ids.len();
        let documents = documents.unwrap_or_else(|| vec![String::new(); n]);
        let metadatas = metadatas.unwrap_or_else(|| vec![HashMap::new(); n]);
        if documents.len() != n || metadatas.len() != n {
            return Err(Error::Store(format!(
                "upsert: ids/documents/metadatas length mismatch (ids={n}, docs={}, metas={})",
                documents.len(),
                metadatas.len()
            )));
        }

        // e5 convention: tag stored documents as "passage: …".
        let tagged: Vec<String> = documents
            .iter()
            .map(|d| format!("passage: {d}"))
            .collect();
        let vectors = embed_texts(&self.inner, tagged).await?;

        if vectors.len() != n {
            return Err(Error::Store(format!(
                "embedder returned {} vectors for {n} documents",
                vectors.len()
            )));
        }

        let collection = self.collection.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut db = inner.db.lock();
            let tx = db
                .transaction()
                .map_err(|e| Error::Store(format!("begin tx: {e}")))?;
            {
                let mut stmt = tx
                    .prepare(
                        "INSERT INTO vec_chunks (collection, id, embedding, document, metadata)
                         VALUES (?1, ?2, ?3, ?4, ?5)
                         ON CONFLICT(collection, id) DO UPDATE SET
                             embedding = excluded.embedding,
                             document = excluded.document,
                             metadata = excluded.metadata",
                    )
                    .map_err(|e| Error::Store(format!("prepare upsert: {e}")))?;
                for i in 0..n {
                    let blob = vec_to_blob(&vectors[i]);
                    let meta_json = serde_json::to_string(&metadatas[i])
                        .map_err(|e| Error::Store(format!("serialise meta: {e}")))?;
                    stmt.execute(params![
                        collection,
                        ids[i],
                        blob,
                        documents[i],
                        meta_json,
                    ])
                    .map_err(|e| Error::Store(format!("upsert row: {e}")))?;
                }
            }
            tx.commit()
                .map_err(|e| Error::Store(format!("commit upsert: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("upsert join: {e}")))?
    }

    async fn query_text(
        &self,
        text: &str,
        n_results: usize,
        where_filter: Option<Value>,
    ) -> Result<Vec<VectorHit>> {
        if n_results == 0 {
            return Ok(Vec::new());
        }

        let filter = match where_filter {
            Some(v) => Some(Filter::parse(&v)?),
            None => None,
        };

        // Empty-text query is a filter-only fetch — callers use it to
        // pull neighbor chunks by metadata without an embedding
        // round-trip. Skip the embedder and just paginate.
        let query_vec = if text.is_empty() {
            None
        } else {
            let tagged = format!("query: {text}");
            let mut vecs = embed_texts(&self.inner, vec![tagged]).await?;
            Some(
                vecs.pop()
                    .ok_or_else(|| Error::Store("embedder returned no vectors".into()))?,
            )
        };

        let collection = self.collection.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<VectorHit>> {
            let db = inner.db.lock();
            let mut stmt = db
                .prepare(
                    "SELECT id, embedding, document, metadata
                     FROM vec_chunks WHERE collection = ?1",
                )
                .map_err(|e| Error::Store(format!("prepare query: {e}")))?;
            let rows = stmt
                .query_map(params![collection], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Vec<u8>>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                    ))
                })
                .map_err(|e| Error::Store(format!("query vec rows: {e}")))?;

            let mut scored: Vec<(f32, String, String, HashMap<String, Value>)> = Vec::new();
            for row in rows {
                let (id, blob, document, meta_json) =
                    row.map_err(|e| Error::Store(format!("read vec row: {e}")))?;
                let meta: HashMap<String, Value> = if meta_json.is_empty() {
                    HashMap::new()
                } else {
                    serde_json::from_str(&meta_json).unwrap_or_default()
                };
                if let Some(f) = &filter
                    && !f.matches(&meta)
                {
                    continue;
                }
                let dist = match &query_vec {
                    Some(qv) => {
                        let stored = blob_to_vec(&blob);
                        cosine_distance(qv, &stored)
                    }
                    None => 0.0,
                };
                scored.push((dist, id, document, meta));
            }

            scored.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            scored.truncate(n_results);

            let hits = scored
                .into_iter()
                .map(|(distance, id, document, meta)| VectorHit {
                    id,
                    distance,
                    document: if document.is_empty() {
                        None
                    } else {
                        Some(document)
                    },
                    metadata: Some(meta),
                })
                .collect();
            Ok(hits)
        })
        .await
        .map_err(|e| Error::Store(format!("query join: {e}")))?
    }

    async fn delete(&self, ids: Vec<String>) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }
        let collection = self.collection.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut db = inner.db.lock();
            let tx = db
                .transaction()
                .map_err(|e| Error::Store(format!("begin delete tx: {e}")))?;
            {
                let mut stmt = tx
                    .prepare("DELETE FROM vec_chunks WHERE collection = ?1 AND id = ?2")
                    .map_err(|e| Error::Store(format!("prepare delete: {e}")))?;
                for id in &ids {
                    stmt.execute(params![collection, id])
                        .map_err(|e| Error::Store(format!("delete row: {e}")))?;
                }
            }
            tx.commit()
                .map_err(|e| Error::Store(format!("commit delete: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("delete join: {e}")))?
    }

    async fn delete_by_filter(&self, where_filter: Value) -> Result<()> {
        let filter = Filter::parse(&where_filter)?;
        let collection = self.collection.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut db = inner.db.lock();

            // Read → filter in Rust → delete matching ids. We don't
            // translate the whole DSL to SQL because (a) the subset we
            // need wouldn't save much work at current row counts, and
            // (b) the filter rules live in one place (`Filter::matches`)
            // this way, guaranteeing delete_by_filter selects the same
            // rows query_text would have.
            let matching_ids: Vec<String> = {
                let mut stmt = db
                    .prepare(
                        "SELECT id, metadata FROM vec_chunks WHERE collection = ?1",
                    )
                    .map_err(|e| Error::Store(format!("prepare delete_by_filter scan: {e}")))?;
                let rows = stmt
                    .query_map(params![collection], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                    })
                    .map_err(|e| Error::Store(format!("delete_by_filter scan: {e}")))?;
                let mut out = Vec::new();
                for row in rows {
                    let (id, meta_json) =
                        row.map_err(|e| Error::Store(format!("read scan row: {e}")))?;
                    let meta: HashMap<String, Value> = if meta_json.is_empty() {
                        HashMap::new()
                    } else {
                        serde_json::from_str(&meta_json).unwrap_or_default()
                    };
                    if filter.matches(&meta) {
                        out.push(id);
                    }
                }
                out
            };

            if matching_ids.is_empty() {
                return Ok(());
            }

            let tx = db
                .transaction()
                .map_err(|e| Error::Store(format!("begin delete_by_filter tx: {e}")))?;
            {
                let mut stmt = tx
                    .prepare("DELETE FROM vec_chunks WHERE collection = ?1 AND id = ?2")
                    .map_err(|e| Error::Store(format!("prepare delete_by_filter: {e}")))?;
                for id in &matching_ids {
                    stmt.execute(params![collection, id])
                        .map_err(|e| Error::Store(format!("delete_by_filter row: {e}")))?;
                }
            }
            tx.commit()
                .map_err(|e| Error::Store(format!("commit delete_by_filter: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("delete_by_filter join: {e}")))?
    }

    async fn count(&self) -> Result<usize> {
        let collection = self.collection.clone();
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || -> Result<usize> {
            let db = inner.db.lock();
            let n: i64 = db
                .query_row(
                    "SELECT COUNT(*) FROM vec_chunks WHERE collection = ?1",
                    params![collection],
                    |r| r.get(0),
                )
                .map_err(|e| Error::Store(format!("count: {e}")))?;
            Ok(n as usize)
        })
        .await
        .map_err(|e| Error::Store(format!("count join: {e}")))?
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn embed_texts(inner: &Arc<StoreInner>, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
    // `TextEmbedding::embed` is CPU-bound and holds `&mut self`, so we
    // run it on the blocking pool and acquire the async mutex via
    // `blocking_lock` — fine here because the task is *already* on a
    // thread dedicated to blocking work.
    let inner = inner.clone();
    tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
        let mut guard = inner.embedder.blocking_lock();
        guard
            .embed(texts, None)
            .map_err(|e| Error::Store(format!("embed: {e}")))
    })
    .await
    .map_err(|e| Error::Store(format!("embed join: {e}")))?
}

fn open_sqlite(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| Error::Store(format!("open vectors.sqlite: {e}")))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         CREATE TABLE IF NOT EXISTS vec_chunks (
             collection TEXT NOT NULL,
             id         TEXT NOT NULL,
             embedding  BLOB NOT NULL,
             document   TEXT NOT NULL DEFAULT '',
             metadata   TEXT NOT NULL DEFAULT '',
             PRIMARY KEY (collection, id)
         );
         CREATE INDEX IF NOT EXISTS vec_chunks_collection
             ON vec_chunks(collection);",
    )
    .map_err(|e| Error::Store(format!("init vectors schema: {e}")))?;
    Ok(conn)
}

/// Serialise an `f32` vector as tightly packed little-endian bytes. We
/// store the dimension implicitly — every row is `EMBED_DIM` floats —
/// so deserialisation just divides `bytes.len()` by 4.
fn vec_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

fn blob_to_vec(b: &[u8]) -> Vec<f32> {
    let mut out = Vec::with_capacity(b.len() / 4);
    let mut i = 0;
    while i + 4 <= b.len() {
        let arr = [b[i], b[i + 1], b[i + 2], b[i + 3]];
        out.push(f32::from_le_bytes(arr));
        i += 4;
    }
    out
}

/// Cosine *distance* (not similarity): `1 - cos(θ)` in `[0, 2]`. Callers
/// sort ascending so lower = closer. Returns `1.0` (≈ orthogonal) on
/// zero-norm inputs instead of NaN so a query never crashes on a
/// malformed row.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 1.0;
    }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        return 1.0;
    }
    1.0 - (dot / denom)
}

// ---------------------------------------------------------------------------
// Compat: path helper matching the old `StoreRoot::chroma_path`.
// ---------------------------------------------------------------------------

/// Canonical on-disk location for the vectors SQLite inside a store
/// root. Lives next to `archive_sessions.db`, not inside a subdir, so
/// a `rm -rf <root>/chroma` from the Chroma era never lands on it.
pub fn vectors_path(root: &Path) -> PathBuf {
    root.join("vectors.sqlite")
}

// Silence the unused-const warning when no caller references EMBED_DIM
// directly — it's still load-bearing as documentation.
#[allow(dead_code)]
const _EMBED_DIM_DOC: usize = EMBED_DIM;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_parses_eq_shorthand_and_long_form() {
        let short = serde_json::json!({"a": "x"});
        let long = serde_json::json!({"a": {"$eq": "x"}});
        match Filter::parse(&short).unwrap() {
            Filter::Eq(f, v) => {
                assert_eq!(f, "a");
                assert_eq!(v, serde_json::json!("x"));
            }
            _ => panic!("shorthand should parse to Eq"),
        }
        match Filter::parse(&long).unwrap() {
            Filter::Eq(f, v) => {
                assert_eq!(f, "a");
                assert_eq!(v, serde_json::json!("x"));
            }
            _ => panic!("long form should parse to Eq"),
        }
    }

    #[test]
    fn filter_parses_and_or_nesting() {
        let v = serde_json::json!({
            "$and": [
                {"session_id": {"$eq": "s1"}},
                {"chunk_index": {"$eq": 3}},
            ]
        });
        let f = Filter::parse(&v).unwrap();
        let meta_hit: HashMap<String, Value> = [
            ("session_id".to_string(), serde_json::json!("s1")),
            ("chunk_index".to_string(), serde_json::json!(3)),
        ]
        .into_iter()
        .collect();
        let meta_miss: HashMap<String, Value> = [
            ("session_id".to_string(), serde_json::json!("s1")),
            ("chunk_index".to_string(), serde_json::json!(7)),
        ]
        .into_iter()
        .collect();
        assert!(f.matches(&meta_hit));
        assert!(!f.matches(&meta_miss));
    }

    #[test]
    fn filter_in_matches_any_element() {
        let v = serde_json::json!({"topic": {"$in": ["a", "b"]}});
        let f = Filter::parse(&v).unwrap();
        let mut meta = HashMap::new();
        meta.insert("topic".into(), serde_json::json!("b"));
        assert!(f.matches(&meta));
        meta.insert("topic".into(), serde_json::json!("c"));
        assert!(!f.matches(&meta));
    }

    #[test]
    fn filter_rejects_unknown_operator() {
        let v = serde_json::json!({"a": {"$gte": 3}});
        assert!(Filter::parse(&v).is_err());
    }

    #[test]
    fn blob_roundtrip_preserves_floats() {
        let v: Vec<f32> = (0..EMBED_DIM).map(|i| i as f32 * 0.001).collect();
        let b = vec_to_blob(&v);
        let w = blob_to_vec(&b);
        assert_eq!(v, w);
    }

    #[test]
    fn cosine_distance_handles_degenerate_inputs() {
        let zero = vec![0.0_f32; 4];
        let nonzero = vec![1.0_f32, 0.0, 0.0, 0.0];
        assert_eq!(cosine_distance(&zero, &nonzero), 1.0);
        assert_eq!(cosine_distance(&[], &[]), 1.0);
    }

    #[test]
    fn cosine_distance_identity_is_zero() {
        let v = vec![0.5_f32, 0.3, -0.2, 0.8];
        let d = cosine_distance(&v, &v);
        assert!(d.abs() < 1e-5, "identity distance should be ~0, got {d}");
    }
}
