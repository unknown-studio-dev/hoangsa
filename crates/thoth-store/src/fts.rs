//! Full-text BM25 index backed by [`tantivy`].
//!
//! Indexes the chunks produced by `thoth-parse::parse_file` so that
//! `thoth-retrieve` can do keyword search without needing embeddings.
//!
//! The index lives on disk under `<root>/fts/`. The schema is intentionally
//! small — if we need more fields later (e.g. a code-aware tokenizer), we
//! bump the index generation and rebuild.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{FAST, Field, STORED, STRING, Schema, TEXT, Value};
use tantivy::{Index, IndexReader, IndexWriter, TantivyDocument, doc};
use thoth_core::{Error, Result};

const WRITER_HEAP_BYTES: usize = 64 * 1024 * 1024; // 64 MiB

/// Payload passed to [`FtsIndex::index_chunk`].
#[derive(Debug, Clone)]
pub struct ChunkDoc {
    /// Stable chunk id (used as the dedup key on re-index).
    pub id: String,
    /// Source file path.
    pub path: String,
    /// Optional symbol / FQN.
    pub symbol: Option<String>,
    /// Full body text (indexed, not stored).
    pub body: String,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line (inclusive).
    pub end_line: u32,
    /// Language tag (e.g. `"rust"`).
    pub language: String,
}

/// A single BM25 hit.
#[derive(Debug, Clone)]
pub struct FtsHit {
    /// Chunk id (path + line range).
    pub id: String,
    /// File path.
    pub path: String,
    /// Optional symbol name (FQN).
    pub symbol: Option<String>,
    /// 1-based start line.
    pub start_line: u32,
    /// 1-based end line (inclusive).
    pub end_line: u32,
    /// Language.
    pub language: String,
    /// BM25 score.
    pub score: f32,
}

struct Fields {
    id: Field,
    path: Field,
    symbol: Field,
    body: Field,
    start_line: Field,
    end_line: Field,
    language: Field,
}

/// Tantivy-backed BM25 index.
///
/// Cheap to [`clone`](Clone); internals are shared behind [`Arc`].
#[derive(Clone)]
pub struct FtsIndex {
    index: Index,
    reader: IndexReader,
    writer: Arc<Mutex<IndexWriter>>,
    fields: Arc<Fields>,
    #[allow(dead_code)]
    path: PathBuf,
}

