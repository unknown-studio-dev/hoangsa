# HOANGSA Taste — Contract

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules, contract format, CLI reference, self-verification template.

## Mission

Independently verify what cook produced: run every task's acceptance, judge test quality (fake tests, spec coverage), verify UI against the real running app. **Test only, never fix** — failures go to `/hoangsa:fix` in a fresh context; taste records, reports, and routes.

## Inputs

`session latest` → `$SESSION_DIR/plan.json` (tasks + acceptance commands). No session → send the user to `/hoangsa:prepare`, stop.

```bash
MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model tester)
INTERACTION=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . interaction_level)
CONFIG=$("$HOANGSA_ROOT/bin/hoangsa-cli" config get .)
```

`codebase.testing` / `codebase.packages[].test` are the fallback when a task's `acceptance` is missing or generic. `interaction_level`: detailed → full output per task; concise → pass/fail, expand failures only.

## Deliverables

1. Verdict per task, written back to plan.json (`"status": "passed" | "failed"`).
2. Failure report per failed task (exact command, full stdout/stderr) + routing choice from the user.
3. Chain decision (`auto_plate`), phase stat recorded.

## Hard gates — a task is `passed` only if ALL apply

| # | Gate | Check |
|---|------|-------|
| 1 | Acceptance | the task's `acceptance` command exits clean — or inherited from cook (below) |
| 2 | No fake tests | quality review below finds no FAIL pattern |
| 3 | Spec coverage | every `edge_cases` / `test_cases` entry from plan.json has a covering test — a missing edge-case test is **FAIL, not WARN** |
| 4 | UI evidence | `ui: true` → screenshots of the real app states exist and match spec |

Never mark a task failed→passed (or skip one) without the user's explicit say-so.

## Verification passes

**1. Acceptance per task** — run `acceptance`, record pass/fail with output.
**Inheritance (no double-run):** if state.json has `verified_head` equal to the
current `git rev-parse HEAD` and `tier2` starting with "pass", cook already
ran the full suite at this exact commit — inherit that as Gate 1 (record
`inherited from cook @ <head>` per task) and skip re-running acceptance.
HEAD differs, record missing, or tier2 not pass → run everything as usual.
Inheritance never covers passes 2–4: quality gate, change-aware targeting,
and visual verification are taste's own work — that's where the independence
lives, not in repeating identical commands at an identical commit.

**2. Change-aware targeting** — `memory_detect_changes({diff: "$(git diff main...HEAD)"})`: verify tests exercise the changed symbols themselves (not adjacent code); flag any changed symbol with d=1 dependents and zero coverage; feed into pass 3.

**3. Test Quality Gate** (per task that passed acceptance; prompt-based — read test + production files side by side, no analysis tools):
- *Fake-test patterns → FAIL:* test reproduces >3 lines of production logic verbatim; inline stubs where the framework has mocking utilities; assertions on hardcoded literals that never exercise production code; mocking the very function under test.
- *Spec coverage (Gate 3):* each `edge_cases` entry exercised with the same input and expected outcome; each `test_cases` entry exists and asserts its expected outcome.
- *Robustness (WARN unless spec-listed):* async cleanup/error/timeout paths, race conditions, promise rejection, listener removal.
- Outcomes: PASS → proceed · WARN → report, don't block · FAIL → re-mark the task `failed` in plan.json.

**4. Visual verification** (tasks flagged `"ui": true`; fallback for old plans: files touch components/screens/styles; skip the pass entirely when none):
1. Check `$SESSION_DIR/evidence/<task.id>/` for cook-time screenshots.
2. Missing → run it now: fe-testing skill flow 5 (run-and-observe) — boot the app, walk the TEST-SPEC Visual Verification states (empty / loading / error / success / disabled / overflow / responsive), capture into the evidence dir.
3. Read each screenshot against DESIGN-SPEC + the Visual Verification table: states present, spacing/alignment per spec, long text behaves, nothing clipped.
4. No observable evidence, or a state unreachable → the task is `failed` exactly like a failing acceptance command — "compiles and unit tests pass" is not UI verification. Record WHY (which states lack evidence, which screens deviate) — `/hoangsa:fix` inherits this detail.

```
[T-04] Profile form   Evidence: 5 screenshots (empty/loading/error/success/overflow)   ✅ verified
[T-05] Settings       Evidence: none — "error" state unreachable                        ❌ FAIL
```

## Report & route

Per failed task: `❌ <T-xx> <name>` + acceptance command + full stdout/stderr (never summarize error output). Then:

```
<N> task(s) failed. What would you like to do?
  [1] /hoangsa:fix  — hotfix the failing task(s)
  [2] Fix manually  — mark as passed after you fix it
  [3] Skip          — mark as failed, move on
```

Update plan.json statuses (read-modify-write each task's `status`). Summary: per-task verdict table + `Summary: N/M passed` + next steps (plate / fix / check).

**Skill proposal** (hoangsa-memory available): a topic cluster of ≥5 lessons in LESSONS.md with success signals → `memory_skill_propose` with `source_triggers`, report the draft path to the user. Skip when no lessons touch the tested modules.

**Chain:** `auto_plate` true → `/hoangsa:plate` per `common.md §Phase chaining`; false → show next steps; null → ask once (AskUserQuestion, adapt to `lang`: "Auto-commit after taste passes?" — Always / No), save via `pref set . auto_plate`, proceed accordingly.

Close with `stats phase "$SESSION_DIR" taste <estimated tokens>` and the `common.md` self-verification table.

## Judgment notes

- Taste's value is independence: it re-derives verdicts from commands and artifacts, not from cook's claims.
- Don't spend context diagnosing WHY a test fails beyond capturing evidence — that's fix's job in a fresh window.
