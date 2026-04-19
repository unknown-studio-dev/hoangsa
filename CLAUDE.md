<!-- thoth:managed:start -->
## Thoth memory (managed by `thoth setup` — edits inside this block are overwritten)

This project uses **Thoth MCP** as its long-term memory. Initialized on 2026-04-19.

### Memory workflow

- Persist facts via `thoth_remember_fact({text, tags?})` → `./.thoth/MEMORY.md`.
- Persist lessons via `thoth_remember_lesson({trigger, advice})` → `./.thoth/LESSONS.md`.
- Before every Write / Edit / Bash: call `thoth_recall({query})` at least once.
- The `UserPromptSubmit` hook auto-recalls for context but passes `log_event: false`, so that ceremonial recall does NOT satisfy the `thoth-gate` PreToolUse gate — only agent-initiated recalls do.
- Browse raw memory without tool calls: open `./.thoth/MEMORY.md` and `./.thoth/LESSONS.md`.
- Remove this block and all Thoth wiring: `thoth uninstall`.

### Code intelligence tools

| Tool | Params | Purpose |
|------|--------|---------|
| `thoth_recall` | `query`, `top_k?` (default 8) | Hybrid search (symbol + BM25 + graph + markdown) |
| `thoth_impact` | `fqn`, `direction?` (up/down/both), `depth?` (1-8) | Blast radius — who breaks if this symbol changes |
| `thoth_symbol_context` | `fqn`, `limit?` (default 32) | 360° view: callers, callees, imports, siblings, doc |
| `thoth_detect_changes` | `diff` (git diff output), `depth?` (1-6) | Find symbols touched by a diff + their blast radius |
| `thoth_index` | `path?` (default ".") | Reindex source tree |

### Before editing code

1. **MUST** run `thoth_impact({fqn: "module::symbol"})` before modifying any function/class.
2. Report blast radius (direct callers at d=1, indirect at d=2+) to the user.
3. **MUST** warn the user if d=1 impact includes critical paths before proceeding.

### Before committing

Run `thoth_detect_changes({diff: "<git diff output>"})` to verify changes only affect expected symbols.

### Memory maintenance

- `thoth review` — periodic background curation; the PostToolUse hook spawns it automatically when `background_review = true` (every `background_review_interval` mutations, subject to `background_review_min_secs` cooldown). Appends new insights; never deletes. Uses `background_review_model` (default `claude-haiku-4-5`) to avoid burning Opus tokens on a curator task.
- `thoth compact` — LLM-driven consolidation of `MEMORY.md` / `LESSONS.md`. Reads every entry, merges reworded near-duplicates into canonical form, **rewrites** both files (with `.bak-<unix>` backups). Run `thoth compact --dry-run` first to eyeball the proposal. Use when the files feel bloated with restatements of the same subject. Reuses the review backend/model config — no extra setup.

### Available skills

Use `/skill-name` to invoke: `thoth-exploring` (understand code), `thoth-debugging` (trace bugs), `thoth-refactoring` (safe renames/moves), `thoth-impact-analysis` (blast radius), `thoth-reflect` (end-of-session lessons), `thoth-guide` (Thoth help).
<!-- thoth:managed:end -->

## HOANGSA auto-compact

HOANGSA runs `thoth compact` in the background via a `PostToolUse` hook (`hoangsa-cli hook compact-check`). After every `auto_compact_interval` mutations (default 500) and at least `auto_compact_cooldown_secs` seconds (default 86400) since the last run, it spawns a detached `thoth compact` to merge near-duplicate facts/lessons in `MEMORY.md` + `LESSONS.md`. Output logs to `.hoangsa/state/compact-check.log`. Tune or disable via `hoangsa-cli pref set . auto_compact false` (or `auto_compact_interval` / `auto_compact_cooldown_secs`).

<!-- thoth:code-intel:start -->
# Thoth — Code Intelligence

This project is indexed by **Thoth** (scoped via `.thoth/` directory — no repo parameter needed). Use Thoth MCP tools to understand code, assess impact, and navigate safely.

> Thoth indexes automatically via file watcher. If needed, run `thoth index` manually for an immediate reindex.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `thoth_impact({target: "symbolName"})` and report the blast radius (direct callers, affected flows, risk level) to the user.
- **MUST run `thoth_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `thoth_recall({query: "concept"})` to find relevant code and flows instead of grepping.
- When you need full context on a specific symbol — callers, callees, which flows it participates in — use `thoth_symbol_context({name: "symbolName"})`.

## When Debugging

1. `thoth_recall({query: "<error or symptom>"})` — find code and flows related to the issue
2. `thoth_symbol_context({name: "<suspect function>"})` — see all callers, callees, and participation
3. For regressions: `thoth_detect_changes({scope: "compare", base_ref: "main"})` — see what your branch changed

## When Refactoring

- **Renaming**: Run `thoth_symbol_context({name: "oldName"})` to find all references, then use Grep to confirm text occurrences before renaming across files.
- **Extracting/Splitting**: Run `thoth_symbol_context({name: "target"})` to see all incoming/outgoing refs, then `thoth_impact({target: "target"})` to find all external callers before moving code.
- After any refactor: run `thoth_detect_changes()` to verify only expected files changed.

## Never Do

- NEVER edit a function, class, or method without first running `thoth_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with blind find-and-replace — use `thoth_symbol_context` + Grep to understand the full reference graph first.
- NEVER commit changes without running `thoth_detect_changes()` to check affected scope.

## Tools Quick Reference

| Tool | When to use | Command |
|------|-------------|---------|
| `recall` | Find code by concept | `thoth_recall({query: "auth validation"})` |
| `symbol_context` | 360-degree view of one symbol | `thoth_symbol_context({name: "validateUser"})` |
| `impact` | Blast radius before editing | `thoth_impact({target: "X"})` |
| `detect_changes` | Pre-commit scope check | `thoth_detect_changes()` |

## Impact Risk Levels

| Depth | Meaning | Action |
|-------|---------|--------|
| d=1 | WILL BREAK — direct callers/importers | MUST update these |
| d=2 | LIKELY AFFECTED — indirect deps | Should test |
| d=3 | MAY NEED TESTING — transitive | Test if critical path |

## Self-Check Before Finishing

Before completing any code modification task, verify:
1. `thoth_impact` was run for all modified symbols
2. No HIGH/CRITICAL risk warnings were ignored
3. `thoth_detect_changes()` confirms changes match expected scope
4. All d=1 (WILL BREAK) dependents were updated

## CLI

- Index: `thoth --json index`
- Watch for changes: `thoth watch .`
- Query: `thoth --json query "<concept>"`
- Impact: `thoth --json impact "<symbol>"`
- Context: `thoth --json context "<symbol>"`
- Changes: `thoth --json changes`
<!-- thoth:code-intel:end -->
