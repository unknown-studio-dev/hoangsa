# hoangsa-memory-parse

tree-sitter wrapper, AST-aware chunking, file discovery, and change
watching — the perception layer for **Hoangsa Memory**.

Its outputs feed every other pipeline:

- `parse_file` produces `SourceChunk`s and a `SymbolTable` that
  `hoangsa-memory-store` persists.
- `walk::walk_sources` enumerates indexable files in a project, honouring
  `.gitignore` and friends.
- `watch::Watcher` streams `hoangsa_memory_core::Event`s on filesystem
  change.

---

## Install (as a dependency)

```toml
[dependencies]
hoangsa-memory-parse = "0.2"
```

### Language features

Every supported language is gated behind a Cargo feature. The defaults
cover the core quartet:

```toml
default = ["lang-rust", "lang-python", "lang-javascript", "lang-typescript"]
```

Opt-in languages:

| Feature | tree-sitter grammar |
|---------|---------------------|
| `lang-rust` | `tree-sitter-rust` |
| `lang-python` | `tree-sitter-python` |
| `lang-javascript` | `tree-sitter-javascript` |
| `lang-typescript` | `tree-sitter-typescript` |
| `lang-go` | `tree-sitter-go` |

Enable with:

```toml
hoangsa-memory-parse = { version = "0.2", features = ["lang-go"] }
```

Or disable defaults and pick exactly what you need:

```toml
hoangsa-memory-parse = { version = "0.2", default-features = false, features = ["lang-rust"] }
```

tree-sitter ABI: grammar crates track `tree-sitter` core `0.23.x`.

---

## Public API

```rust
use hoangsa_memory_parse::{parse_file, SourceChunk, SymbolTable, Language};
use hoangsa_memory_parse::walk::walk_sources;
use hoangsa_memory_parse::watch::Watcher;
```

- `parse_file(path)` — AST-aware chunking + symbol extraction.
- `walk_sources(root)` — gitignore-honouring file iterator.
- `Watcher::new(root)` — debounced filesystem watcher.

See `DESIGN.md` §4 and §9.

---

## License

MIT OR Apache-2.0.
