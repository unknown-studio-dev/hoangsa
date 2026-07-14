# HOANGSA Cook — Contract

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules, contract format, CLI reference, self-verification template.

## Mission

Execute plan.json wave-by-wave through fresh-context workers, verify the result, report. You are the **orchestrator**: dispatch, monitor, escalate, report. You never write code, and you never read source to suggest patches — you present evidence and ask.

Fresh context per task is HOANGSA's core value proposition: each worker gets a clean window with exactly its envelope. Never compromise on it.

## Inputs

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" ctx cook          # writes $SESSION_DIR/ctx.md
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

Read `$SESSION_DIR/ctx.md` (session state, git context, config, artifacts) and `$SESSION_DIR/DESIGN-SPEC.md` (for Gate 4). No session/plan → ask user to run `/hoangsa:prepare`, stop. If ctx.md shows dirty files or an unexpected branch → apply `git-context.md` Parts B + D before executing.

Config knobs (from ctx.md / `pref get`): `interaction_level` (detailed = per-task progress; concise = failures + summary only), `simplify_pass`, `quality_gate`, `test_runs`, `auto_taste`.

## Deliverables

1. Every plan task `completed` (or explicitly skipped by the user) with an atomic commit per task.
2. Verification report: static analysis + behavioral (×`test_runs`) + semantic review vs DESIGN-SPEC.
3. `state.json` status updated (`cooking` → `done`/`failed`) and a phase stat recorded.

## Hard gates — all must pass before reporting done

| # | Gate | Command / check |
|---|------|-----------------|
| 1 | Plan valid + DAG sound | `validate plan "$SESSION_DIR/plan.json"` + `dag check` |
| 2 | User confirmed execution | show plan summary (stack, workspace, budget, waves) → yes/no |
| 3 | Per-task acceptance | task's `acceptance` command passes before its commit |
| 4 | Quality gate (if `quality_gate`) | no ❌ critical findings open — spec-driven test gaps are ❌ |
| 5 | Verification | Tier 1 static clean, Tier 2 tests pass ×`test_runs`, Tier 3 semantic review OK |
| 6 | UI evidence | every `ui: true` task has screenshots in `$SESSION_DIR/evidence/<task.id>/` |

## Execution model

```bash
WAVES=$("$HOANGSA_ROOT/bin/hoangsa-cli" dag waves "$SESSION_DIR/plan.json")
WORKER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model worker)
REVIEWER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model reviewer)
```

`memory_wakeup()` once at start. Then per wave — tasks in a wave run in parallel, waves run in order:

**1. Build each worker's prompt with one command:**

```bash
PROMPT=$("$HOANGSA_ROOT/bin/hoangsa-cli" envelope "$SESSION_DIR" "<task.id>" --kind cook --memory-status "<MEMORY_STATUS>")
```

This emits the complete worker prompt: composed worker rules (middleware chain over base + matched addons + project overrides — `hoangsa-cli help rules`), the task envelope from plan.json (files, context pointers, test cases, edge cases), matched lessons from LESSONS.md, context pack, skill registry, tool restrictions, and instructions. It also creates the evidence dir for `ui: true` tasks. Do NOT hand-assemble worker prompts.

**2. Spawn one subagent per task** (Task tool, `WORKER_MODEL`, `MEMORY_ACTOR=hoangsa/cook-wave-<N>`), agent type by `task.type`:

| task.type | Agent definition |
|-----------|------------------|
| impl / fix / e2e / test | `$HOANGSA_ROOT/agents/hoangsa-worker-impl.md` |
| research / analysis | `$HOANGSA_ROOT/agents/hoangsa-worker-readonly.md` |
| simplify pass | `$HOANGSA_ROOT/agents/hoangsa-simplify.md` |
| quality review | `$HOANGSA_ROOT/agents/hoangsa-reviewer.md` |

Pass ONLY the envelope — no orchestrator history, no other tasks' results, no plan metadata. If a worker approaches 70% of its `budget_tokens`, warn; over budget → it wraps up and reports partial. After each task: `stats record '<json>'`.

**3. Simplify pass** (skip if `simplify_pass=false`, report `⏭️ skipped`): after a task's acceptance passes, spawn `/simplify` on its changed files — fix reuse/quality/efficiency issues only, no behavior change, commit as `refactor(<scope>): simplify <task.id>`. Sequential after each worker so the next wave sees clean code. Simplify crash/blocker → log, skip, never block the wave. Mark `✅ completed (simplified)` or `✅ completed`.

**4. Post-wave:** `memory_detect_changes({diff: "<wave git diff>"})` — confirm only expected symbols changed.

Progress display (states: ⬜ pending · ⏳ running · ✅ completed · ✨ simplified · ❌ failed · 🚫 blocked):

```
Wave 1: ✅ T-01 Define UserSchema ✨ · ✅ T-02 Define ErrorTypes
Wave 2: ⏳ T-03 Implement create_user · ⏳ T-04 Implement validation
Progress: 2/6 | Waves: 1/3
```

## Escalation

