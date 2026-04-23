# hoangsa-memory-graph

Symbol, call, import, and reference graph built on top of
`hoangsa_memory_store::KvStore` (redb). This is the spine of Mode::Zero
retrieval: it answers *"who calls X"*, *"what does X call"*, *"which
modules import Y"* without any LLM or embedding.

---

## Design

- Every parsed symbol becomes a `Node` keyed by its fully qualified name
  (FQN). Nodes carry the declaration path + 1-based line.
- Every `calls` / `imports` / `extends` / `references` relationship
  becomes an `Edge`. Edges are stored in the underlying KV as
  `"<src>|<kind>|<dst>"`, so outgoing-edge lookups collapse into a
  prefix scan.
- Traversal is plain BFS bounded by `depth` — fine at indexing scale and
  keeps the API predictable for the `memory_impact` MCP tool.

See `DESIGN.md` §4 and §5.

---

## Install (as a dependency)

```toml
[dependencies]
hoangsa-memory-graph = "0.2"
```

Transitively pulls `hoangsa-memory-core`, `-store`, and `-parse`.

---

## Public API

```rust
use hoangsa_memory_graph::{Node, Edge, EdgeKind, Graph};

let mut g = Graph::open(store_root)?;
let impact = g.blast_radius("my_crate::Foo", BfsDir::Up, /* depth */ 3)?;
```

`EdgeKind` variants: `Calls`, `Imports`, `Extends`, `References`.

---

## License

MIT OR Apache-2.0.
