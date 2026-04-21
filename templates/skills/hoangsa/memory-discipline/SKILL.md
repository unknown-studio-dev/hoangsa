---
name: memory-discipline
description: >
  This skill should be used before any non-trivial coding action — editing
  code, writing new files, running migrations, deploying, or answering a
  question that involves factual claims about this codebase. It forces the
  agent to consult Thoth's persistent memory (USER.md, MEMORY.md,
  LESSONS.md, the indexed code graph) and acknowledge relevant lessons
  before acting. Trigger phrases: "edit", "refactor", "implement",
  "fix the bug in", "add a feature", "deploy", "why does this do",
  "how does X work".
metadata:
  version: "0.1.0"
---

# Memory Discipline

You are coding inside a repository that has a Thoth memory server attached
via MCP. That server gives you four things you MUST use before taking any
load-bearing action:

1. **Indexed code graph** — via `thoth_recall` (hybrid BM25 + symbol +
   vector search over the tree).
2. **User preferences** — `.thoth/USER.md`, first-person style + workflow
   choices that apply across projects.
3. **Project facts** — `.thoth/MEMORY.md`, durable invariants about this
   codebase.
4. **Reflective lessons** — `.thoth/LESSONS.md`, action-triggered advice.

USER.md + MEMORY.md + LESSONS.md are injected verbatim at SessionStart, so
you already have them in context. The `thoth_recall` hit extends that with
relevant code chunks.

Skipping this loop causes drift. Past sessions spent hours fixing
hallucinated APIs, re-learning patterns already documented in LESSONS.md,
or refactoring against conventions they never checked. Don't repeat them.

## The loop

Run this before writing code or asserting a non-obvious fact:

### 1. Recall

Call `thoth_recall` with a query derived from the user's intent. If the
intent is "add a retry wrapper around the HTTP client", recall
`"http client retry"`. Prefer nouns from the user's request over verbs.

Read the returned chunks. Every chunk has a `path:line-span` you can cite.

### 2. Honour USER.md + LESSONS.md

USER.md and LESSONS.md were already injected at SessionStart. Before
acting, scan them for:

- **USER.md** entries that shape HOW you respond (tone, language, commit
  style, testing preferences). Apply them without being asked.
- **LESSONS.md** triggers that match your planned action. Restate the
  relevant lessons before proceeding — if a lesson advises against your
  plan, stop and ask the user.

### 3. Act

Proceed only after honouring preferences + lessons. If you edit code,
quote the recalled chunk ids you relied on.

### 4. Reflect

After the action completes (tests pass/fail, file saved, command run),
decide what to persist. Three surfaces, three tools:

