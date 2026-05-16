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
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use hoangsa_memory_core::{Error, Result};
use parking_lot::Mutex as PlMutex;
use rusqlite::{params, Connection};
use serde_json::Value;
use tokio::sync::Mutex as TokioMutex;
use tokio::sync::RwLock as TokioRwLock;

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

/// Derive an install root from a binary path. Returns `Some` only when
/// the binary lives in an installed layout `<root>/bin/<name>` — i.e.
/// the immediate parent is literally named `bin`. This guard prevents
/// `cargo run` (binary at `target/debug/hoangsa-memory`) from
/// accidentally reporting `target/debug` as the install root and
/// writing the 118 MB fastembed cache into the build artefacts tree.
fn derive_install_root_from_exe(exe: &Path) -> Option<PathBuf> {
    let parent = exe.parent()?;
    if parent.file_name()?.to_str()? != "bin" {
        return None;
    }
    parent.parent().map(Path::to_path_buf)
}

/// Resolve the directory fastembed should use to cache ONNX weights.
///
/// Resolution order:
///   1. `FASTEMBED_CACHE_DIR` — explicit user override, honored verbatim.
///   2. `HOANGSA_INSTALL_DIR/cache/fastembed` — explicit install-dir
///      override. Users with multi-profile Claude setups bake this into
///      the alias that launches Claude (NOT into `.zshrc`), so per-profile
///      caches stay separate. The installer scripts deliberately do not
///      persist this to rc to avoid a single global value colliding across
///      profiles.
///   3. Derive from `current_exe()` — canonicalize to resolve PATH
///      shim symlinks, then accept only when the parent is literally
///      `bin` (see `derive_install_root_from_exe`). Works in fresh
///      shells without any env.
///   4. `$HOME/.hoangsa/cache/fastembed` — default for dev runs
///      (`cargo run`) and when `current_exe` fails.
///   5. `./.fastembed_cache` — last-resort, matches fastembed's own
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
    if let Ok(exe) = std::env::current_exe() {
        let resolved = std::fs::canonicalize(&exe).unwrap_or(exe);
        if let Some(root) = derive_install_root_from_exe(&resolved) {
            return root.join("cache").join("fastembed");
        }
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

/// Shareable handle to a single fastembed `TextEmbedding`.
///
/// Phase 4 of the project-isolation refactor (see
/// `.hoangsa/sessions/docs/memory-daemon-refactor/NOTES.md`) hoists the
/// embedder out of per-project [`EmbeddedVectorStore`] so the multi-project
/// MCP daemon allocates the ~150 MB ONNX model once and shares it across
/// every project's vector store.
///
/// The underlying `TextEmbedding` is initialised lazily on the first
/// [`SharedEmbedder::embed`] call; constructing a `SharedEmbedder` is
/// cheap and never blocks.
///
/// The embedder also supports **idle eviction** — after a configurable
/// idle window the `TextEmbedding` is dropped, releasing the ORT session
/// (and its CPU memory arena, which would otherwise ratchet up with the
/// largest tensor ever embedded). The next [`Self::embed`] call pays the
/// 5–30 s re-init cost. This is the one fix that actually returns memory
/// to the OS — disabling the arena from outside fastembed isn't possible
/// without a fork.
pub struct SharedEmbedder {
    /// `None` when the model isn't currently loaded (cold or evicted),
    /// `Some(Arc<...>)` when it is. The inner `Arc` lets in-flight embeds
    /// keep working through an eviction — the writer just drops the slot;
    /// the embedder is only freed once the last in-flight task releases
    /// its clone.
    state: TokioRwLock<Option<Arc<TokioMutex<TextEmbedding>>>>,
    /// Dedupe concurrent first-init / re-init. Held only across the
    /// `try_new` call.
    init_lock: TokioMutex<()>,
    /// Unix seconds of last successful [`Self::embed`] dispatch. Updated
    /// without locking via [`Ordering::Relaxed`] — the eviction loop
    /// reads it the same way and a one-tick lag is harmless.
    last_access_unix: AtomicI64,
    /// Unix seconds when the current `TextEmbedding` was constructed.
    /// `0` when the slot is empty. Drives the max-age forced eviction
    /// that fires even under continuous load — without it, a workload
    /// that never goes idle keeps ratcheting the ORT arena upward.
    loaded_at_unix: AtomicI64,
}

/// Cap on the per-inference batch fastembed runs internally. fastembed's
/// own default is 256, which lets the caller hand in one giant `Vec` and
/// have ORT process it in a single forward pass — fast, but the ORT CPU
/// arena sizes itself to the biggest tensor ever seen, so one large
/// embedder call permanently ratchets RSS up. Pinning to 32 means
/// individual ORT runs work on at most `32 × max_seq_len × hidden_dim`
/// tensors regardless of how big the caller's `texts` vector is, which
/// keeps the arena bounded under sustained traffic. Throughput cost is
/// marginal — fastembed loops the batches internally either way.
const EMBED_BATCH_SIZE: usize = 32;

impl SharedEmbedder {
    /// Build a fresh handle. The model isn't loaded until the first
    /// [`Self::embed`] call, so this is cheap enough to call from
    /// startup paths even when embeddings are gated behind config.
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            state: TokioRwLock::new(None),
            init_lock: TokioMutex::new(()),
            last_access_unix: AtomicI64::new(0),
            loaded_at_unix: AtomicI64::new(0),
        })
    }

    /// Embed a batch of texts. The first call (and the first call after
    /// idle eviction) runs the fastembed initialisation (~1–3 s warm /
    /// ~5–30 s cold); concurrent first-callers converge on the same
    /// init via `init_lock`. Subsequent calls share the cached
    /// `TextEmbedding`.
    pub async fn embed(self: Arc<Self>, texts: Vec<String>) -> Result<Vec<Vec<f32>>> {
        let model = self.get_or_init().await?;
        self.last_access_unix.store(now_unix(), Ordering::Relaxed);
        tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
            let mut guard = model.blocking_lock();
            guard
                .embed(texts, Some(EMBED_BATCH_SIZE))
                .map_err(|e| Error::Store(format!("embed: {e}")))
        })
        .await
        .map_err(|e| Error::Store(format!("embed join: {e}")))?
    }

    /// Get the loaded model handle, initialising it under `init_lock`
    /// if necessary. Returns a clone of the `Arc<TokioMutex<...>>` so
    /// callers can finish their embed even if a concurrent eviction
    /// clears the state slot mid-flight.
    async fn get_or_init(&self) -> Result<Arc<TokioMutex<TextEmbedding>>> {
        if let Some(m) = self.state.read().await.as_ref() {
            return Ok(m.clone());
        }
        let _init = self.init_lock.lock().await;
        // Double-check: another waiter may have populated while we
        // queued on `init_lock`.
        if let Some(m) = self.state.read().await.as_ref() {
            return Ok(m.clone());
        }
        let model = tokio::task::spawn_blocking(|| TextEmbedding::try_new(default_init_options()))
            .await
            .map_err(|e| Error::Store(format!("embedder init join: {e}")))?
            .map_err(|e| Error::Store(format!("embedder init: {e}")))?;
        let arc = Arc::new(TokioMutex::new(model));
        *self.state.write().await = Some(arc.clone());
        self.loaded_at_unix.store(now_unix(), Ordering::Relaxed);
        Ok(arc)
    }

    /// Drop the loaded `TextEmbedding` if it has been idle for at least
    /// `idle`. Returns `true` if a model was actually evicted. In-flight
    /// embeds that already cloned the `Arc` keep their handle; the model
    /// is only truly freed once the last reference dies.
    ///
    /// Reading `last_access_unix` without the state lock is intentional:
    /// the only failure mode is "evict raced with embed update" which
    /// the eviction loop will catch on the next scan, and the embed
    /// itself is already in flight on a cloned `Arc`.
    pub async fn evict_if_idle(&self, idle: Duration) -> bool {
        let last = self.last_access_unix.load(Ordering::Relaxed);
        let cutoff = now_unix().saturating_sub(idle.as_secs() as i64);
        if last > cutoff {
            return false;
        }
        self.drop_slot().await
    }

    /// Forced eviction once the model has been loaded continuously for
    /// `max_age`, regardless of how recently it was used. Returns `true`
    /// if a model was actually evicted.
    ///
    /// This is the safety net for "user works continuously and never
    /// goes idle for 60 s" — the ORT CPU arena ratchets up with the
    /// largest tensor ever embedded, so even under sustained load RSS
    /// climbs monotonically until something drops the embedder. A 30-min
    /// forced reset produces a sawtooth where RSS resets every half
    /// hour and the user pays ~1–3 s of re-init from disk cache.
    pub async fn evict_if_stale(&self, max_age: Duration) -> bool {
        let loaded = self.loaded_at_unix.load(Ordering::Relaxed);
        if loaded == 0 {
            return false; // nothing is loaded
        }
        let cutoff = now_unix().saturating_sub(max_age.as_secs() as i64);
        if loaded > cutoff {
            return false;
        }
        self.drop_slot().await
    }

    /// Shared body of [`Self::evict_if_idle`] / [`Self::evict_if_stale`].
    /// Drops the state slot if present and resets `loaded_at_unix`.
    async fn drop_slot(&self) -> bool {
        let mut g = self.state.write().await;
        if g.is_none() {
            return false;
        }
        *g = None;
        self.loaded_at_unix.store(0, Ordering::Relaxed);
        true
    }

    /// Test/diagnostic accessor. `true` when the model is currently
    /// loaded into memory.
    pub async fn is_loaded(&self) -> bool {
        self.state.read().await.is_some()
    }
}

