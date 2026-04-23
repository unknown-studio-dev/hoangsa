# hoangsa-memory-retrieve

Retrieval orchestrator + indexer for **Hoangsa Memory**. Given a `Query`
and a `Mode`, it fans out to the relevant stores, fuses the results with
Reciprocal Rank Fusion, and returns a `Retrieval`.

This crate is where BM25, symbol lookup, graph walk, markdown recall,
and vector search meet. It also hosts the `Indexer` that walks a source
tree and populates every backend behind a `StoreRoot`.

---

## Pipeline

```text
Query → { symbol | graph | BM25 | markdown | vector (Mode::Full) }
      → RRF fuse
      → (Mode::Full) Synthesizer::synthesize
      → Retrieval
```

Vector recall runs in-process via `fastembed` (see
`hoangsa_memory_store::vector`); the old ChromaDB Python sidecar has
been removed.

See `DESIGN.md` §4.

---

## Install (as a dependency)

```toml
[dependencies]
hoangsa-memory-retrieve = "0.2"
```

Transitively pulls `hoangsa-memory-core`, `-parse`, `-store`, and
`-graph`.

---

## Public API

```rust
use hoangsa_memory_retrieve::{
    Retriever,                    // hybrid recall entry point
    Indexer, IndexStats, IndexProgress,
    chunk_id, read_span,
    enrich_chunks, extract_docstring,
    RetrieveConfig, IndexConfig, OutputConfig, VectorStoreConfig, WatchConfig,
    IngestOpts, IngestStats, run_ingest,   // verbatim archive ingest
};
```

### Typical usage

```rust
let store = StoreRoot::open(&root).await?;
let mut indexer = Indexer::new(&store);
let stats = indexer.index(&project_root).await?;

let retriever = Retriever::new(&store);
let retrieval = retriever.query(Query::text("retry logic"), /* top_k */ 8).await?;
```

### Config

`RetrieveConfig::load_or_default(&root)` reads `<root>/config.toml` with
sensible defaults. The schema covers retrieval knobs, the indexer's
glob rules, watcher debounce, output shape, and vector-store toggles.

---

## Benchmarks

```sh
cargo bench -p hoangsa-memory-retrieve --bench recall
```

Criterion with Tokio integration; harness in `benches/recall.rs`.

---

## License

MIT OR Apache-2.0.