- **Preference** (`thoth_remember_preference`) — first-person, stable
  across projects ("user prefers Vietnamese responses", "user runs
  `make test` not `cargo test`"). Writes to USER.md.
- **Fact** (`thoth_remember_fact`) — project-specific invariant
  ("HTTP retry lives in crates/net/retry.rs"). Writes to MEMORY.md.
- **Lesson** (`thoth_remember_lesson`) — action-triggered advice
  ("when adding a retry → use RetryPolicy, not reqwest middleware").
  Writes to LESSONS.md.

Be conservative — only save memory that is specific, durable, and
non-obvious.

If the outcome was a success that validates a lesson you followed, call
`thoth_lesson_outcome { signal: "success", triggers: [...] }` with the
triggers of the lessons you honoured. On failure, call it with
`signal: "failure"`. This bumps confidence counters so stale advice
eventually dies.

## Handling `cap_exceeded`

All three `remember_*` tools return a structured error when the write
would exceed `[memory].cap_*_bytes`. The error JSON has this shape:

```json
{
  "code": "cap_exceeded",
  "kind": "fact",
  "current_bytes": 13784,
  "cap_bytes": 16384,
  "attempted_bytes": 14200,
  "hint": "Call thoth_memory_replace or thoth_memory_remove to free space, then retry.",
  "preview": [
    {"index": 0, "first_line": "...", "bytes": 396, "tags": [...]}
  ]
}
```

When you see this, do NOT append to a sibling file or silently drop the
new memory. Instead:

1. Read the `preview` list. Each entry has `index`, `first_line`, and
   `bytes`.
2. Pick the entry(s) to consolidate or drop — prefer dropping stale
   session-handoff / bare-SHA / outdated entries over real invariants.
3. Call `thoth_memory_replace { kind, query, new_text }` to consolidate
   (merges the new memory into an existing entry), or
   `thoth_memory_remove { kind, query }` to free space outright.
4. Retry the original `remember_*` call.

For bulk cleanup of a legacy MEMORY.md / LESSONS.md that accumulated
pre-cap entries, run `thoth memory migrate --llm` from the shell —
classifier triages every entry as keep / move-to-USER.md / drop, then
applies via the same replace/remove verbs.

## Anti-hallucination rules

- **Never assert a name, signature, or behaviour without a recall hit.**
  If `thoth_recall` returns nothing relevant, say so explicitly: "I can't
  find that in the indexed code — can you point me at it?"
- **Quote chunk ids.** Citations look like `[chunk-id]` in your answer;
  the Thoth server uses them to validate that you grounded the response.
- **Bail on deny.** If a LESSONS.md trigger applies and the advice is
  "don't do X", and your plan is X, stop and ask the user.

## When NOT to run the loop

Skip the loop only for:

- pure conversation (no tool calls),
- read-only questions about files the user explicitly pasted,
- trivial one-line comment or typo fixes.

For everything else, run the loop. It takes ~5 seconds of tool calls and
saves hours of rework.

## Configuration

The enforcement level comes from `<root>/config.toml` under
`[discipline]` and `[memory]`:

- `mode = "nudge"` (default) — warn the user if you skip a step.
- `mode = "strict"` — a PreToolUse gate hook will **block** every `Write`,
  `Edit`, `NotebookEdit`, and `Bash` tool call unless a `thoth_recall` was
  logged within the gate window. The block response tells you exactly
  what to do: run `thoth_recall`, then retry.
- `gate_window_short_secs = 60` — recency shortcut; any recall within
  this window passes without a relevance check.
- `gate_window_long_secs = 1800` — relevance pool; the gate looks this
  far back for a topically-matching recall when recency alone fails.
- `gate_relevance_threshold = 0.30` — containment-based match threshold.
- `memory_mode = "auto"` (default) or `"review"`. See below.
- `cap_memory_bytes = 16384` — hard cap for MEMORY.md.
- `cap_user_bytes = 4096` — hard cap for USER.md.
- `cap_lessons_bytes = 16384` — hard cap for LESSONS.md.
- `strict_content_policy = false` — when true, ephemeral-looking inputs
  (session-handoff prose, bare SHAs, date-only entries) are rejected at
  the `remember_*` entry point instead of just warning.

If the project has no `.thoth/` directory and `global_fallback = true`
(the default), fall back to `~/.thoth/` memory. If neither exists, the
gate falls open (approves) with a warning and asks you to run
`thoth index .` — it never bricks the editor.

## Memory modes: `auto` vs `review`

When you call `thoth_remember_*`, the server honours `memory_mode`:

- **`auto`** — the entry is appended straight to its target file.
  Fastest. Relies on the forget pass + confidence counters to prune bad
  memory later. Good for solo use.
- **`review`** — the entry is appended to a `*.pending.md` sibling. The
  user must run `thoth memory promote <kind> <index>` (or call
  `thoth_memory_promote`) to accept. Rejected entries are archived with
  a reason in `memory-history.jsonl`. Good for teams.

Even in `auto` mode, the server refuses to silently **overwrite** an
existing lesson — if a `trigger` already exists, the new lesson is
staged and flagged with `"conflict": {...}` in the tool output. When you
see a conflict, do NOT try to auto-promote: flag it to the user via
`thoth_request_review` and let them decide.

## Audit log

Every memory mutation lands in `.thoth/memory-history.jsonl` (one JSON
per line) with `op`, `kind`, `title`, `actor`, `reason`, and a timestamp.
Ops include: `append`, `replace`, `remove`, `stage`, `promote`, `reject`,
`quarantine`, `propose`, `request_review`. Inspect with
`thoth memory log --limit 50`. This log is size-capped and
self-truncates — old entries past the session window are intentionally
shed since reflection debt counts from `.session-start` anyway.

## Proposing new skills

When you've hit the same pattern in ≥5 lessons, consolidate them into a
reusable skill via `thoth_skill_propose`:

- `slug`: kebab-case directory name.
- `body`: full SKILL.md text starting with `---\nname: ...` frontmatter.
- `source_triggers`: the triggers of the lessons being consolidated.

The draft lands at `.thoth/skills/<slug>.draft/SKILL.md` and an entry is
written to the history log. The user promotes the draft via `thoth
skills install .thoth/skills/<slug>.draft` once they've reviewed it.

Drafts are NOT auto-installed — a human must review before a proposed
skill starts shaping future sessions. This is the main guardrail against
runaway self-modification.

## Why the strict gate exists

Prompts alone are bypassable — a self-confident agent can talk itself
into skipping the recall step. Strict mode trips a **ground-truth** check
against `<root>/episodes.db`: every `thoth_recall` call writes a
`query_issued` event, and the `thoth-gate` binary (a small Rust executable
shipped alongside `thoth-mcp`) queries SQLite directly. If no recent event
exists, the tool call is blocked at the hook level — the agent never gets
to rationalise its way past it. You see:

```json
{"decision": "block",
 "reason": "Thoth discipline: no `thoth_recall` has been logged ..."}
```

Treat that as non-negotiable: call `thoth_recall`, read the chunks, then
retry the original tool call.
