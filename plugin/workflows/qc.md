# HOANGSA QC — Contract

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules, contract format, CLI reference, self-verification template.

## Mission

Independent QC: take a spec, design test cases from it, execute them against the real system, and deliver a verdict per case where **every verdict — pass or fail — is backed by captured evidence**. A pass without evidence is a claim, not a result; a bug without a reproduction is a rumor. You test and prove — you never fix (bugs route to `/hoangsa:fix`).

## Inputs

**The spec, in priority order:** (1) a file/path/URL or pasted requirements the user provides · (2) the current session's `TEST-SPEC.md` + `DESIGN-SPEC.md` (`session latest`) · (3) an external task link → apply `task-link.md`. Nothing found → ask the user for the spec, stop until provided. Media in the input → `common.md §Media detection` (mockups/recordings become expected-behavior context).

**Session:** reuse the spec's session when it came from one; otherwise `session init test "qc-<slug>"` (slug auto-derived from the spec title). Config (`config get .`) supplies test frameworks and run commands; `resolve-model tester`.

## Deliverables

1. `$SESSION_DIR/QC-TESTCASES.md` — test cases derived from the spec, **user-approved before execution**.
2. `$SESSION_DIR/QC-REPORT.md` — verdict per case, each linking its evidence files.
3. `$SESSION_DIR/evidence/qc/<case-id>/` — captured artifacts for EVERY executed case.
4. One bug report per failure (format below), ready to hand to `/hoangsa:fix`.
5. `stats phase "$SESSION_DIR" qc <estimated tokens>`.

## Hard gates

| # | Gate | Check |
|---|------|-------|
| 1 | Coverage | every requirement/behavior in the spec has ≥1 case; every spec Edge Cases row has its own case; each requirement has ≥1 negative case where meaningful |
| 2 | Plan approved | user approved QC-TESTCASES.md (revise loop) before execution |
| 3 | Every case concluded | verdict ∈ pass / fail / blocked — `blocked` only with the user's explicit ack |
| 4 | **Evidence per verdict** | every pass AND every fail links ≥1 captured artifact on disk; a case with no artifact is `unverified` and must be re-run — it is NEVER reported as pass |
| 5 | Bugs reproducible | every fail has repro steps + expected-vs-actual + evidence; where possible, verified twice |

## Test case design

Derive from the spec — not from the implementation (read code only to locate entry points and run commands; independence is the point). For each requirement: happy path + boundary/edge rows from the spec + negative cases (invalid input, unauthorized, empty state). Type each case `auto` (runnable command) or `manual` (run-and-observe).

```markdown
### TC-01: <name>
- **Covers**: [REQ-xx]  ·  **Priority**: P1|P2|P3  ·  **Type**: auto|manual
- **Precondition**: <state/fixtures>
- **Steps**: <numbered, concrete — a stranger could execute them>
- **Expected**: <observable outcome — output, status code, rendered state>
- **Execute**: `<command>` (auto) / <entry point + walk-through> (manual)
```

Show the full QC-TESTCASES.md, then AskUserQuestion: "Test cases OK chưa?" — OK / Cần sửa (chi tiết vào Other) / Thêm case. Loop until OK (Gate 2).

## Evidence discipline — what counts, per surface

Evidence is **artifacts captured to disk during the run**, linked from the report. Narration is not evidence.

| Surface | Required artifacts in `evidence/qc/<case-id>/` |
|---------|------------------------------------------------|
| CLI / unit / integration | exact command + full stdout/stderr + exit code → `run.txt` (e.g. `<cmd> 2>&1 \| tee run.txt; echo "exit=$?" >> run.txt`) |
| API | request (method/url/headers-sans-secrets/body) + full response (status + body) → `request.json`, `response.json` |
| UI | screenshot per asserted state (fe-testing skill, flow 5 run-and-observe) → `<state>.png`; interactions get before/after pairs |
| Data / DB | query + result snapshot → `query.txt`, `result.txt` |

Capture at execution time, not reconstructed afterwards. Failing cases additionally capture whatever localizes the bug (logs, stack traces, the offending response).

## Execution

Run `auto` cases first (cheap, parallel where independent), then `manual`/UI cases via run-and-observe. For every case: execute → capture artifacts → record verdict + artifact paths. Flaky signal (pass/fail varies) → run 3×, report the distribution, verdict = fail with `flaky` tag. Environment prevents a case from running → `blocked` + what's missing, and ask the user before proceeding past it (Gate 3).

## Report & route

`QC-REPORT.md`:

```
# QC Report — <spec title>            <date> · spec: <path/ref>
Summary: N pass / N fail / N blocked  ·  Coverage: N/N requirements

| Case | Covers | Verdict | Evidence |
|------|--------|---------|----------|
| TC-01 | REQ-01 | ✅ pass | evidence/qc/TC-01/run.txt |
| TC-04 | REQ-02 | ❌ fail | evidence/qc/TC-04/{run.txt,error.png} |

## BUG-01 — <title>   (from TC-04, severity: critical|major|minor)
Repro: <numbered steps>          Expected: <per spec, cite REQ>
Actual: <what happened>          Evidence: <paths>
```

Bugs found → offer routing: `/hoangsa:fix` per bug (the bug report is the fix workflow's input — root-cause analysis starts from your repro), fix manually, or accept as known-issue (user's call, recorded in the report). All pass → report and stop; QC does not chain further on green.

## Judgment notes

- Independence is the value: if the spec and the implementation disagree, the spec wins — that's a bug (or a spec-update conversation for the user), never "adjust the expected value to match".
- Don't gold-plate coverage: P1 = spec requirements and their edge rows; invented exploratory cases are P3 and never block a verdict on the spec itself.
- Evidence files are small and targeted — capture the asserting artifact, not 10MB of logs.
