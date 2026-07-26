---
name: memory-cli
description: >
  Use when the user needs to run hoangsa-memory CLI commands — init / index /
  query / watch / memory / archive / impact / context / changes / graph /
  projects. Examples: "index this repo", "show memory", "trace the graph",
  "find taint paths", "consolidate memory".
metadata:
  version: "0.0.1"
---

# hoangsa-memory CLI Reference

Most hoangsa-memory MCP tools have a CLI equivalent, plus a few CLI-only
commands (init, watch, prefetch-embed). Run from the repo root unless
noted — the CLI defaults `--root` to `./.hoangsa/memory`.

## Bootstrap

### `hoangsa-memory init`

Create a bare `./.hoangsa/memory/`: seeds `MEMORY.md`, `LESSONS.md`, and a
fully commented `config.toml`. Idempotent — existing files are preserved.
Higher-level install (hooks, MCP registration, skills) is `hoangsa-cli
install`, not this.

### `hoangsa-memory prefetch-embed`

Download the default embedding model (~118 MB) into the shared fastembed
cache so the first `index` / `query` doesn't stall. No-op once cached.

## Indexing

### `hoangsa-memory index [path]`

Parse + index a source tree. Populates `chunks.db`, the graph, and
(if `--embedder` is set) `vectors.db`.

```bash
hoangsa-memory index .                         # Mode::Zero — BM25 + symbol + graph
hoangsa-memory index . --embedder voyage      # Mode::Full — adds semantic vectors
```

Embedders require their API key in the matching env var
(`VOYAGE_API_KEY`, `OPENAI_API_KEY`, `COHERE_API_KEY`) and the matching
Cargo feature at build time.

### `hoangsa-memory watch [path]`

Re-index on file save. Cheaper than re-running `hoangsa-memory index` manually
during an active session.

```bash
hoangsa-memory watch .
hoangsa-memory watch . --debounce-ms 500
```

## Retrieval

### `hoangsa-memory query <text...>`

Hybrid recall. Joins extra args with spaces — no quoting needed for
multi-word queries.

```bash
hoangsa-memory query authentication login session
hoangsa-memory query -k 16 retry pool exhausted    # more hits
hoangsa-memory query --json error handler 500      # machine-readable
```

### `hoangsa-memory impact <fqn>`

Blast-radius analysis. Direction defaults to `up` (who calls this);
`down` is "what does this depend on"; `both` is the union.

```bash
hoangsa-memory impact server::dispatch_tool
hoangsa-memory impact auth::verify_token -d 5
hoangsa-memory impact util::fmt --direction down
```

### `hoangsa-memory context <fqn>`

360° view of a symbol: callers, callees, extends, extended_by,
references, siblings, unresolved imports.

```bash
hoangsa-memory context server::dispatch_tool
hoangsa-memory context auth::Session --limit 64
```

### `hoangsa-memory changes`

Change-impact over a unified diff. With no `--from`, runs
`git diff HEAD` in the current tree.

```bash
hoangsa-memory changes                       # current working-tree diff
hoangsa-memory changes --from patch.diff     # from a file
gh pr diff 123 | hoangsa-memory changes --from -
hoangsa-memory changes -d 3                  # deeper upstream walk
```

## Graph

Whole-shape queries over the code graph — reach for these instead of
grepping call chains by hand. All accept `--json`.

### `hoangsa-memory graph query <start...>`

Traverse callers/callees/refs/imports from seed symbol(s). Filter by
direction / edge kinds / depth; export JSON or DOT.

```bash
hoangsa-memory graph query server::dispatch_tool --direction both -d 2
hoangsa-memory graph query auth::verify_token --edge-kinds calls --format dot
```

### `hoangsa-memory graph paths <from> <to>`

Shortest dependency/call path between two symbols.

```bash
hoangsa-memory graph paths router::handle_login db::users::find_by_email
```

### `hoangsa-memory graph communities`

Architecture map — clusters of tightly-coupled symbols (largest first).

```bash
hoangsa-memory graph communities --min-size 5
```

### `hoangsa-memory graph processes`

Execution flows from entry points (`::main` or `--entry-globs`).

```bash
hoangsa-memory graph processes -d 8
```

### `hoangsa-memory graph taint`

Source→sink security dataflow. Requires an index built with
`hoangsa-memory index . --pdg`. Omit `--source`/`--sink` for built-in defaults.

```bash
hoangsa-memory index . --pdg
hoangsa-memory graph taint --source env::var --sink Command::new --json
```

## Memory

### `hoangsa-memory memory show`

Print `MEMORY.md` + `LESSONS.md`.

### `hoangsa-memory memory edit`

Open `MEMORY.md` in `$EDITOR`.

### `hoangsa-memory memory fact <text...>`

Append a fact. Tags are comma-separated.

```bash
hoangsa-memory memory fact "HTTP retry lives in crates/net/retry.rs"
hoangsa-memory memory fact --tags net,retry "HTTP retry lives in ..."
```

### `hoangsa-memory memory lesson --when <trigger> <advice...>`

Append a lesson.

```bash
hoangsa-memory memory lesson \
  --when "adding a retry to an HTTP call" \
  Use the existing RetryPolicy in crates/net/retry.rs.
```

### `hoangsa-memory memory lesson-feedback success|failure <trigger...>`

Bump a lesson's success / failure counter. The forget pass drops lessons
below `lesson_floor` and quarantines those past `quarantine_failure_ratio`,
so these counters are what eventually prunes bad advice.

```bash
hoangsa-memory memory lesson-feedback failure "when editing migrations"
```

### `hoangsa-memory memory dream [--force] [--dry-run]`

Run the consolidation pass now: hand `MEMORY.md` / `LESSONS.md` /
`USER.md` to a model that merges duplicates, rewrites stale entries, and
drops ones the code no longer supports. `--dry-run` reports its verdicts
without writing. `--force` ignores `[dream].enabled` and the
`min_interval_hours` floor. Normally the daemon runs this on its own when
the project goes idle.

```bash
hoangsa-memory memory dream --dry-run     # see what it would change
hoangsa-memory memory dream --force       # run it now regardless of config
```

## Archive

### `hoangsa-memory archive <ingest|search|status|topics|purge>`

Verbatim conversation archive, separate from curated memory. `ingest` is
normally hook-driven (PreCompact / SessionEnd). `purge` takes either
`--older-than 30d` or `--all`. See `hoangsa-memory archive --help`.

## Projects

### `hoangsa-memory projects <list|rename|remove|show>`

Manage the registry at `~/.hoangsa/projects.json`. Every CLI invocation
auto-registers the cwd; these subcommands edit that.

## Global flags

- `--root PATH` — defaults to `./.hoangsa/memory`. Point at `~/.hoangsa/memory` for
  user-global memory.
- `--json` — machine-readable output (for subcommands that support it).
- `-v` / `-vv` / `-vvv` — tracing verbosity.
