# hoangsa-memory

The command-line interface for **Hoangsa Memory** — long-term memory and
code intelligence for coding agents. Ships a single `hoangsa-memory`
binary that drives the library stack (`hoangsa-memory-core`,
`hoangsa-memory-parse`, `-store`, `-graph`, `-retrieve`, `-policy`) and
dispatches to the `hoangsa-memory-mcp` daemon when an MCP client is
attached.

Operators use it to index a repo, query recall, inspect memory, run
impact analysis, and manage the verbatim conversation archive.

---

## Commands

```text
hoangsa-memory <cmd> [args]

  init                       Seed .hoangsa/memory/ (MEMORY.md, LESSONS.md, config.toml)
  index [path]               Parse + index a source tree (default: .)
  query <text...>            Hybrid recall over the indexed memory
  watch [path]               Re-index on filesystem change
  memory show | edit | fact | lesson
                             Inspect or edit MEMORY.md / LESSONS.md
  archive ingest | status | topics | search | purge
                             Verbatim conversation archive (fastembed + SQLite)
  impact <fqn>               Blast-radius graph traversal for a symbol FQN
  context <fqn>              360-degree symbol context (callers/callees/types)
  changes [--from <ref>]     Change-impact analysis over a diff
  prefetch-embed             Download the multilingual-e5-small ONNX weights

Flags:
  --root <dir>      Memory data dir (default: .hoangsa/memory or ~/.hoangsa/memory/projects/<slug>)
  --json            Machine-readable output for supported subcommands
  -v / -vv          Verbosity (info / debug); -vvv = trace
```

Root resolution: `--root` > `$HOANGSA_MEMORY_ROOT` > `./.hoangsa/memory/`
> `~/.hoangsa/memory/projects/<slug>/`.

---

## Install

Install the whole HOANGSA toolchain (includes `hoangsa-memory`):

```sh
curl -fsSL https://github.com/pirumu/hoangsa/releases/latest/download/install.sh | sh
```

The installer:

1. Drops `hoangsa-memory` and `hoangsa-memory-mcp` into `~/.hoangsa/bin/`.
2. Pre-downloads the `multilingual-e5-small` ONNX weights (~118 MB, or
   ~4xx MB with tokenizer assets) into `~/.hoangsa/cache/fastembed/`.
3. Registers the MCP server in your project's `.mcp.json` so Claude Code
   can call `memory_recall`, `memory_remember_*`, `memory_impact`, etc.

Pass `--no-embed` to skip step 2 (weights fetch lazily on first use).

See the [root README](../../README.md#installation) for flags and
environment overrides.

**Build from source:**

```sh
cargo install --path crates/hoangsa-memory
```

---

## Quick start

```sh
cd my-project
hoangsa-memory init               # seed .hoangsa/memory/
hoangsa-memory index .            # walk + parse + persist chunks + symbols
hoangsa-memory query "retry logic" -k 10
hoangsa-memory impact my_crate::Foo --direction both --depth 3
hoangsa-memory memory show
```

Once indexed, Claude Code's `memory_recall` MCP tool returns
`path:line-span` hits you can cite instead of guessing at APIs.

---

## Storage layout

Each project gets a memory root with these files:

```text
.hoangsa/memory/
├── config.toml          # retrieval / vector / watch / output config
├── MEMORY.md            # durable project facts
├── LESSONS.md           # action-triggered advice
├── USER.md              # cross-project user preferences
├── skills/<slug>/       # SKILL.md + assets
├── graph.redb           # symbol + call graph (redb)
├── fts.tantivy/         # BM25 full-text index
├── episodes.db          # SQLite + FTS5 episodic log
└── vectors.sqlite       # fastembed vectors as BLOBs
```

See `hoangsa-memory-store` for the layout contract, and
`hoangsa-memory-retrieve/src/config.rs` for the `config.toml` schema.

---

## License

MIT OR Apache-2.0.
