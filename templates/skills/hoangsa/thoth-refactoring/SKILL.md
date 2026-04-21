---
name: thoth-refactoring
description: >
  Use when the user wants to rename, extract, split, move, or
  restructure code safely. Examples: "rename this function", "extract
  this into a module", "split this class", "move this to a separate
  file", "refactor this service".
metadata:
  version: "0.0.1"
---

# Refactoring with Thoth

Refactors fail when a caller you didn't know about gets out of sync.
The graph exposes every caller, reference, and subtype before you
touch the code — so the edit plan is grounded, not guessed.

## When to Use

- "Rename this function safely"
- "Extract this into a module"
- "Split this service"
- "Move this to a new file"
- Any rename / extract / move / signature change across files.

## Workflow

```
1. thoth_recall({query: "<target concept>"})         → find exact FQN
2. thoth_impact({fqn, direction: "up", depth: 3})    → every dependent
3. thoth_symbol_context({fqn})                        → siblings + extends
4. Draft the edit plan, naming every touched file
5. Apply edits in order: callees first, then callers
6. Re-index if needed: thoth_index
7. Verify: repeat thoth_impact, confirm 0 stale refs
8. thoth_lesson_outcome on the triggers you followed
```

### 1. Pin the FQN

Use `thoth_recall` to disambiguate the target. Refactors on wrong
symbols are the most expensive mistake — double-check the FQN matches
what the user meant.

### 2. Map the blast radius

```
thoth_impact { fqn: "auth::verify_token", direction: "up", depth: 3 }
```

Count the depth-1 ring. That's the minimum number of sites you must
edit (rename) or that will break (signature change). Depths 2–3 break
only if the signature change propagates through wrappers.

If `direction: "up"` returns **0 dependents**, the symbol may be dead
code — flag that to the user before refactoring ("nothing references
this; did you mean to delete it?").

### 3. Pull sibling context

```
thoth_symbol_context { fqn: "auth::verify_token", limit: 32 }
```

Look for:

- **`siblings`** — sharing the file. A module-level rename may pull
  them along; make sure you don't accidentally split what belongs
  together.
- **`extends` / `extended_by`** — if you're changing a trait or base
  class, every subtype needs the matching edit.
- **`references`** — non-call uses (type annotations, generic bounds,
  macro args). These are easy to miss with grep; the graph has them.

### 4. Draft the edit plan

Before editing, list every file you'll touch. Use the output of steps
2+3 as the checklist. Share the plan with the user if the radius is
> 5 files or crosses crate boundaries — refactor scope is the #1
thing users want to veto early.

### 5. Apply in order

- **Rename** — apply at the definition, then every dependent. Order
  matters: if tests reference the symbol, update tests too or they'll
  fail to compile.
- **Extract** — create the new module first, move the body, then
  update the original site to re-export or call into the new module.
  Re-export for one session keeps the old API alive during migration.
- **Move** — update `mod` / `use` / `import` statements along with
  the actual move.
- **Signature change** — update callees first (so the new signature
  compiles at the definition), then callers (to match).

Apply one file at a time — don't rely on one big diff to land atomically.

### 6. Re-index

After the edits, the graph reflects the *old* code. Run:

```
thoth_index { path: "." }
```

(Or `thoth index .` via CLI. If `thoth watch` is running, it already
re-indexed on save — skip this step.)

### 7. Verify zero stale refs

Re-run `thoth_impact` on the **old** FQN. It should error with
`symbol not found` — that's the success signal (the old name is
gone). Then run `thoth_impact` on the **new** FQN and confirm the
dependent count matches what you expected from step 2.

Run the test suite. If it passes, the refactor held.

### 8. Reflect

If a lesson guided the refactor (e.g. "use `RetryPolicy` not reqwest
middleware"), call:

```
thoth_lesson_outcome {
  signal: "success",
  triggers: ["adding a retry to an HTTP call"]
}
```

If the refactor surfaced a recurring pattern (e.g. "this repo never
changes a public trait without adding a blanket impl"), persist it:

```
thoth_remember_lesson {
  trigger: "renaming a method on a public trait in this repo",
  advice: "Add a default impl calling the new name that delegates to
           the old name for one release; remove after N+1."
}
```

## Anti-patterns

- **Skipping impact.** Renaming blind is how you ship a broken PR.
  Always run `thoth_impact` first, even for "tiny" renames.
- **Treating depth-3 hits as depth-1.** The direct ring is what
  breaks on rename. Deep transitive hits only break on signature
  changes.
- **Assuming the graph is fresh.** If you've edited in this session,
  re-index or `thoth_impact` will omit new callers and include
  deleted ones.
- **Batching unrelated refactors.** One rename per commit. Users who
  see a 40-file diff can't tell signal from noise.

## When the radius is huge

If `thoth_impact depth: 1` returns > 30 dependents, stop and reconsider.

- **Pattern: deprecate + migrate.** Add the new API alongside the old,
  migrate callers in batches, delete the old. Safer than a single big
  diff.
- **Pattern: add a shim.** The new impl lives in a new name; the old
  name becomes a one-liner `pub fn old() -> New { new(…) }` that can
  be deprecated in a follow-up.

Flag these to the user before starting — they often change their mind
when they see the radius.
