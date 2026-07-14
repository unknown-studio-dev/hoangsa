//! MCP server core: request dispatch and tool implementations.
//!
//! The transport layer (stdio + Unix socket) lives in [`run_stdio`] /
//! [`run_socket`] in the `transport` submodule; the rest is pure logic
//! driven by a [`Server`] handle.

mod archive_tools;
mod catalog;
mod dispatch;
mod graph_tools;
mod memory_tools;
mod recall_tools;
mod transport;
mod watcher;

pub use transport::{run_socket, run_stdio, socket_path};
pub(crate) use transport::handle_socket_conn;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use serde_json::json;
use hoangsa_memory_parse::LanguageRegistry;
use hoangsa_memory_retrieve::{
    Indexer, RetrieveConfig, Retriever, VectorStoreConfig, WatchConfig,
};
use hoangsa_memory_store::{
    EmbeddedVectorStore, SharedEmbedder, StoreRoot, VectorCol, VectorStore,
};
use time::OffsetDateTime;
use tracing::warn;

// ===========================================================================
// Server
// ===========================================================================

/// MCP server handle. Cheap to clone — all backing state is behind `Arc`.
#[derive(Clone)]
pub struct Server {
    pub(crate) inner: Arc<Inner>,
}

/// Heavy per-project resources that get evicted after idle. Phase 5 of the
/// project-isolation work: in the multi-project daemon a project that hasn't
/// served a request in 30 minutes drops its tantivy reader, sqlite pool, and
/// redb handle to claw back ~10-50 MB RAM; the next request rehydrates the
/// bundle transparently via [`Server::resources`].
pub(crate) struct ResourceBundle {
    pub(crate) store: StoreRoot,
    pub(crate) indexer: Indexer,
    pub(crate) retriever: Retriever,
    pub(crate) graph: hoangsa_memory_graph::Graph,
}

impl ResourceBundle {
    async fn open(root: &Path) -> anyhow::Result<Self> {
        let store = StoreRoot::open(root).await?;
        let retrieve_cfg = RetrieveConfig::load_or_default(root).await;
        let indexer = Indexer::new(store.clone(), LanguageRegistry::new());
        let retriever =
            Retriever::new(store.clone()).with_markdown_boost(retrieve_cfg.rerank_markdown_boost);
        let graph = hoangsa_memory_graph::Graph::new(store.kv.clone());
        Ok(Self {
            store,
            indexer,
            retriever,
            graph,
        })
    }
}

