//! # thoth
//!
//! Long-term memory for coding agents — public library surface.
//!
//! This crate is a thin umbrella on top of the workspace: it re-exports the
//! types from [`thoth_core`] and wires the individual stores ([`thoth_store`]),
//! the parser ([`thoth_parse`]), the graph ([`thoth_graph`]), the retrieval
//! orchestrator ([`thoth_retrieve`]), and the memory lifecycle
//! ([`thoth_memory`]) behind a single [`CodeMemory`] façade.
//!
//! Everything here is real — no stubs, no unimplemented branches. Callers
//! who want finer control can bypass the façade and use the sub-crates
//! directly; the types travel across boundaries unchanged.
//!
//! ```no_run
//! # async fn demo() -> thoth::Result<()> {
//! use thoth::{CodeMemory, Mode, Query};
//!
//! let mem = CodeMemory::open(".thoth").await?;
//! mem.index(".").await?;
//!
//! // Mode::Zero — no external calls.
//! let r = mem
//!     .recall(Query::text("where is auth handled"), Mode::Zero)
//!     .await?;
//! # let _ = r;
//! # Ok(()) }
//! ```

#![deny(rust_2018_idioms)]
#![warn(missing_docs)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

// ------------------------------------------------------- re-exports (public)

pub use thoth_core::{
    Chunk, Citation, Error, Event, EventId, Fact, Lesson, MemoryKind, MemoryMeta, Mode, Outcome,
    Prompt, Query, QueryScope, Result, Retrieval, RetrievalSource, Skill, Synthesis, Synthesizer,
};

pub use thoth_graph;
pub use thoth_memory::{MemoryConfig, MemoryManager, NudgeReport, WorkingMemory, WorkingNote};
pub use thoth_parse::LanguageRegistry;
pub use thoth_retrieve::{IndexProgress, IndexStats, Indexer, RetrieveConfig, Retriever};
pub use thoth_store::{ChromaStore, StoreRoot};

// --------------------------------------------------------------- CodeMemory

/// Top-level Thoth façade.
///
/// A `CodeMemory` is opened against a `.thoth/` directory that holds the
/// on-disk memory (markdown files + indexes). Construction initialises
/// every backend eagerly — once `open` returns, every subsequent call is
/// guaranteed to hit real storage.
///
/// The façade intentionally forwards to the specialised sub-crates instead
/// of reimplementing their logic. Anything the façade can do, you can also
/// do by reaching for the sub-handles directly via [`Self::store`],
/// [`Self::memory`], [`Self::indexer`], and [`Self::retriever`].
pub struct CodeMemory {
    root: PathBuf,
    store: StoreRoot,
    memory: MemoryManager,
    registry: LanguageRegistry,
    working: WorkingMemory,
    retrieve: RetrieveConfig,
}