fn now_unix() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

/// The concrete [`VectorStore`] backed by fastembed + SQLite.
///
/// Cheap to clone — only the `Arc<StoreInner>` is bumped, the underlying
/// SQLite connection and embedder are shared. The multi-project MCP
/// daemon relies on this so it can hand the same store to the bundle
/// (for eviction) and to long-running tools without an extra wrapping
/// `Arc`.
#[derive(Clone)]
pub struct EmbeddedVectorStore {
    inner: Arc<StoreInner>,
}

struct StoreInner {
    /// SQLite connection, guarded by a blocking mutex because
    /// `rusqlite::Connection: !Sync`. Writes are short and rare
    /// relative to embed time, so contention is negligible.
    db: PlMutex<Connection>,
    /// Shared fastembed handle. Multiple stores in the same process
    /// (e.g. the multi-project MCP daemon) hold clones of one Arc so
    /// the underlying `TextEmbedding` is allocated once.
    embedder: Arc<SharedEmbedder>,
}

impl EmbeddedVectorStore {
    /// Open or create the vectors SQLite at `data_path`, allocating a
    /// fresh [`SharedEmbedder`] for it. Used by one-shot CLI paths
    /// where there is no longer-lived service to share an embedder
    /// with; the daemon path uses [`Self::open_with_embedder`].
    pub async fn open(data_path: &Path) -> Result<Self> {
        Self::open_with_embedder(data_path, SharedEmbedder::new()).await
    }

