---
name: memory-cli
description: >
  Use when the user needs to run hoangsa-memory CLI commands — setup / index /
  query / watch / impact / context / changes / memory / skills / eval
  / uninstall. Examples: "index this repo", "show memory", "run
  evaluation", "uninstall hoangsa-memory".
metadata:
  version: "0.0.1"
---

# hoangsa-memory CLI Reference

Every hoangsa-memory MCP tool has a CLI equivalent, plus a few CLI-only
commands (setup, watch, eval). Run from the repo root unless noted —
the CLI defaults `--root` to `./.hoangsa/memory`.

## Bootstrap

### `hoangsa-memory setup`

One-shot install. Writes `./.hoangsa/memory/config.toml`, seeds `MEMORY.md` +
`LESSONS.md`, merges hoangsa-memory hooks + skills + MCP server into
`.claude/settings.json` (or `~/.claude/settings.json` with
`--scope user`). Re-run any time to reconfigure / self-heal.

```bash
hoangsa-memory setup                # interactive
hoangsa-memory setup --yes          # accept defaults (CI / scripts)
hoangsa-memory setup --status       # show install state, don't modify
```

### `hoangsa-memory uninstall`

Removes hoangsa-memory's managed hooks + skills + MCP entry from
`.claude/settings.json` and `.mcp.json`. Leaves the `.hoangsa/memory/` data
directory intact — delete it manually if you want a hard reset.

```bash
hoangsa-memory uninstall                    # project scope
hoangsa-memory uninstall --scope user       # user scope
```

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

### `hoangsa-memory memory pending`

List entries staged in `MEMORY.pending.md` / `LESSONS.pending.md`
(only populated when `memory_mode = "review"` or on lesson conflicts).

### `hoangsa-memory memory promote <kind> <index>`

Accept a staged entry. `kind` is `fact` or `lesson`; `index` is 0-based
from `hoangsa-memory memory pending`.

```bash
hoangsa-memory memory promote lesson 2
```

### `hoangsa-memory memory reject <kind> <index> [--reason ...]`

Drop a staged entry without promoting.

```bash
hoangsa-memory memory reject fact 0 --reason "duplicate of existing fact"
```

### `hoangsa-memory memory forget`

Run the TTL / capacity sweep. Quarantines lessons whose failure ratio
exceeds `quarantine_failure_ratio`.

### `hoangsa-memory memory log [--limit N]`

Tail `memory-history.jsonl` — the audit trail of every stage /
promote / reject / quarantine / propose event.

### `hoangsa-memory memory nudge [--window N]`

Mode::Full only. Asks the synthesizer to propose new lessons from
recent episodes.

## Skills

### `hoangsa-memory skills list`

Enumerate installed skills (under `.claude/skills/`).

### `hoangsa-memory skills install [PATH]`

Without `PATH`: (re)installs the bundled skills (`memory-discipline`,
`memory-reflect`, `memory-guide`, `memory-exploring`, `memory-debugging`,
`memory-impact-analysis`, `memory-refactoring`, `memory-cli`).

With a `PATH` pointing at a `<slug>.draft/` directory (produced by
the agent's `memory_skill_propose` MCP tool): promotes the draft into
a live skill and removes the draft.

```bash
hoangsa-memory skills install                                  # bundled
hoangsa-memory skills install .hoangsa/memory/skills/my-skill.draft     # promote draft
hoangsa-memory skills install --scope user                     # ~/.claude/skills/
```

## Evaluation

### `hoangsa-memory eval --gold <file>`

Run precision@k over a gold query set (TOML). Reports P@k, MRR, and
per-query latency.

```bash
hoangsa-memory eval --gold eval/gold.toml
hoangsa-memory eval --gold eval/gold.toml --mode full -k 16
hoangsa-memory eval --gold eval/gold.toml --mode both    # side-by-side Zero vs Full
```

`--mode full` / `both` requires `--embedder` and/or `--synth`, plus a
stopped daemon (the redb lock is exclusive).

## Domain memory

### `hoangsa-memory domain sync --source <adapter>`

Pull business rules from an external source (`file`, `notion`,
`asana`, …) into `<root>/domain/<context>/_remote/<source>/`. See
`hoangsa-memory domain sync --help` for per-adapter flags.

## Global flags

- `--root PATH` — defaults to `./.hoangsa/memory`. Point at `~/.hoangsa/memory` for
  user-global memory.
- `--json` — machine-readable output (for subcommands that support it).
- `--embedder <voyage|openai|cohere>` — Mode::Full semantic search.
- `--synth <anthropic|…>` — Mode::Full LLM synthesizer.
- `-v` / `-vv` / `-vvv` — tracing verbosity.
