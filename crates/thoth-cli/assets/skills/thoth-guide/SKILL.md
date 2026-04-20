---
name: thoth-guide
description: >
  Use when the user asks about Thoth itself — available MCP tools, CLI
  commands, resources, prompts, skill catalog, or how to drive the
  memory/graph workflow. Examples: "what Thoth tools are available?",
  "how do I use Thoth?", "what skills do I have?".
metadata:
  version: "0.0.1"
---

# Thoth Guide

Quick reference for every Thoth MCP tool, resource, prompt, and skill.
Thoth is a local memory + code-graph server exposed over MCP — it pairs
a hybrid retriever (symbol + BM25 + vector + graph) with a markdown
memory layer (`MEMORY.md`, `LESSONS.md`) and a PreToolUse discipline
gate.

## Always Start Here

For any non-trivial coding task:

1. Call `thoth_recall` with a query derived from the user's intent. The
   `UserPromptSubmit` hook also recalls for context, but that ceremonial
   call does **not** satisfy the discipline gate — only agent-initiated
   recalls do.
2. Read the chunks. Each has `path:line-span` you can cite.
3. Match the task to one of the skills below and follow its workflow.
4. After acting, reflect via `thoth.reflect` → persist fact/lesson if
   durable.

If `thoth_recall` returns `(no matches — did you run thoth_index?)`,
stop and run `thoth index .` (CLI) or `thoth_index` (MCP) before
continuing.

## Skills

| Skill                      | When to read it                                         |
| -------------------------- | ------------------------------------------------------- |
| `memory-discipline`        | Before any Write/Edit/Bash — enforces the recall loop.  |
| `thoth-reflect`            | End of session / after a bug fix / "what did we learn". |
| `thoth-exploring`          | "How does X work?" / architecture questions.            |
| `thoth-debugging`          | "Why does this fail?" / tracing errors.                 |
| `thoth-impact-analysis`    | "What breaks if I change X?" / pre-commit safety.       |
| `thoth-refactoring`        | Rename / extract / move / restructure.                  |
| `thoth-cli`                | Running `thoth setup`, `thoth index`, `thoth eval`, …   |

## MCP tools

### Retrieval

- **`thoth_recall { query, top_k?, log_event? }`** — hybrid recall
  (BM25 + symbol + vector + markdown). Default `log_event = true` —
  agent-initiated recalls must log; that's what the discipline gate
  checks.
- **`thoth_symbol_context { fqn, limit? }`** — 360° view of a symbol:
  callers, callees, extends, extended_by, references, siblings,
  unresolved imports. Pure graph lookup keyed on exact FQN.
- **`thoth_impact { fqn, direction?, depth? }`** — BFS blast radius.
  `direction` ∈ `up | down | both` (default `up`), `depth` ∈ `[1,8]`
  (default 3). `up` answers "what breaks if I change X?".
- **`thoth_detect_changes { diff, depth? }`** — feed a unified diff
  (stdout of `git diff`), returns touched symbols + upstream callers
  per hunk. Designed for pre-commit / PR review.

### Memory (read)

- **`thoth_memory_show`** — dump current `MEMORY.md` + `LESSONS.md`.
- **`thoth_memory_pending`** — list staged facts/lessons awaiting
  promotion (only non-empty when `memory_mode = "review"` or on
  lesson-trigger conflicts).
- **`thoth_memory_history { limit? }`** — tail of
  `memory-history.jsonl` (stage/promote/reject/quarantine events).
- **Resources**: `resources/read` with URI `thoth://memory/MEMORY.md`
  or `thoth://memory/LESSONS.md` — same data, lighter wire shape.

### Memory (write)

- **`thoth_remember_fact { text, tags?, stage? }`** — append a durable
  fact. Set `stage: true` if you're unsure — it lands in
  `MEMORY.pending.md` instead.
- **`thoth_remember_lesson { trigger, advice, stage? }`** — append a
  reflective lesson. `trigger` is a situation description ("adding a
  retry to an HTTP call"), not a command. Conflicts with existing
  triggers auto-stage.
- **`thoth_lesson_outcome { signal, triggers }`** — bump confidence
  counters. `signal` ∈ `success | failure`, `triggers` is the list of
  lessons that were in play. Call this after the outcome of an action
  guided by lessons.
- **`thoth_memory_forget`** — run the TTL sweep. Quarantines lessons
  whose failure ratio exceeds `quarantine_failure_ratio`.
- **`thoth_memory_promote { kind, index }`** / **`thoth_memory_reject
  { kind, index, reason? }`** — resolve pending entries.
- **`thoth_request_review`** — flag an entry for the user to audit
  (writes to `memory-history.jsonl`).
- **`thoth_episode_append { event }`** — raw episodic log entry.
  Normally hook-driven; agents rarely call this directly.
- **`thoth_skill_propose { slug, body, source_triggers? }`** — draft a
  new skill from ≥5 related lessons. Lands in
  `.thoth/skills/<slug>.draft/` for user review.
- **`thoth_skills_list`** — enumerate installed skills.

## MCP prompts

Fetch via `prompts/get { name, arguments }`:

- **`thoth.nudge { intent }`** — surfaces LESSONS.md entries whose
  trigger plausibly applies, and forces you to restate the plan naming
  each lesson you're honouring. Expand before Write/Edit when
  `gate_require_nudge = true`.
- **`thoth.reflect { summary, outcome? }`** — end-of-step reflection.
  Drives the fact/lesson decision.
- **`thoth.grounding_check { claim }`** — verify a factual claim
  against the indexed graph before asserting it.

## Discipline modes

From `<root>/config.toml` `[discipline]`:

- `mode = "off"` — no enforcement.
- `mode = "nudge"` (default) — warn the user if a step was skipped.
- `mode = "strict"` — PreToolUse gate blocks Write/Edit/Bash unless a
  `thoth_recall` was logged within `gate_window_short_secs` (default
  180s) and scored ≥ `gate_relevance_threshold` against the upcoming
  tool's args.

If the gate blocks, the stderr message tells you what to do. Call
`thoth_recall` with a query matching the intended action, then retry.

## CLI parity

Every MCP tool has a CLI equivalent for headless use:

| CLI                                      | MCP tool                 |
| ---------------------------------------- | ------------------------ |
| `thoth query <text>`                     | `thoth_recall`           |
| `thoth index [path]`                     | `thoth_index`            |
| `thoth impact <fqn> [--direction]`       | `thoth_impact`           |
| `thoth context <fqn>`                    | `thoth_symbol_context`   |
| `thoth changes [--from <file\|->]`       | `thoth_detect_changes`   |
| `thoth memory show \| log \| forget`     | `thoth_memory_*`         |
| `thoth skills list \| install`           | `thoth_skills_list`      |

See the `thoth-cli` skill for the full command tree.