    /// Open or create the vectors SQLite at `data_path`, sharing the
    /// supplied `embedder` instead of building a new one. The multi-
    /// project MCP daemon uses this so the ~150 MB ONNX model is
    /// allocated once across every project it serves.
    pub async fn open_with_embedder(
        data_path: &Path,
        embedder: Arc<SharedEmbedder>,
    ) -> Result<Self> {
        if let Some(parent) = data_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| Error::Store(format!("create vector dir: {e}")))?;
        }

        let db_path = data_path.to_path_buf();
        let db = tokio::task::spawn_blocking(move || open_sqlite(&db_path))
            .await
            .map_err(|e| Error::Store(format!("vector sqlite join: {e}")))??;

        Ok(Self {
            inner: Arc::new(StoreInner {
                db: PlMutex::new(db),
                embedder,
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
    inner.embedder.clone().embed(texts).await
}

fn open_sqlite(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .map_err(|e| Error::Store(format!("open vectors.sqlite: {e}")))?;
    // `cache_size = -20000` caps the per-connection page cache at 20 MB
    // (negative = KiB). Default `-2000` (2 MB) is fine but undocumented;
    // setting it explicitly means RSS per project sqlite stays predictable
    // even when the multi-project daemon has many concurrent connections.
    // `temp_store = MEMORY` keeps small intermediates out of /tmp.
    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA cache_size = -20000;
         PRAGMA temp_store = MEMORY;
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
    fn derive_install_root_accepts_standard_bin_layout() {
        let exe = PathBuf::from("/opt/hoangsa/bin/hoangsa-memory");
        assert_eq!(
            derive_install_root_from_exe(&exe),
            Some(PathBuf::from("/opt/hoangsa"))
        );
    }

    #[test]
    fn derive_install_root_rejects_cargo_target_layout() {
        let exe = PathBuf::from("/workspace/target/debug/hoangsa-memory");
        assert_eq!(derive_install_root_from_exe(&exe), None);
    }

    #[test]
    fn derive_install_root_rejects_wrong_parent_name() {
        let exe = PathBuf::from("/home/u/scripts/hoangsa-memory");
        assert_eq!(derive_install_root_from_exe(&exe), None);
    }

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

    #[tokio::test]
    async fn shared_embedder_is_held_by_every_store() {
        // Two stores constructed with the same Arc<SharedEmbedder> must hold
        // clones of the same Arc — that's the load-bearing property Phase 4
        // relies on (one ONNX model across N projects). We don't trigger an
        // actual embed here so the test stays offline-friendly; the lazy
        // state slot inside SharedEmbedder means construction never touches
        // fastembed.
        use tempfile::tempdir;
        let dir1 = tempdir().unwrap();
        let dir2 = tempdir().unwrap();
        let p1 = dir1.path().join("vectors.sqlite");
        let p2 = dir2.path().join("vectors.sqlite");

        let embedder = SharedEmbedder::new();
        let pre = Arc::strong_count(&embedder);

        let _s1 = EmbeddedVectorStore::open_with_embedder(&p1, embedder.clone())
            .await
            .unwrap();
        let _s2 = EmbeddedVectorStore::open_with_embedder(&p2, embedder.clone())
            .await
            .unwrap();

        assert!(
            Arc::strong_count(&embedder) >= pre + 2,
            "both stores must retain a clone of the same Arc<SharedEmbedder>; \
             pre={pre} now={}",
            Arc::strong_count(&embedder),
        );
    }

    #[tokio::test]
    async fn evict_if_idle_is_noop_when_unloaded() {
        // A SharedEmbedder that never embedded must report eviction as a
        // no-op — there's nothing to drop. This is the cold-daemon /
        // gated-embedding path.
        let embedder = SharedEmbedder::new();
        assert!(!embedder.is_loaded().await);
        let evicted = embedder.evict_if_idle(Duration::from_secs(0)).await;
        assert!(!evicted, "unloaded embedder must not report eviction");
        assert!(!embedder.is_loaded().await);
    }

    #[tokio::test]
    async fn evict_if_idle_skips_when_recently_active() {
        // Manually wedge a fake last-access into the future to simulate a
        // very recent embed (we can't actually call `embed` in unit tests
        // without downloading the ONNX model). The slot itself is None,
        // so evict still returns false — but the path that matters is
        // the timestamp check above the state check.
        let embedder = SharedEmbedder::new();
        embedder.last_access_unix.store(
            now_unix() + 3600, // 1 h in the future
            Ordering::Relaxed,
        );
        let evicted = embedder.evict_if_idle(Duration::from_secs(60)).await;
        assert!(
            !evicted,
            "recent activity must keep the embedder loaded; \
             evict_if_idle returned true unexpectedly",
        );
    }

    #[tokio::test]
    async fn evict_if_stale_is_noop_when_never_loaded() {
        // `loaded_at_unix == 0` means the model has never been initialised.
        // Stale eviction must not fire — there's nothing to drop and we
        // don't want a forced re-init for a model that hasn't loaded yet.
        let embedder = SharedEmbedder::new();
        assert_eq!(embedder.loaded_at_unix.load(Ordering::Relaxed), 0);
        let evicted = embedder.evict_if_stale(Duration::from_secs(0)).await;
        assert!(!evicted, "stale eviction must skip never-loaded embedders");
    }

    #[tokio::test]
    async fn evict_if_stale_skips_when_loaded_recently() {
        // Pretend the model loaded 5 s ago. A 30-min max-age check should
        // be a no-op. Like the idle test, the slot itself is None so we're
        // really exercising the timestamp gate.
        let embedder = SharedEmbedder::new();
        embedder.loaded_at_unix.store(
            now_unix() - 5, // 5 s ago
            Ordering::Relaxed,
        );
        let evicted = embedder
            .evict_if_stale(Duration::from_secs(30 * 60))
            .await;
        assert!(
            !evicted,
            "stale eviction must wait for max_age to elapse; \
             evict_if_stale returned true unexpectedly",
        );
    }
}
