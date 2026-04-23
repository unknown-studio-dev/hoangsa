# Snippet: memory reflect at end

Included via `Read $HOANGSA_ROOT/templates/snippets/memory-reflect-end.md` at
the end of a workflow that completed real work (commit, PR, fix, refactor).
The goal is to persist **durable, non-obvious** learnings — not a summary of
what just happened.

## Decide first: is anything worth remembering?

Ask three quick questions. If all three are **no**, skip this step entirely.

1. Did the user correct your approach in a way that should apply to future
   sessions? → candidate for `memory_remember_preference`.
2. Did we uncover an invariant about this codebase that was not obvious from
   reading one file? (e.g., "HTTP retry lives in `crates/net/retry.rs`";
   "migrations must run before `cargo sqlx prepare`") → candidate for
   `memory_remember_fact`.
3. Did we learn a rule that only fires in a specific situation? (e.g., "when
   editing migrations, run `sqlx prepare` after") → candidate for
   `memory_remember_lesson`.

If unsure, do **not** save. Memory pollution is worse than a missed save.

## What to do for each candidate

- `memory_remember_fact({ text, tags })` — project invariants → `MEMORY.md`.
  `text` is one sentence; `tags` is a short list (e.g. `["http", "retry"]`).

- `memory_remember_lesson({ trigger, advice })` — action-triggered advice →
  `LESSONS.md`. `trigger` describes **when** the advice applies; `advice`
  describes **what to do**. Both must be concrete.

- `memory_remember_preference({ text })` — first-person, cross-project workflow
  choice → `USER.md`. Only when the user expressed a preference, not when you
  inferred one.

## Rules

- At most 1 save per category per workflow run. If you want to save 3 facts,
  pick the most durable one.
- Never paraphrase a "save" into a multi-paragraph essay. Each entry is one
  sentence of rule + one sentence of why.
- If `hoangsa-memory` is not installed, skip silently.