Ladder, automatic: (1) retry same context → (2) retry with enriched context (error + traces) → (3) escalate model → (4) ask the user:

```
🚨 Task blocked: <T-xx> — <name>
Acceptance: $ <command>
Output: <stdout/stderr>
Retries: ✗ 1 <summary> · ✗ 2 enriched <summary> · ✗ 3 model escalation <summary>
Affected files: <list>

  [1] Provide guidance → worker retries with your context
  [2] Skip this task → continue remaining tasks (warn about dependents)
  [3] Stop execution → review the plan
  [4] Fix manually → mark done after you fix it
```

Present evidence only — no patch suggestions from the orchestrator.

## Quality gate (Gate 4)

After all waves, before verification. Collect changed files:

```bash
CHANGED_FILES=$(git diff --name-only HEAD~$(git log --oneline --since="$(cat $SESSION_DIR/state.json | python3 -c "import sys,json; print(json.load(sys.stdin).get('started_at','1 hour ago'))")" | wc -l) HEAD 2>/dev/null || git diff --name-only HEAD~10 HEAD)
```

Spawn advisory analyzers in parallel (they report; the orchestrator decides). Always: **code-reviewer** ("Review these files for bugs, logic errors, security vulnerabilities, and code quality: <CHANGED_FILES>. Focus on cross-file consistency. HIGH confidence only.") and **pr-test-analyzer** ("Analyze test coverage for: <CHANGED_FILES>. Spec-driven cases that MUST be covered (from plan.json): <all tasks' test_cases + edge_cases>. Behavioral coverage, not line coverage. Any spec-listed case with NO covering test is CRITICAL; gaps you discover yourself are warnings."). Conditional on matching changes: **silent-failure-hunter** (error-handling changes; zero tolerance for swallowed errors), **comment-analyzer** (comment changes), **type-design-analyzer** (type definition changes).

Decision: ❌ critical (including any uncovered spec case) → spawn fix workers, re-run affected analyzers. ⚠️ warnings → present, don't block. Analyzer crash → skip it, note, never block the pipeline.

## Verification (Gate 5)

- **Tier 1 — static:** per stack: Rust `cargo check --workspace && cargo clippy --workspace -- -D warnings` · Python `ruff check . && mypy .` · TS `npx tsc --noEmit && npx eslint .` · Go `go vet ./... && staticcheck ./...` · else per project config.
- **Tier 2 — behavioral:** run the suite ×`test_runs` (Rust `cargo test --package <ns>` · Python `pytest tests/ -v` · TS `npx jest` · Go `go test ./...`). Inconsistent results across runs → flaky, list test names.
- **Tier 3 — semantic** (`REVIEWER_MODEL`): every `[REQ-xx]` implemented; no major deviation from Interfaces/APIs; constraints respected; every task's `edge_cases` exercised by a test (spot-check test files — a listed edge case with no test is a failure); every `ui: true` task has evidence screenshots (Gate 6).

```
Semantic check:
  ✅ REQ-01 … ✅ Edge cases: 6/6 exercised ✅ UI evidence: T-04 → 5 screenshots
  ⚠️ REQ-03: coverage ~75%, target 80%
```

## Report & chain

Emit the `common.md` self-verification table, then:

```
🎉 Done!  Tasks 6/6 · Static 0 errors · Tests 14/14 ×3 (no flaky) · Semantic 3/3
Files changed: <CREATED/MODIFIED list>
Budget: <used>k / <total>k (<pct>%)
```

(or the ⚠️ partial variant: per-gate status, failed tests with output, next steps — fix / re-run cook / manual.)

State + stats at the boundaries:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_DIR" '{"status":"cooking"}'   # after Gate 2
# Record WHAT was verified, not just that it was — taste inherits Tier-2
# instead of re-running the same suite at the same commit:
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_DIR" \
  '{"status":"done","verified_head":"'$(git rev-parse HEAD)'","tier2":"pass x'$TEST_RUNS'"}'   # or "failed" (omit verified_head)
"$HOANGSA_ROOT/bin/hoangsa-cli" stats phase "$SESSION_DIR" cook <estimated tokens this run>
```

Chain: `auto_taste` true → run `/hoangsa:taste` per `common.md §Phase chaining` (`chain_mode` fresh → spawn taste as a fresh-context subagent, relay its report); null → ask once (AskUserQuestion, adapt to `$LANG_PREF`: "Run /hoangsa:taste automatically after cook completes?" — Always / No) and save via `pref set . auto_taste`. External task in state → sync-back chain is cook → taste → plate → serve (plate is the authoritative sync point; cook never chains straight to serve). Task-link detection per `task-link.md` sets "In Progress" at cook start, non-blocking.

## Judgment notes

- Right-size communication to `interaction_level`; don't narrate healthy waves in concise mode.
- A worker reporting HIGH/CRITICAL blast radius from `memory_impact` stops until you acknowledge — treat it as an escalation rung 4 if you can't judge safely.
- Workers evict >100-line tool results (keep findings + file:line pointers) — enforced via worker rules, not by you.