impl CodeMemory {
    /// Open (or create) a Thoth memory rooted at `path`.
    ///
    /// Side effects:
    /// - creates `path/` if missing,
    /// - migrates any legacy `path/index/` layout to the current flat
    ///   filenames (see [`StoreRoot::open`]),
    /// - opens `MEMORY.md`, `LESSONS.md`, redb KV, tantivy FTS, and the
    ///   episodic SQLite log,
    /// - loads `<path>/config.toml` (via
    ///   [`MemoryConfig::load_or_default`]) so TTL / decay / nudge flags
    ///   take effect immediately,
    /// - seeds empty markdown files so downstream reads never observe a
    ///   half-initialised layout,
    /// - creates an in-process [`WorkingMemory`] scratchpad.
    ///
    /// The vector store is *not* opened here; it is opened lazily by
    /// [`Self::recall`] when `Mode::Full` is requested.
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let root = path.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&root).await?;
        let store = StoreRoot::open(&root).await?;
        // Seed empty source-of-truth files so `memory show` on a fresh repo
        // isn't confused by missing markdown.
        for name in ["MEMORY.md", "LESSONS.md"] {
            let p = store.path.join(name);
            if !p.exists() {
                tokio::fs::write(&p, format!("# {name}\n")).await?;
            }
        }
        let memory = MemoryManager::open_with(&root, store.episodes.clone()).await?;
        let retrieve = RetrieveConfig::load_or_default(&root).await;
        Ok(Self {
            root,
            store,
            memory,
            registry: LanguageRegistry::new(),
            working: WorkingMemory::with_capacity(128),
            retrieve,
        })
    }

    /// Path to the root `.thoth/` directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Borrow the underlying [`StoreRoot`]. Useful for callers who want to
    /// reach individual backends (FTS, KV, markdown, episodic log).
    pub fn store(&self) -> &StoreRoot {
        &self.store
    }

    /// Borrow the memory manager (markdown + episodic TTL + nudge).
    pub fn memory(&self) -> &MemoryManager {
        &self.memory
    }

    /// Borrow the in-process session scratchpad (DESIGN §5 working memory).
    ///
    /// Cloning returns another handle onto the *same* buffer, so feel free
    /// to hand clones to background tasks.
    pub fn working(&self) -> &WorkingMemory {
        &self.working
    }

    /// Build an [`Indexer`] bound to this memory. Callers can chain
    /// [`Indexer::with_embedding`] / [`Indexer::with_progress`] /
    /// [`Indexer::with_concurrency`] before running a walk.
    pub fn indexer(&self) -> Indexer {
        Indexer::new(self.store.clone(), self.registry.clone())
    }

    /// Build a Mode::Zero [`Retriever`] bound to this memory.
    ///
    /// The `[retrieve] rerank_markdown_boost` config knob loaded at
    /// [`Self::open`] is applied to every retriever handed out here.
    pub fn retriever(&self) -> Retriever {
        Retriever::new(self.store.clone()).with_markdown_boost(self.retrieve.rerank_markdown_boost)
    }

    /// Index a source tree. Runs the full pipeline: walk → parse → write
    /// chunks / symbols / edges → commit FTS.
    ///
    /// This is Mode::Zero: no embeddings are written. For semantic indexing
    /// use [`Self::indexer`] and chain `with_embedding(...)`.
    pub async fn index(&self, src: impl AsRef<Path>) -> Result<IndexStats> {
        self.indexer().index_path(src.as_ref()).await
    }

    /// Recall context for a query, under the given [`Mode`].
    ///
    /// In `Mode::Zero` this runs the symbol / BM25 / graph / markdown
    /// fusion. In `Mode::Full` it additionally plugs in the caller-supplied
    /// synthesizer and runs the full hybrid pipeline.
    pub async fn recall(&self, q: Query, mode: Mode) -> Result<Retrieval> {
        match mode {
            Mode::Zero => self.retriever().recall(&q).await,
            Mode::Full { synthesizer } => {
                let synth: Option<Arc<dyn Synthesizer>> = synthesizer.map(Arc::from);
                Retriever::with_full(self.store.clone(), None, synth)
                    .with_markdown_boost(self.retrieve.rerank_markdown_boost)
                    .recall_full(&q)
                    .await
            }
        }
    }

    /// Append an observation to the episodic log. Returns the SQLite
    /// autoincrement row id of the appended row.
    pub async fn record_event(&self, ev: Event) -> Result<i64> {
        self.store.episodes.append(&ev).await
    }

    /// Append a fact to `MEMORY.md`. Convenience shortcut for
    /// `self.store().markdown.append_fact(...)`.
    pub async fn remember_fact(&self, text: impl Into<String>, tags: Vec<String>) -> Result<()> {
        let fact = Fact {
            meta: MemoryMeta::new(MemoryKind::Semantic),
            text: text.into(),
            tags,
            scope: Default::default(),
        };
        self.store.markdown.append_fact(&fact).await
    }

    /// Append a lesson to `LESSONS.md`. Convenience shortcut for
    /// `self.store().markdown.append_lesson(...)`.
    pub async fn remember_lesson(
        &self,
        trigger: impl Into<String>,
        advice: impl Into<String>,
    ) -> Result<()> {
        let lesson = Lesson {
            meta: MemoryMeta::new(MemoryKind::Reflective),
            trigger: trigger.into(),
            advice: advice.into(),
            success_count: 0,
            failure_count: 0,
            enforcement: Default::default(),
            suggested_enforcement: None,
            block_message: None,
        };
        self.store.markdown.append_lesson(&lesson).await
    }

    /// List installed skills under `<root>/skills/`.
    pub async fn skills(&self) -> Result<Vec<Skill>> {
        self.store.markdown.list_skills().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn open_creates_layout_and_seeds_markdown() {
        let dir = tempdir().unwrap();
        let mem = CodeMemory::open(dir.path()).await.unwrap();
        assert!(mem.root().join("MEMORY.md").exists());
        assert!(mem.root().join("LESSONS.md").exists());
        // New DESIGN §7 layout puts index files at the root, not under `index/`.
        assert!(mem.root().join("graph.redb").exists());
        assert!(mem.root().join("episodes.db").exists());
        assert!(mem.root().join("fts.tantivy").exists());
    }

    #[tokio::test]
    async fn working_memory_handle_is_live() {
        let dir = tempdir().unwrap();
        let mem = CodeMemory::open(dir.path()).await.unwrap();
        mem.working().push(WorkingNote::new("recent query")).await;
        assert_eq!(mem.working().len().await, 1);
    }

    #[tokio::test]
    async fn remember_and_read_back() {
        let dir = tempdir().unwrap();
        let mem = CodeMemory::open(dir.path()).await.unwrap();
        mem.remember_fact("uses JWT in cookie", vec!["auth".into()])
            .await
            .unwrap();
        let facts = mem.store().markdown.read_facts().await.unwrap();
        assert!(facts.iter().any(|f| f.text.contains("JWT")));
    }

    #[tokio::test]
    async fn record_event_returns_id() {
        let dir = tempdir().unwrap();
        let mem = CodeMemory::open(dir.path()).await.unwrap();
        let ev = Event::QueryIssued {
            id: uuid::Uuid::new_v4(),
            text: "where is auth".into(),
            at: time::OffsetDateTime::now_utc(),
        };
        let id = mem.record_event(ev).await.unwrap();
        assert!(id > 0);
    }
}
