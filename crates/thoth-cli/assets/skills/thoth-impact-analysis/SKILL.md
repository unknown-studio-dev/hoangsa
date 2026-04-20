---
name: thoth-impact-analysis
description: >
  Use when the user wants to know what will break if they change
  something, or needs safety analysis before editing code. Examples:
  "is it safe to change X?", "what depends on this?", "what will
  break?", "show me the blast radius". Also compose with PR review —
  run this per touched symbol after `gh pr diff`.
metadata:
  version: "0.0.1"
---

# Impact Analysis with Thoth

Answer "what breaks if I change X?" using the code graph. This is the
one thing the graph does that grep + IDE-nav cannot: it traverses
`calls`, `references`, and `extends` edges across files, packages, and
languages in one query.

## When to Use

- "Is it safe to change this function?"
- "What will break if I modify X?"
- "Who uses this code?"
- Before any rename / signature change / semantic tweak.
- Before committing — cheap sanity check on blast radius.
- As one step in a PR review (see §PR review compose below).

## Workflow

```
1. thoth_recall({query: "<name or concept>"})        → find the exact FQN
2. thoth_impact({fqn, direction: "up", depth: 3})    → who calls / references
3. (optional) thoth_symbol_context({fqn})            → full 360° on target
4. Quote chunk ids in your answer                    → prove grounding
```

### 1. Pin the FQN

Graph keys are `module::symbol` (e.g. `server::dispatch_tool`). If the
user gave you a bare name, run `thoth_recall` first to disambiguate —
otherwise `thoth_impact` returns `symbol not found`.

### 2. Walk the blast radius

```
thoth_impact {
  fqn: "server::dispatch_tool",
  direction: "up",       // "up" | "down" | "both"
  depth: 3               // 1..8
}
```

- **`up`** (default) — callers, referencers, subtypes. **Answers "what
  breaks"**. Use this 90% of the time.
- **`down`** — callees, parent types. Use when you want "what does this
  depend on".
- **`both`** — union. Expensive; only when you're mapping a whole
  subsystem.

Results are grouped by BFS depth. Depth 1 is direct dependents; deeper
rings are transitive and usually less critical. Quote the depth-1 ring
in your answer — that's what actually breaks on change.

### 3. Read the context (optional)

If you're about to edit the symbol, also pull `thoth_symbol_context
{fqn}` to see siblings (other symbols in the same file you might
accidentally touch) and unresolved imports (external deps you can't
mock).

### 4. Decide + cite

Answer in this shape:

> Changing `X` affects N callers at depth 1: `A::f`, `B::g`, `C::h`.
> Transitive reach is M symbols across `depth ≤ 3`. Safe to change if
> the signature is preserved; breaking signature changes need
> follow-up edits in [list].

Always cite the chunk ids from the recall output. If the impact set is
large, call out the top 3–5 most load-bearing callers (tests, public
API, cross-crate edges).

## PR review compose

During PR review, pipe the diff through `thoth_detect_changes`:

```
1. gh pr diff <n>                                    → unified diff
2. thoth_detect_changes({diff, depth: 2})            → touched symbols + upstream
3. For each high-risk symbol: thoth_impact({..})    → deeper walk
4. Flag: missing tests, public API breaks, cross-crate edges
```

`thoth_detect_changes` parses the diff, maps hunks to graph nodes, and
returns their upstream callers in one call — cheaper than running
`thoth_impact` per symbol. Use it as the first pass, then drill in
with `thoth_impact` on anything risky.

Compose with `pr-review-toolkit:review-pr` (or `code-reviewer` /
`silent-failure-hunter`) if installed — Thoth covers the **graph** side
of PR review (blast radius, orphan symbols, test coverage gaps on
touched callers); the toolkit covers style, error handling, and type
design.

## Anti-patterns

- **Skipping the recall step.** Running `thoth_impact` on a guessed
  FQN wastes a round-trip when `symbol not found` comes back. Always
  recall first unless you literally copied the FQN out of prior output.
- **Mistaking depth-5 transitive reach for real risk.** Most changes
  don't propagate past depth 2. Lead with the depth-1 ring.
- **Using `direction: both` by default.** It's 2–5× the noise. Only
  reach for it when you genuinely want both directions.
- **Forgetting references.** `thoth_impact direction: up` includes
  non-call references (type refs, trait bounds, generic params). A
  symbol with 0 callers but 20 references is still load-bearing.

## When the graph is wrong

The graph reflects the last `thoth index .` run. If the repo has been
edited since, callers may be stale. Signs: a caller in the result that
the user insists doesn't exist any more. Remedy: `thoth index .` or
`thoth watch .` (which re-indexes on save), then retry.

Symbols added in an **unsaved** edit won't be in the graph at all —
that's why impact analysis is a *pre-edit* tool, not a post-edit one.
