# hoangsa-memory-core

Public API, traits, and core types for **Hoangsa Memory**. This crate
defines the stable surface every other crate in the workspace depends on
— types, traits, errors — and nothing more.

Deliberately small. No IO, no backends, no retrieval logic. Downstream
crates (`hoangsa-memory-store`, `hoangsa-memory-parse`,
`hoangsa-memory-graph`, `hoangsa-memory-retrieve`,
`hoangsa-memory-policy`, `hoangsa-memory-mcp`) compose these types
directly.

---

## What it exports

| Module | Contents |
|--------|----------|
| `error` | `Error`, `Result` — crate-wide error type |
| `event` | `Event`, `EventId`, `Outcome`, `UserSignal` — episodic events |
| `memory` | `Fact`, `Lesson`, `Preference`, `Skill`, `MemoryKind`, `MemoryMeta`, `Enforcement` |
| `mode` | `Mode::Zero` (no LLM) / `Mode::Full` (LLM-curated) |
| `provider` | `Synthesizer`, `Prompt`, `Synthesis`, `NudgeProposal` — LLM adapter trait |
| `query` | `Query`, `Chunk`, `Citation`, `Retrieval`, `SymbolRef`, `RenderOptions` |

---

## Install (as a dependency)

Already in the workspace. Third-party consumers:

```toml
[dependencies]
hoangsa-memory-core = "0.2"
```

---

## Design notes

- **Mode::Zero is first-class.** Every type works without a Synthesizer
  attached; `Mode::Full` additions are purely additive.
- **No backend assumptions.** A `Chunk` is a value type — nothing about
  redb, tantivy, fastembed, or SQLite leaks in here.
- **Serde-friendly.** All user-visible types derive `Serialize` /
  `Deserialize` so they pass cleanly through MCP JSON-RPC without extra
  conversion.

See `DESIGN.md` §3 and §9 for the rationale.

---

## License

MIT OR Apache-2.0.
