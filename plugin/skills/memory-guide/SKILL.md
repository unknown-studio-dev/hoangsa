---
name: memory-guide
description: >
  Use when the user asks about hoangsa-memory itself — available MCP tools, CLI
  commands, resources, prompts, skill catalog, or how to drive the
  memory/graph workflow. Examples: "what hoangsa-memory tools are available?",
  "how do I use hoangsa-memory?", "what skills do I have?".
metadata:
  version: "0.0.1"
---

# hoangsa-memory Guide

Quick reference for every hoangsa-memory MCP tool, resource, prompt, and skill.
hoangsa-memory is a local memory + code-graph server exposed over MCP — it pairs
a hybrid retriever (symbol + BM25 + vector + graph) with a markdown
memory layer (`MEMORY.md`, `LESSONS.md`, `USER.md`) and a PreToolUse
discipline gate.

## Always Start Here

For any non-trivial coding task:

1. Call `memory_recall` with a query derived from the user's intent. The
   `UserPromptSubmit` hook also recalls for context, but that ceremonial
   call does **not** satisfy the discipline gate — only agent-initiated
   recalls do (it passes `log_event: false`; yours defaults to `true`).
2. Read the chunks. Each has `path:line-span` you can cite. Bodies are
   stripped by default — `Read path:L-L` on a hit, or pass `detail: true`.
3. Match the task to one of the skills below and follow its workflow.
4. After acting, reflect via the `memory_reflect` prompt → persist
   fact/lesson if durable.

If `memory_recall` returns `(no matches — did you run memory_index?)`,
stop and run `hoangsa-memory index .` (CLI) or `memory_index` (MCP) before
continuing.

## Skills

| Skill                      | When to read it                                         |
| -------------------------- | ------------------------------------------------------- |
| `memory-discipline`        | Before any Write/Edit/Bash — enforces the recall loop.  |
| `memory-reflect`           | End of session / after a bug fix / "what did we learn". |
| `memory-exploring`         | "How does X work?" / architecture questions.            |
| `memory-debugging`         | "Why does this fail?" / tracing errors.                 |
| `memory-impact-analysis`   | "What breaks if I change X?" / pre-commit safety.       |
| `memory-refactoring`       | Rename / extract / move / restructure.                  |
| `memory-cli`               | Running `hoangsa-memory index`, `query`, `memory dream`, … |

## MCP tools

### Retrieval

- **`memory_recall { query, top_k?, scope?, tags?, min_score?, detail?,
  log_event? }`** — hybrid recall (symbol + BM25 + graph + markdown +
  semantic). Returns ranked chunks with path, line span, preview, and
  graph context. `scope` ∈ `curated` (default: code + facts/lessons) |
  `archive` (verbatim conversations only) | `all`. `log_event` defaults
  to `true` — that log is what proves to `hoangsa-cli enforce` that you
  consulted memory before mutating.
- **`memory_index { path? }`** — walk a source tree, parse it, populate
  symbols / call graph / BM25 / chunks. `path` defaults to `.`.
- **`memory_symbol_context { fqn, limit? }`** — 360° view of a symbol:
  callers, callees, extends, extended_by, references, siblings,
  unresolved imports. Pure graph lookup keyed on exact FQN.
- **`memory_impact { fqn, direction?, depth? }`** — BFS blast radius.
  `direction` ∈ `up | down | both` (default `up`), `depth` ∈ `[1,8]`
  (default 3). `up` answers "what breaks if I change X?".
- **`memory_detect_changes { diff, depth? }`** — feed a unified diff
  (stdout of `git diff`), returns touched symbols + upstream blast
  radius per hunk. Ideal as a PR pre-check.
- **`memory_event_trace { topic, bus?, limit? }`** — publishers and
  subscribers of an event-bus topic. Use when a pub/sub flow is
  decoupled by a broker so `memory_symbol_context` can't connect the two
  halves.

### Graph traversal & analytics

Reach for these **instead of** repeated Grep/Read when tracing how code
connects — one call replaces many searches. Unknown FQNs land in
`unresolved`; these tools do not error on a bad name.

- **`memory_graph_query { start[], direction?, edge_kinds?, max_depth?,
  max_nodes?, format? }`** — BFS from seed symbol(s). `direction` ∈
  `out | in | both` (default `out`), `edge_kinds` ∈ `calls, imports,
  references, extends, declared_in, emits, subscribes`. `format` ∈
  `json` (default) | `dot` (Graphviz). Answers "who calls X / what does
  X reach". `truncated` is set when `max_nodes` (default 500) is hit.
- **`memory_graph_paths { from, to, edge_kinds?, direction?, max_depth? }`**
  — shortest dependency path A→B. Returns `found: false` when
  unreachable within `max_depth` (default 10).
- **`memory_graph_communities { min_size? }`** — architecture map:
  clusters of tightly-coupled symbols via label propagation over
  Calls/Imports/Extends. Answers "what are the main components?" without
  reading the tree. Sorted largest-first; `min_size` defaults to 3.
- **`memory_graph_processes { max_depth?, entry_globs? }`** — execution
  flows from entry points down Calls edges. Symbols ending in `::main`
  are always entry points; add more via `entry_globs`. Answers "walk me
  through what happens from startup".
- **`memory_taint_paths { sources?, sinks?, max_depth?, max_findings? }`**
  — security dataflow: can untrusted input reach a dangerous sink?
  Traces source→sink over DataDep and Calls edges only, never plain
  control flow. **Requires an index built with `--pdg`.** Omitting
  `sources`/`sinks` uses built-in defaults (env vars, stdin, args →
  subprocess, eval, `fs::write`, query).

### Memory (read)