pub(crate) struct Inner {
    pub(crate) root: PathBuf,
    /// Heavy backends (tantivy / redb / episodes sqlite). Lazily
    /// (re-)opened by [`Server::resources`]; dropped by
    /// [`Server::evict_resources`] when the project has been idle long
    /// enough to be worth the rehydrate cost on the next request.
    pub(crate) bundle: tokio::sync::RwLock<Option<Arc<ResourceBundle>>>,
    /// Unix-seconds timestamp of the last [`Server::resources`] call.
    /// Used by the daemon's eviction loop to decide when a project is
    /// "idle enough" to drop its bundle. Read/written via `Relaxed` —
    /// the eviction loop tolerates a few seconds of skew.
    last_access: AtomicI64,
    /// Lazy handle to the in-process vector store. Holds the SQLite
    /// connection (page cache + prepared-statement cache, ~hundreds of
    /// KB per project) plus a clone of `vector_store_embedder` (cheap —
    /// the embedder loads lazily on first use).
    ///
    /// Cleared alongside the bundle in [`Server::evict_resources`]: an
    /// idle project must not keep its SQLite handle resident, so the
    /// slot needs to be takeable. `None` = uninit or evicted, `Some(_)`
    /// = warm. Init failures aren't sticky — the next call retries, so
    /// a transient cause (filesystem hiccup) can clear on its own.
    vector_store: tokio::sync::RwLock<Option<EmbeddedVectorStore>>,
    /// Mirror of `[vector_store] enabled` at server-open time. When
    /// false, `get_vector_store` short-circuits without even trying to
    /// init — useful on machines where fastembed's model download
    /// would time out.
    vector_store_enabled: bool,
    /// Shared fastembed handle. In `Server::open` this is a fresh
    /// instance unique to this server; in
    /// `Server::open_with_embedder` (used by the multi-project MCP
    /// daemon) every per-project Server holds a clone of one Arc so
    /// the ~150 MB ONNX model is allocated once across all projects.
    vector_store_embedder: Arc<SharedEmbedder>,
    /// Serialises `memory_index` calls against each other. Two
    /// concurrent tool_index invocations on the same store would
    /// double-parse and double-embed identical chunks (idempotent via
    /// deterministic chunk ids, but wasteful). With this lock held, the
    /// second call waits; when it runs most files are cache hits and
    /// it finishes quickly. Doesn't block `memory_recall` or other
    /// read-side tools.
    index_mutex: tokio::sync::Mutex<()>,
    /// Abort handle for the per-project background file watcher.
    /// Populated by [`Server::spawn_watcher`] on success; consumed by
    /// [`Server::abort_watcher`] when the project is unregistered so
    /// the watcher's `Arc<Inner>` clone goes away and the bundle can
    /// drop. A `std::sync::Mutex` is fine — the handle is touched only
    /// at start/stop and the work is never `.await`-suspended.
    watcher: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Server {
    /// Open a server rooted at `path` (the `.hoangsa/memory/` directory).
    ///
    /// The fastembed ONNX model is **not** loaded here — it is lazily
    /// initialized on first use to avoid the ~130 MB RSS hit when no
    /// vector operation is needed.
    pub async fn open(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        Self::open_with_embedder(path, SharedEmbedder::new()).await
    }

    /// Like [`Self::open`] but reuses an externally-owned shared
    /// embedder. The multi-project MCP daemon (`ServiceState`) holds
    /// one [`SharedEmbedder`] for the lifetime of the process and
    /// passes a clone into every per-project Server it opens, so the
    /// ONNX model is allocated once across N projects instead of N
    /// times.
    pub async fn open_with_embedder(
        path: impl AsRef<Path>,
        embedder: Arc<SharedEmbedder>,
    ) -> anyhow::Result<Self> {
        let root = path.as_ref().to_path_buf();
        let bundle = ResourceBundle::open(&root).await?;
        let vector_store_enabled = Self::is_vector_store_enabled(&root).await;

        Ok(Self {
            inner: Arc::new(Inner {
                root,
                bundle: tokio::sync::RwLock::new(Some(Arc::new(bundle))),
                last_access: AtomicI64::new(now_unix()),
                vector_store: tokio::sync::RwLock::new(None),
                vector_store_enabled,
                vector_store_embedder: embedder,
                index_mutex: tokio::sync::Mutex::new(()),
                watcher: std::sync::Mutex::new(None),
            }),
        })
    }

    /// Read-only accessor for the shared embedder. Used by the
    /// multi-project daemon's tests to assert that every per-project
    /// Server holds the same Arc.
    #[doc(hidden)]
    pub fn shared_embedder(&self) -> &Arc<SharedEmbedder> {
        &self.inner.vector_store_embedder
    }

    /// Borrow (rehydrating if needed) the heavy backend bundle.
    ///
    /// In the multi-project daemon the bundle may have been dropped by
    /// [`Self::evict_resources`] after a long idle window — the first
    /// call after eviction reopens tantivy/redb/episodes (≈100-200 ms).
    /// In single-project mode the bundle is built eagerly at
    /// [`Self::open_with_embedder`] time, so this is just a refcount
    /// bump on the cached `Arc`.
    ///
    /// Every call refreshes `last_access` so the eviction loop's
    /// idleness check is grounded in actual tool traffic.
    pub(crate) async fn resources(&self) -> anyhow::Result<Arc<ResourceBundle>> {
        self.inner
            .last_access
            .store(now_unix(), Ordering::Relaxed);
        if let Some(b) = self.inner.bundle.read().await.as_ref() {
            return Ok(b.clone());
        }
        // Slow path: take the write lock and double-check — a concurrent
        // caller may have populated the slot between our read and write.
        let mut w = self.inner.bundle.write().await;
        if let Some(b) = w.as_ref() {
            return Ok(b.clone());
        }
        let bundle = Arc::new(ResourceBundle::open(&self.inner.root).await?);
        tracing::info!(
            root = %self.inner.root.display(),
            "project resources rehydrated after idle eviction"
        );
        *w = Some(bundle.clone());
        Ok(bundle)
    }

    /// Borrow the bundle **only if it is currently warm** — never
    /// rehydrate. Used by background tasks that should not defeat
    /// Phase-5 eviction by triggering an expensive reopen on their own
    /// schedule (the file watcher is the canonical case: fs activity
    /// keeps firing even when no user is actively touching the project,
    /// and using [`Self::resources`] there would rebuild tantivy + redb
    /// + episodes every few seconds).
    ///
    /// Does **not** refresh `last_access` — the caller's traffic is by
    /// definition not user-driven.
    pub(crate) async fn resources_if_warm(&self) -> Option<Arc<ResourceBundle>> {
        self.inner.bundle.read().await.as_ref().cloned()
    }

    /// Drop the heavy backend bundle **and** the lazily-opened vector
    /// store. The cached `Arc<Server>` and the shared embedder Arc stay
    /// live; the next [`Self::resources`] / [`Self::get_vector_store`]
    /// call rebuilds them.
    ///
    /// Returns `true` if anything was actually dropped, `false` if both
    /// slots were already empty.
    pub async fn evict_resources(&self) -> bool {
        let mut bundle = self.inner.bundle.write().await;
        let mut vector = self.inner.vector_store.write().await;
        // Bitwise `|` (not `||`) so we always take both slots — `||`
        // would short-circuit and leak the vector store when the
        // bundle was the first to clear.
        let dropped = bundle.take().is_some() | vector.take().is_some();
        if dropped {
            tracing::info!(
                root = %self.inner.root.display(),
                "project resources evicted (idle)"
            );
        }
        dropped
    }

    /// Unix-seconds timestamp of the most recent [`Self::resources`]
    /// call. Used by the daemon eviction loop.
    pub fn last_access_unix(&self) -> i64 {
        self.inner.last_access.load(Ordering::Relaxed)
    }

    /// True when [`Self::resources_if_warm`] would return `Some`. For
    /// tests asserting eviction behaviour without poking the private
    /// slot.
    #[doc(hidden)]
    pub async fn bundle_is_warm(&self) -> bool {
        self.inner.bundle.read().await.is_some()
    }

    /// True when the lazy vector store has been opened and not yet
    /// evicted. Test-only accessor.
    #[doc(hidden)]
    pub async fn vector_store_is_warm(&self) -> bool {
        self.inner.vector_store.read().await.is_some()
    }

    async fn is_vector_store_enabled(root: &Path) -> bool {
        VectorStoreConfig::load_or_default(root)
            .await
            .is_effectively_enabled()
    }

    pub(crate) async fn get_vector_store(&self) -> Option<EmbeddedVectorStore> {
        if !self.inner.vector_store_enabled {
            return None;
        }
        // Fast path: already warm.
        if let Some(s) = self.inner.vector_store.read().await.as_ref() {
            return Some(s.clone());
        }
        // Slow path: take the write lock and double-check — a concurrent
        // caller may have populated the slot between our read and write.
        let mut w = self.inner.vector_store.write().await;
        if let Some(s) = w.as_ref() {
            return Some(s.clone());
        }
        let cfg = VectorStoreConfig::load_or_default(&self.inner.root).await;
        let path = cfg
            .data_path
            .map(PathBuf::from)
            .unwrap_or_else(|| StoreRoot::vectors_path(&self.inner.root));
        let embedder = self.inner.vector_store_embedder.clone();
        match EmbeddedVectorStore::open_with_embedder(&path, embedder).await {
            Ok(s) => {
                tracing::info!(path = %path.display(), "embedded vector store opened (lazy init)");
                *w = Some(s.clone());
                Some(s)
            }
            Err(e) => {
                tracing::warn!(error = %e, "embedded vector store init failed");
                None
            }
        }
    }

    /// Spawn a background file watcher if `[watch] enabled = true` in
    /// `config.toml`. The watcher reuses the server's `Indexer` so there
    /// is no lock contention with the MCP daemon. Returns `true` if a
    /// watcher was spawned.
    ///
    /// `src` is the source tree to watch (typically the project root,
    /// i.e. the parent of `.hoangsa/memory/`).
    pub async fn spawn_watcher(&self, src: PathBuf) -> bool {
        let cfg = WatchConfig::load_or_default(&self.inner.root).await;
        if !cfg.enabled {
            return false;
        }
        let debounce = std::time::Duration::from_millis(cfg.debounce_ms);
        let server = self.clone();
        let handle = tokio::spawn(async move {
            if let Err(e) = watcher::run_watcher(server, src, debounce).await {
                warn!(error = %e, "background watcher exited");
            }
        });
        let mut slot = self
            .inner
            .watcher
            .lock()
            .expect("watcher handle slot poisoned");
        // Replace any previous handle (caller spawning a fresh watcher
        // for the same project means the previous one is conceptually
        // dead — abort it so we don't leak two clones of `Server`).
        if let Some(prev) = slot.replace(handle) {
            prev.abort();
        }
        true
    }

    /// Abort the background watcher task spawned by
    /// [`Self::spawn_watcher`], if one is running. The watcher holds a
    /// clone of this `Server`, so aborting it lets the project's `Arc`
    /// graph drop when [`ServiceState::unregister`] removes the slot.
    pub fn abort_watcher(&self) {
        let mut slot = self
            .inner
            .watcher
            .lock()
            .expect("watcher handle slot poisoned");
        if let Some(handle) = slot.take() {
            handle.abort();
        }
    }

    async fn upsert_memory_vector(&self, kind: &str, text: &str, tags: &[String]) {
        let col = match self.open_memory_vector().await {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(error = %e, "vector memory upsert skipped (store unavailable)");
                return;
            }
        };
        let id = format!("{kind}:{}", blake3::hash(text.as_bytes()).to_hex());
        let mut meta = std::collections::HashMap::new();
        meta.insert("kind".to_string(), json!(kind));
        if !tags.is_empty() {
            meta.insert("tags".to_string(), json!(tags.join(",")));
        }
        if let Err(e) = col
            .upsert(vec![id], Some(vec![text.to_string()]), Some(vec![meta]))
            .await
        {
            tracing::debug!(error = %e, "vector memory upsert failed");
        }
    }

