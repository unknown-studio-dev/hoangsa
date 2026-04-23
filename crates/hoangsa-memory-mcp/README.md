# hoangsa-memory-mcp

MCP (Model Context Protocol) server that exposes `hoangsa-memory` to any
MCP-aware client — Claude Code, Claude Agent SDK, Cursor, Zed, Cowork,
etc.

Speaks **newline-delimited JSON-RPC 2.0** on stdin/stdout per the
`2024-11-05` MCP schema. Also listens on a Unix socket for the
`hoangsa-memory` CLI thin-client, so CLI calls share the same in-process
embedder and watcher.

---

## Tools exposed

| Tool | Purpose |
|------|---------|
| `memory_recall` | Hybrid recall over the indexed code memory (BM25 + symbol + vector) |
| `memory_wakeup` | Compact one-line index of `MEMORY.md` + `LESSONS.md` |
| `memory_index` | Walk a source path, populate all backend indexes |
| `memory_detect_changes` | Git-diff-driven symbol-level change log |
| `memory_impact` | Blast-radius traversal for a symbol FQN |
| `memory_symbol_context` | Callers / callees / parent types / references |
| `memory_detail` | Inspect a single recalled chunk |
| `memory_show` | Return current `MEMORY.md` + `LESSONS.md` |
| `memory_remember_fact` | Append a fact to `MEMORY.md` |
| `memory_remember_lesson` | Append a lesson to `LESSONS.md` |
| `memory_remember_preference` | Append to `USER.md` |
| `memory_remove` / `memory_replace` | Edit memory entries |
| `memory_skills_list` / `memory_skill_propose` | Enumerate / propose skills under `.hoangsa/memory/skills/` |
| `memory_turn_save` / `memory_turns_search` | Persist and search conversation turns |
| `memory_archive_ingest` / `_search` / `_status` / `_topics` | Verbatim conversation archive |

MCP resources exposed:

- `hoangsa-memory://memory/MEMORY.md`
- `hoangsa-memory://memory/LESSONS.md`

---

## Install

Installed automatically by the HOANGSA installer into `~/.hoangsa/bin/`
and registered in your project's `.mcp.json`:

```sh
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
```

**Manual registration** (`.mcp.json`):

```json
{
  "mcpServers": {
    "hoangsa-memory": {
      "command": "/ABSOLUTE/PATH/TO/hoangsa-memory-mcp",
      "env": { "HOANGSA_MEMORY_ROOT": "/abs/path/to/.hoangsa/memory" }
    }
  }
}
```

**Build from source:**

```sh
cargo install --path crates/hoangsa-memory-mcp
```

---

## Running standalone

```sh
hoangsa-memory-mcp                               # stdio + socket, logs to stderr
HOANGSA_MEMORY_ROOT=/abs/path hoangsa-memory-mcp
RUST_LOG=debug hoangsa-memory-mcp
```

Logs go to **stderr** only — stdout is reserved for the JSON-RPC
transport. If you see MCP handshake failures, suspect a stray `println!`
on stdout or a conflicting watcher on the same root.

A background file watcher auto-starts when invoked from a project root;
it debounces filesystem events and keeps the code graph fresh.

---

## Protocol notes

- `initialize` / `initialized` — standard MCP bootstrap.
- `ping` — liveness check.
- `tools/list`, `tools/call` — tool invocation.
- `resources/list`, `resources/read` — exposes memory markdown files as
  MCP resources.

See `src/proto.rs` for the request/response shapes and
`src/server.rs` for the dispatch table.

---

## Layout

```
crates/hoangsa-memory-mcp/
├── src/
│   ├── main.rs       # stdio + socket entry point
│   ├── lib.rs        # public Server API
│   ├── server.rs     # dispatch: tools/list, tools/call, resources/*
│   ├── proto.rs      # MCP wire types
│   └── sanitize.rs   # input normalisation + path scoping
└── tests/            # protocol integration tests
```

---

## License

MIT OR Apache-2.0.