- **`memory_wakeup { scope?, include_on_demand? }`** — compact
  one-line-per-entry index of facts, lessons, and preferences. Start
  here at session start; it's far cheaper than `memory_show`. `scope` ∈
  `all` (default) | `facts` | `lessons` | `preferences`. Only
  `always`-scope facts appear unless `include_on_demand: true`.
- **`memory_detail { id }`** — full content of one entry. `id` is an
  index from `memory_wakeup` (e.g. `F03`, `L01`) or a heading substring.
- **`memory_show`** — dump `MEMORY.md` + `LESSONS.md` + `USER.md` whole.
  Prefer wakeup + detail once memory grows.
- **Resources**: `resources/read` with URI `hoangsa-memory://memory/MEMORY.md`
  or `hoangsa-memory://memory/LESSONS.md` — same data, lighter wire shape.

The audit trail is `.hoangsa/memory/memory-history.jsonl`; read the file
directly, there is no tool for it.

### Memory (write)

- **`memory_remember_fact { text, tags?, scope? }`** — append a durable
  fact. `scope: "on-demand"` keeps it out of the always-injected set.
- **`memory_remember_lesson { trigger, advice, suggested_enforcement?,
  block_message? }`** — append a reflective lesson. `trigger` is a
  situation description ("adding a retry to an HTTP call"), not a
  command. A trigger that already exists is **rejected**, with the
  existing advice returned — use `memory_replace` if yours supersedes it.
- **`memory_remember_preference { text, tags? }`** — first-person,
  cross-project workflow choice. Lands in `USER.md`.
- **`memory_replace { kind, query, new_text }`** / **`memory_remove
  { kind, query }`** — edit or drop one entry. `kind` ∈ `fact | lesson |
  preference`. This is how you resolve a cap-exceeded error or a
  duplicate trigger.
- **`memory_skill_propose { slug, body, source_triggers? }`** — draft a
  new skill from ≥5 related lessons. Lands in
  `.hoangsa/memory/skills/<slug>.draft/` for user review.
- **`memory_skills_list`** — enumerate installed skills.

Confidence counters are bumped for you — the Stop hook records a success
or failure against every lesson it surfaced (via `hoangsa-memory memory
lesson-feedback`). Pruning is automatic too: the daemon runs the forget
pass (TTL, capacity, decay, quarantine) and, when `[dream].enabled`, the
dream consolidation pass. There is no MCP tool for either.

### Conversation archive

Verbatim conversation history, separate from curated memory. Normally
driven by the PreCompact / SessionEnd hooks; query it when the user
refers to a past discussion rather than to code.

- **`memory_archive_status`** — sessions, turns, curated count. ~100
  tokens; good for orientation.
- **`memory_archive_topics { project? }`** — topics with session and
  turn counts.
- **`memory_archive_search { query, top_k?, project?, topic? }`** —
  semantic search over archived conversations. Finds past discussions,
  decisions, and context. (`memory_recall` with `scope: "archive"` or
  `"all"` reaches the same corpus alongside code.)
- **`memory_archive_ingest { project?, topic?, refresh?, limit? }`** —
  ingest Claude Code sessions through the daemon so concurrent sessions
  don't each reload the ONNX model. Hook-driven; rarely called by hand.
- **`memory_turn_save { session_id, role, content, commit_sha?,
  file_paths? }`** — save one verbatim turn. `commit_sha` + `file_paths`
  let `memory_archive_search` link the turn back to the code state at
  that moment.
- **`memory_turns_search { query, top_k? }`** — full-text (FTS5) search
  over saved turns, with session context.

## MCP prompts

Fetch via `prompts/get { name, arguments }`:

- **`memory_nudge { intent }`** — surfaces LESSONS.md entries whose
  trigger plausibly applies, and forces you to restate the plan naming
  each lesson you're honouring.
- **`memory_reflect { summary, outcome? }`** — end-of-step reflection.
  Drives the fact/lesson decision.
- **`memory_grounding_check { claim }`** — verify a factual claim
  against the indexed graph before asserting it. Only advertised when
  `[curation] grounding_check = true` (legacy alias: `[discipline]`).

## Enforcement

PreToolUse gating lives in `hoangsa-cli hook enforce`, not in this
crate. It reads `.hoangsa/rules.json` plus
`.hoangsa/state/enforcement.events` — see the `hoangsa-cli rule` docs.
If the gate blocks, the stderr message tells you which prerequisite
(e.g. `memory_recall`, `memory_impact`) is missing — call it, then
retry.

## CLI parity

Most MCP tools have a CLI equivalent for headless use:

| CLI                                        | MCP tool                  |
| ------------------------------------------ | ------------------------- |
| `hoangsa-memory query <text>`              | `memory_recall`           |
| `hoangsa-memory index [path]`              | `memory_index`            |
| `hoangsa-memory impact <fqn>`              | `memory_impact`           |
| `hoangsa-memory context <fqn>`             | `memory_symbol_context`   |
| `hoangsa-memory changes [--from <file\|->]`| `memory_detect_changes`   |
| `hoangsa-memory graph query \| paths \| communities \| processes \| taint` | `memory_graph_*`, `memory_taint_paths` |
| `hoangsa-memory memory show`               | `memory_show`             |
| `hoangsa-memory memory fact \| lesson`     | `memory_remember_*`       |
| `hoangsa-memory archive <ingest\|search\|status\|topics>` | `memory_archive_*` |

CLI-only: `init`, `watch`, `prefetch-embed`, `projects`, `memory edit`,
`memory lesson-feedback`, `memory dream`. Note the CLI takes the redb
lock exclusively — stop the daemon first, or prefer the MCP tool.

See the `memory-cli` skill for the full command tree.
