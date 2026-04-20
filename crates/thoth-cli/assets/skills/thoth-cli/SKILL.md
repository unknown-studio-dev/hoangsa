---
name: thoth-cli
description: >
  Use when the user needs to run Thoth CLI commands — setup / index /
  query / watch / impact / context / changes / memory / skills / eval
  / uninstall. Examples: "index this repo", "show memory", "run
  evaluation", "uninstall thoth".
metadata:
  version: "0.0.1"
---

# Thoth CLI Reference

Every Thoth MCP tool has a CLI equivalent, plus a few CLI-only
commands (setup, watch, eval). Run from the repo root unless noted —
the CLI defaults `--root` to `./.thoth`.

## Bootstrap

### `thoth setup`

One-shot install. Writes `./.thoth/config.toml`, seeds `MEMORY.md` +
`LESSONS.md`, merges Thoth hooks + skills + MCP server into
`.claude/settings.json` (or `~/.claude/settings.json` with
`--scope user`). Re-run any time to reconfigure / self-heal.

```bash
thoth setup                # interactive
thoth setup --yes          # accept defaults (CI / scripts)
thoth setup --status       # show install state, don't modify
```

### `thoth uninstall`

Removes Thoth's managed hooks + skills + MCP entry from
`.claude/settings.json` and `.mcp.json`. Leaves the `.thoth/` data
directory intact — delete it manually if you want a hard reset.

```bash
thoth uninstall                    # project scope
thoth uninstall --scope user       # user scope
```

## Indexing

### `thoth index [path]`

Parse + index a source tree. Populates `chunks.db`, the graph, and
(if `--embedder` is set) `vectors.db`.

```bash
thoth index .                         # Mode::Zero — BM25 + symbol + graph
thoth index . --embedder voyage      # Mode::Full — adds semantic vectors
```

Embedders require their API key in the matching env var
(`VOYAGE_API_KEY`, `OPENAI_API_KEY`, `COHERE_API_KEY`) and the matching
Cargo feature at build time.

### `thoth watch [path]`

Re-index on file save. Cheaper than re-running `thoth index` manually
during an active session.

```bash
thoth watch .
thoth watch . --debounce-ms 500
```

## Retrieval

### `thoth query <text...>`

Hybrid recall. Joins extra args with spaces — no quoting needed for
multi-word queries.

```bash
thoth query authentication login session
thoth query -k 16 retry pool exhausted    # more hits
thoth query --json error handler 500      # machine-readable
```

### `thoth impact <fqn>`

Blast-radius analysis. Direction defaults to `up` (who calls this);
`down` is "what does this depend on"; `both` is the union.

```bash
thoth impact server::dispatch_tool
thoth impact auth::verify_token -d 5
thoth impact util::fmt --direction down
```

### `thoth context <fqn>`

360° view of a symbol: callers, callees, extends, extended_by,
references, siblings, unresolved imports.

```bash
thoth context server::dispatch_tool
thoth context auth::Session --limit 64
```

### `thoth changes`

Change-impact over a unified diff. With no `--from`, runs
`git diff HEAD` in the current tree.

```bash
thoth changes                       # current working-tree diff
thoth changes --from patch.diff     # from a file
gh pr diff 123 | thoth changes --from -
thoth changes -d 3                  # deeper upstream walk
```

## Memory

### `thoth memory show`

Print `MEMORY.md` + `LESSONS.md`.

### `thoth memory edit`

Open `MEMORY.md` in `$EDITOR`.

### `thoth memory fact <text...>`

Append a fact. Tags are comma-separated.

```bash
thoth memory fact "HTTP retry lives in crates/net/retry.rs"
thoth memory fact --tags net,retry "HTTP retry lives in ..."
```

### `thoth memory lesson --when <trigger> <advice...>`

Append a lesson.

```bash
thoth memory lesson \
  --when "adding a retry to an HTTP call" \
  Use the existing RetryPolicy in crates/net/retry.rs.
```

### `thoth memory pending`

List entries staged in `MEMORY.pending.md` / `LESSONS.pending.md`
(only populated when `memory_mode = "review"` or on lesson conflicts).

### `thoth memory promote <kind> <index>`

Accept a staged entry. `kind` is `fact` or `lesson`; `index` is 0-based
from `thoth memory pending`.

```bash
thoth memory promote lesson 2
```

### `thoth memory reject <kind> <index> [--reason ...]`

Drop a staged entry without promoting.

```bash
thoth memory reject fact 0 --reason "duplicate of existing fact"
```

### `thoth memory forget`

Run the TTL / capacity sweep. Quarantines lessons whose failure ratio
exceeds `quarantine_failure_ratio`.

### `thoth memory log [--limit N]`

Tail `memory-history.jsonl` — the audit trail of every stage /
promote / reject / quarantine / propose event.

### `thoth memory nudge [--window N]`

Mode::Full only. Asks the synthesizer to propose new lessons from
recent episodes.

## Skills

### `thoth skills list`

Enumerate installed skills (under `.claude/skills/`).

### `thoth skills install [PATH]`

Without `PATH`: (re)installs the bundled skills (`memory-discipline`,
`thoth-reflect`, `thoth-guide`, `thoth-exploring`, `thoth-debugging`,
`thoth-impact-analysis`, `thoth-refactoring`, `thoth-cli`).

With a `PATH` pointing at a `<slug>.draft/` directory (produced by
the agent's `thoth_skill_propose` MCP tool): promotes the draft into
a live skill and removes the draft.

```bash
thoth skills install                                  # bundled
thoth skills install .thoth/skills/my-skill.draft     # promote draft
thoth skills install --scope user                     # ~/.claude/skills/
```

## Evaluation

### `thoth eval --gold <file>`

Run precision@k over a gold query set (TOML). Reports P@k, MRR, and
per-query latency.

```bash
thoth eval --gold eval/gold.toml
thoth eval --gold eval/gold.toml --mode full -k 16
thoth eval --gold eval/gold.toml --mode both    # side-by-side Zero vs Full
```

`--mode full` / `both` requires `--embedder` and/or `--synth`, plus a
stopped daemon (the redb lock is exclusive).

## Domain memory

### `thoth domain sync --source <adapter>`

Pull business rules from an external source (`file`, `notion`,
`asana`, …) into `<root>/domain/<context>/_remote/<source>/`. See
`thoth domain sync --help` for per-adapter flags.

## Global flags

- `--root PATH` — defaults to `./.thoth`. Point at `~/.thoth` for
  user-global memory.
- `--json` — machine-readable output (for subcommands that support it).
- `--embedder <voyage|openai|cohere>` — Mode::Full semantic search.
- `--synth <anthropic|…>` — Mode::Full LLM synthesizer.
- `-v` / `-vv` / `-vvv` — tracing verbosity.