    async fn open_memory_vector(&self) -> anyhow::Result<Arc<dyn VectorCol>> {
        let vs = self
            .get_vector_store()
            .await
            .ok_or_else(|| anyhow::anyhow!("vector store not configured"))?;
        let (col, _info) = vs.ensure_collection("hoangsa_memory_policy").await?;
        Ok(col)
    }

    /// Code-chunk collection used by the indexer to embed source chunks.
    /// Mirrors the CLI's `open_vector_store` helper so MCP-driven
    /// indexing produces embeddings instead of silently skipping the
    /// vector stage.
    async fn open_code_vector(&self) -> Option<Arc<dyn VectorCol>> {
        let vs = self.get_vector_store().await?;
        match vs.ensure_collection("hoangsa_memory_code").await {
            Ok((col, _info)) => Some(col),
            Err(e) => {
                // Store opened but the collection handshake failed — this
                // means embeddings will be skipped for this index run. Emit
                // a warning so operators can debug instead of staring at a
                // stats line that shows `embedded: 0` with no explanation.
                tracing::warn!(error = %e, "vector: ensure_collection(hoangsa_memory_code) failed — embeddings disabled for this run");
                None
            }
        }
    }

    async fn open_archive_vector(&self) -> anyhow::Result<Arc<dyn VectorCol>> {
        let vs = self
            .get_vector_store()
            .await
            .ok_or_else(|| anyhow::anyhow!("vector store not configured"))?;
        let (col, _info) = vs.ensure_collection("hoangsa_memory_archive").await?;
        Ok(col)
    }
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}
