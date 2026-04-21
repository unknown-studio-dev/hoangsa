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
memory layer (`MEMORY.md`, `LESSONS.md`) and a PreToolUse discipline
gate.

## Always Start Here

For any non-trivial coding task:

1. Call `memory_recall` with a query derived from the user's intent. The
   `UserPromptSubmit` hook also recalls for context, but that ceremonial
   call does **not** satisfy the discipline gate — only agent-initiated
   recalls do.
2. Read the chunks. Each has `path:line-span` you can cite.
3. Match the task to one of the skills below and follow its workflow.
4. After acting, reflect via `memory.reflect` → persist fact/lesson if
   durable.

If `memory_recall` returns `(no matches — did you run memory_index?)`,
stop and run `hoangsa-memory index .` (CLI) or `memory_index` (MCP) before
continuing.

## Skills

| Skill                      | When to read it                                         |
| -------------------------- | ------------------------------------------------------- |
| `memory-discipline`        | Before any Write/Edit/Bash — enforces the recall loop.  |
| `memory-reflect`            | End of session / after a bug fix / "what did we learn". |
| `memory-exploring`          | "How does X work?" / architecture questions.            |
| `memory-debugging`          | "Why does this fail?" / tracing errors.                 |
| `memory-impact-analysis`    | "What breaks if I change X?" / pre-commit safety.       |
| `memory-refactoring`        | Rename / extract / move / restructure.                  |
| `memory-cli`                | Running `hoangsa-memory setup`, `hoangsa-memory index`, `hoangsa-memory eval`, …   |

## MCP tools

### Retrieval

- **`memory_recall { query, top_k?, log_event? }`** — hybrid recall
  (BM25 + symbol + vector + markdown). Default `log_event = true` —
  agent-initiated recalls must log; that's what the discipline gate
  checks.
- **`memory_symbol_context { fqn, limit? }`** — 360° view of a symbol:
  callers, callees, extends, extended_by, references, siblings,
  unresolved imports. Pure graph lookup keyed on exact FQN.
- **`memory_impact { fqn, direction?, depth? }`** — BFS blast radius.
  `direction` ∈ `up | down | both` (default `up`), `depth` ∈ `[1,8]`
  (default 3). `up` answers "what breaks if I change X?".
- **`memory_detect_changes { diff, depth? }`** — feed a unified diff
  (stdout of `git diff`), returns touched symbols + upstream callers
  per hunk. Designed for pre-commit / PR review.

### Memory (read)

- **`memory_show`** — dump current `MEMORY.md` + `LESSONS.md`.
- **`memory_pending`** — list staged facts/lessons awaiting
  promotion (only non-empty when `memory_mode = "review"` or on
  lesson-trigger conflicts).
- **`memory_history { limit? }`** — tail of
  `memory-history.jsonl` (stage/promote/reject/quarantine events).
- **Resources**: `resources/read` with URI `hoangsa-memory://memory/MEMORY.md`
  or `hoangsa-memory://memory/LESSONS.md` — same data, lighter wire shape.

### Memory (write)

- **`memory_remember_fact { text, tags?, stage? }`** — append a durable
  fact. Set `stage: true` if you're unsure — it lands in
  `MEMORY.pending.md` instead.
- **`memory_remember_lesson { trigger, advice, stage? }`** — append a
  reflective lesson. `trigger` is a situation description ("adding a
  retry to an HTTP call"), not a command. Conflicts with existing
  triggers auto-stage.
- **`memory_lesson_outcome { signal, triggers }`** — bump confidence
  counters. `signal` ∈ `success | failure`, `triggers` is the list of
  lessons that were in play. Call this after the outcome of an action
  guided by lessons.
- **`memory_forget`** — run the TTL sweep. Quarantines lessons
  whose failure ratio exceeds `quarantine_failure_ratio`.
- **`memory_promote { kind, index }`** / **`memory_reject
  { kind, index, reason? }`** — resolve pending entries.
- **`memory_request_review`** — flag an entry for the user to audit
  (writes to `memory-history.jsonl`).
- **`memory_episode_append { event }`** — raw episodic log entry.
  Normally hook-driven; agents rarely call this directly.
- **`memory_skill_propose { slug, body, source_triggers? }`** — draft a
  new skill from ≥5 related lessons. Lands in
  `.hoangsa-memory/skills/<slug>.draft/` for user review.
- **`memory_skills_list`** — enumerate installed skills.

## MCP prompts

Fetch via `prompts/get { name, arguments }`:

- **`memory.nudge { intent }`** — surfaces LESSONS.md entries whose
  trigger plausibly applies, and forces you to restate the plan naming
  each lesson you're honouring. Expand before Write/Edit when
  `gate_require_nudge = true`.
- **`memory.reflect { summary, outcome? }`** — end-of-step reflection.
  Drives the fact/lesson decision.
- **`memory.grounding_check { claim }`** — verify a factual claim
  against the indexed graph before asserting it.

## Discipline modes

From `<root>/config.toml` `[discipline]`:

- `mode = "off"` — no enforcement.
- `mode = "nudge"` (default) — warn the user if a step was skipped.
- `mode = "strict"` — PreToolUse gate blocks Write/Edit/Bash unless a
  `memory_recall` was logged within `gate_window_short_secs` (default
  180s) and scored ≥ `gate_relevance_threshold` against the upcoming
  tool's args.

If the gate blocks, the stderr message tells you what to do. Call
`memory_recall` with a query matching the intended action, then retry.

## CLI parity

Every MCP tool has a CLI equivalent for headless use:

| CLI                                      | MCP tool                 |
| ---------------------------------------- | ------------------------ |
| `hoangsa-memory query <text>`                     | `memory_recall`           |
| `hoangsa-memory index [path]`                     | `memory_index`            |
| `hoangsa-memory impact <fqn> [--direction]`       | `memory_impact`           |
| `hoangsa-memory context <fqn>`                    | `memory_symbol_context`   |
| `hoangsa-memory changes [--from <file\|->]`       | `memory_detect_changes`   |
| `hoangsa-memory memory show \| log \| forget`     | `memory_*`         |
| `hoangsa-memory skills list \| install`           | `memory_skills_list`      |

See the `memory-cli` skill for the full command tree.