impl FtsIndex {
    /// Open or create an index rooted at `dir`.
    pub async fn open(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        tokio::fs::create_dir_all(&dir).await?;

        let dir2 = dir.clone();
        tokio::task::spawn_blocking(move || -> Result<Self> {
            let schema = build_schema();
            // `get_field` is infallible in practice — `build_schema` right
            // above this line registers every name we look up — but returning
            // an error (instead of panicking) means a future schema drift
            // surfaces as a clean open-time failure the caller can handle,
            // rather than an abort inside `spawn_blocking`.
            let get = |name: &str| -> Result<Field> {
                schema
                    .get_field(name)
                    .map_err(|e| Error::Store(format!("fts schema missing field `{name}`: {e}")))
            };
            let fields = Fields {
                id: get("id")?,
                path: get("path")?,
                symbol: get("symbol")?,
                body: get("body")?,
                start_line: get("start_line")?,
                end_line: get("end_line")?,
                language: get("language")?,
            };

            let mmap = tantivy::directory::MmapDirectory::open(&dir2).map_err(store)?;
            let index = Index::open_or_create(mmap, schema).map_err(store)?;

            let writer: IndexWriter = index.writer(WRITER_HEAP_BYTES).map_err(store)?;
            // `OnCommitWithDelay` batches reloads and leaves a visible lag
            // between `commit()` and the next `search()` — bad for watch /
            // hook flows where we want the post-edit query to see the edit.
            // `Manual` lets us explicitly reload in `commit()` below.
            let reader = index
                .reader_builder()
                .reload_policy(tantivy::ReloadPolicy::Manual)
                .try_into()
                .map_err(store)?;

            Ok(Self {
                index,
                reader,
                writer: Arc::new(Mutex::new(writer)),
                fields: Arc::new(fields),
                path: dir2,
            })
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Index (or re-index) a single chunk.
    ///
    /// Old documents with the same `id` are deleted before the new one is
    /// added, keeping the index consistent with the latest parse.
    pub async fn index_chunk(&self, chunk: ChunkDoc) -> Result<()> {
        let writer = self.writer.clone();
        let fields = self.fields.clone();

        tokio::task::spawn_blocking(move || -> Result<()> {
            let w = writer.lock();
            // Ensure idempotent updates keyed by chunk id.
            let id_term = tantivy::Term::from_field_text(fields.id, &chunk.id);
            w.delete_term(id_term);
            let mut d = doc!(
                fields.id         => chunk.id,
                fields.path       => chunk.path,
                fields.body       => chunk.body,
                fields.start_line => chunk.start_line as u64,
                fields.end_line   => chunk.end_line as u64,
                fields.language   => chunk.language,
            );
            if let Some(sym) = chunk.symbol {
                d.add_text(fields.symbol, &sym);
            }
            w.add_document(d).map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Index many chunks in a single `spawn_blocking` call (no commit).
    pub async fn index_chunks_batch(&self, chunks: Vec<ChunkDoc>) -> Result<()> {
        if chunks.is_empty() {
            return Ok(());
        }
        let writer = self.writer.clone();
        let fields = self.fields.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let w = writer.lock();
            for chunk in chunks {
                let id_term = tantivy::Term::from_field_text(fields.id, &chunk.id);
                w.delete_term(id_term);
                let mut d = doc!(
                    fields.id         => chunk.id,
                    fields.path       => chunk.path,
                    fields.body       => chunk.body,
                    fields.start_line => chunk.start_line as u64,
                    fields.end_line   => chunk.end_line as u64,
                    fields.language   => chunk.language,
                );
                if let Some(sym) = chunk.symbol {
                    d.add_text(fields.symbol, &sym);
                }
                w.add_document(d).map_err(store)?;
            }
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Delete every document whose `path` field matches `path`.
    ///
    /// Keyed on the STRING `path` field so a full file can be purged without
    /// having to enumerate every stale chunk id first. The caller is still
    /// responsible for calling [`FtsIndex::commit`] before the next search.
    pub async fn delete_path(&self, path: &str) -> Result<()> {
        let writer = self.writer.clone();
        let fields = self.fields.clone();
        let path = path.to_string();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let w = writer.lock();
            let term = tantivy::Term::from_field_text(fields.path, &path);
            w.delete_term(term);
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Commit any pending writes so they become searchable.
    ///
    /// Also explicitly reloads the reader, because we configure the index
    /// with [`tantivy::ReloadPolicy::Manual`] — the default `OnCommitWithDelay`
    /// left a window where a query fired right after a per-file reindex
    /// would still see the pre-commit segments.
    pub async fn commit(&self) -> Result<()> {
        let writer = self.writer.clone();
        let reader = self.reader.clone();
        tokio::task::spawn_blocking(move || -> Result<()> {
            let mut w = writer.lock();
            w.commit().map_err(store)?;
            drop(w);
            reader.reload().map_err(store)?;
            Ok(())
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }

    /// Run a BM25 query across `body` + `symbol`; return the top `k` hits.
    pub async fn search(&self, query: impl Into<String>, k: usize) -> Result<Vec<FtsHit>> {
        let reader = self.reader.clone();
        let index = self.index.clone();
        let fields = self.fields.clone();
        let q = query.into();

        tokio::task::spawn_blocking(move || -> Result<Vec<FtsHit>> {
            let searcher = reader.searcher();
            let qp = QueryParser::for_index(&index, vec![fields.body, fields.symbol]);
            let parsed = qp.parse_query(&q).map_err(store)?;
            // In tantivy 0.26 `TopDocs` itself is a builder; you need to call
            // one of its `order_by_*` methods to get an actual `Collector`.
            let collector = TopDocs::with_limit(k).order_by_score();
            let top = searcher.search(&parsed, &collector).map_err(store)?;

            let mut out = Vec::with_capacity(top.len());
            for (score, addr) in top {
                let d: TantivyDocument = searcher.doc(addr).map_err(store)?;
                out.push(hit_from_doc(&d, &fields, score));
            }
            Ok(out)
        })
        .await
        .map_err(|e| Error::Store(format!("join: {e}")))?
    }
}

fn build_schema() -> Schema {
    let mut b = Schema::builder();
    b.add_text_field("id", STRING | STORED);
    b.add_text_field("path", STRING | STORED);
    b.add_text_field("symbol", TEXT | STORED);
    b.add_text_field("body", TEXT);
    b.add_u64_field("start_line", STORED | FAST);
    b.add_u64_field("end_line", STORED | FAST);
    b.add_text_field("language", STRING | STORED);
    b.build()
}

fn hit_from_doc(d: &TantivyDocument, f: &Fields, score: f32) -> FtsHit {
    let s = |field: Field| {
        d.get_first(field)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let u = |field: Field| d.get_first(field).and_then(|v| v.as_u64()).unwrap_or(0) as u32;
    let symbol = d
        .get_first(f.symbol)
        .and_then(|v| v.as_str().map(str::to_owned));

    FtsHit {
        id: s(f.id),
        path: s(f.path),
        symbol,
        start_line: u(f.start_line),
        end_line: u(f.end_line),
        language: s(f.language),
        score,
    }
}

fn store<E: std::fmt::Display>(e: E) -> Error {
    Error::Store(e.to_string())
}
