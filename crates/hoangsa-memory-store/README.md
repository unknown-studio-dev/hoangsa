# hoangsa-memory-store

Embedded storage backends used by **Hoangsa Memory**. Each submodule
wraps a specific backend behind a thin async-friendly API so that
`hoangsa-memory-retrieve` and `hoangsa-memory-policy` never depend on a
concrete engine.

| Backend | Crate | Role |
|---------|-------|------|
| `redb` | redb | graph nodes / edges + symbol lookup + metadata |
| `tantivy` | tantivy | BM25 full-text index |
| `sqlite` | rusqlite (bundled) | episodic FTS5 log + vector BLOB storage |
| `fastembed` | fastembed | semantic embeddings (ONNX, in-process, no Python) |
| `markdown` | — | `MEMORY.md` / `LESSONS.md` / `USER.md` readers + writers |

No Python sidecar. The old ChromaDB dependency was removed — vectors now
live in a per-project SQLite file with ONNX embeddings produced by
`fastembed`.

---

## On-disk layout

```text
<root>/
  config.toml        (optional user config — loaded by the policy layer)
  MEMORY.md
  LESSONS.md
  USER.md
  skills/<slug>/SKILL.md
  graph.redb         (symbol + call graph)
  fts.tantivy/       (BM25 index)
  episodes.db        (SQLite + FTS5 episodic log)
  vectors.sqlite     (fastembed vectors as BLOBs; replaces chroma/)
```

For backward compat, `StoreRoot::open` auto-migrates the old `index/`
subdirectory layout the first time it opens a stale store.

---

## Install (as a dependency)

```toml
[dependencies]
hoangsa-memory-store = "0.2"
```

### Build prerequisites

- **SQLite** — `rusqlite` uses the `bundled` feature, so no system
  library is required.
- **ONNX runtime** — `fastembed` uses `ort` with **rustls** binaries
  pulled at build time. glibc-only (musl/Alpine not supported — use
  `build-from-source` on those platforms). First run of a program that
  instantiates the embedder downloads `multilingual-e5-small` weights
  (~118 MB) into `~/.hoangsa/cache/fastembed/`. The HOANGSA installer
  pre-fetches them for you; pass `--no-embed` to skip.
- **tantivy / redb / tree-sitter** — pure Rust, no system deps.

---

## Public API highlights

```rust
use hoangsa_memory_store::{
    StoreRoot,                              // on-disk root + open()
    KvStore, NodeRow, EdgeRow, BfsDir,      // redb graph
    FtsIndex,                               // tantivy BM25
    EpisodeLog,                             // SQLite + FTS5
    EmbeddedVectorStore, VectorCol,         // fastembed + SQLite
    ArchiveStore,                           // verbatim conversation archive
    MarkdownStore,                           // MEMORY.md / LESSONS.md
    prefetch_model, fastembed_cache_dir,    // model weight bootstrap
};
```

Each backend is independently openable — tests wire up just the one they
need via `tempfile` + `StoreRoot::open`.

---

## License

MIT OR Apache-2.0.
