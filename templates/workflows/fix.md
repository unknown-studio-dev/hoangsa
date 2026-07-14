# HOANGSA Fix — Contract

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules, contract format, CLI reference, self-verification template.

## Mission

Find the real root cause of a bug (which is often in a different layer than the symptom), fix it minimally through fresh-context workers, verify, and chain to taste. Never patch a symptom while leaving the root cause standing — and never let a fix silently regress the original task's contract.

## Inputs

- Bug description from the user (error output, repro steps, files if known).
- Task link (Linear/Jira/ClickUp/GitHub/Asana) → apply `task-link.md`: fetch to `EXTERNAL-TASK.md`, download attachments, set "In Progress" (non-blocking); labels hint the affected layer, comments may hold repro steps.
- Media → apply `common.md §Media detection`; findings become bug context.
- Past fixes: `memory_archive_search({query: "<error message or bug description>"})` — same root cause may have been seen before.

## Deliverables

1. Root-cause analysis the user has confirmed (origin layer vs symptom layer, trace, affected files).
2. A minimal fix plan (1–3 tasks) in `$SESSION_DIR/plan.json`, executed with atomic `fix(<scope>):` commits.
3. Chain to `/hoangsa:taste`; results reported, phase stat recorded.

## Hard gates

| # | Gate | Check |
|---|------|-------|
| 1 | Root cause confirmed | user approved the analysis (and the fix location, when it differs from the symptom layer) |
| 2 | Plan valid | `validate plan "$SESSION_DIR/plan.json"` passes; 1–3 tasks, each with runnable `acceptance` |
| 3 | Contract inherited | tasks fixing a taste-failed task carry that task's `test_cases`, `edge_cases`, `ui` flag verbatim |
| 4 | Per-task acceptance | `acceptance` passes before commit; max 3 worker retries |
| 5 | Minimal scope | `memory_detect_changes` on each commit shows only expected symbols |
| 6 | UI evidence | a `ui: true` fix has re-rendered screenshots in `$SESSION_DIR/evidence/<task.id>/` |

## Analysis — root cause before plan

Read only what's needed to trace the failure. Use `memory_symbol_context` / `memory_impact` when available.

**Cross-layer tracing** — trigger when: symptom involves data from another layer (FE bug ↔ API response, BE bug ↔ schema), stack trace crosses package/service boundaries, "it worked before" + recent changes elsewhere, or a multi-layer project where the reported layer shows no clear cause. Spawn a readonly research subagent:

```
You are a HOANGSA cross-layer bug tracer. Determine whether this bug originates in a different layer than where symptoms appear.
Bug report: <description> · Symptom layer: <layer> · Symptom files: <files>
1. Read the symptom code — what data/behavior does it expect?
2. Trace data flow backward (FE → endpoint → handler → data source; BE → queries/schema → upstream → middleware)
3. Look for contract mismatches (response shape, types) and recent changes (git log --since="2 weeks ago" on handlers, schemas, shared types, migrations)
Report: root cause layer, symptom layer, trace path, finding (1 paragraph), evidence (file:line — wrong vs expected), recommendation.
```

Consolidate into one report for the user: root cause, origin vs symptom layer, trace path, affected files tagged [ROOT CAUSE] / [SYMPTOM FIX] / [CONTRACT UPDATE]. If the root cause is in an unexpected layer, confirm before proceeding:

Use AskUserQuestion:
  question: "Bug xuất phát từ <root_layer>, không phải <symptom_layer>. Fix ở đâu?"
  header: "Root cause"
  options:
    - label: "Fix gốc (<root_layer>)", description: "Sửa đúng nguyên nhân gốc — recommended"
    - label: "Fix cả hai", description: "Sửa gốc + patch tạm ở <symptom_layer>"
    - label: "Patch tạm (<symptom_layer>)", description: "Chỉ sửa triệu chứng — root cause vẫn còn"
  multiSelect: false

## Plan — minimal, ordered, inherited

Session: `session latest`, or auto-create `session init fix "$SLUG"` (slug = 2-4 key words from the root cause, hyphenated, lowercase — the user never types it). Then apply `git-context.md` Parts A, B, D — fix branches fork from `main`/`master` in gitflow repos.

Write `$SESSION_DIR/plan.json`: `task_type: "fix"`, `status: "cooking"`, 1–3 tasks, each independently verifiable, <10k tokens, zero scope creep. Cross-layer order: root cause → contracts/types → symptom layer (only if a separate patch is needed).

**Inherit the spec contract (Gate 3).** If this fix targets a task that failed taste, copy that task's `test_cases`, `edge_cases`, and `ui` flag into the fix task verbatim, plus taste's visual failure detail for UI tasks — a fix worker that never sees the edge cases will re-break them.

Show the plan (root cause, tasks + acceptance commands); proceed only on user confirmation.

## Execute

```bash
MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model worker)
PROMPT=$("$HOANGSA_ROOT/bin/hoangsa-cli" envelope "$SESSION_DIR" "<task.id>" --kind fix --memory-status "<MEMORY_STATUS>")
```

`envelope --kind fix` emits the complete worker prompt (composed rules, task envelope with inherited contract, lessons, skills, fix instructions) with one placeholder: replace the `<BUG_CONTEXT …>` line with the root-cause summary and cross-layer notes from your analysis. Do NOT hand-assemble the prompt. Spawn one subagent per task (Task tool, `MODEL`).

After each task's acceptance passes: run the simplify pass exactly as in `cook.md §Execution model` step 3 (respect `simplify_pass`).

Escalation: same ladder as `cook.md §Escalation`.

## Verify, report, chain

- Gate 5: `memory_detect_changes` per commit — a hotfix touching unexpected symbols gets reverted or re-scoped, not waved through.
- Chain to `/hoangsa:taste` (it re-runs acceptance, the test-quality gate, and visual verification for `ui: true` tasks).
- Report: root cause, what changed per layer, acceptance results, taste outcome. Update state, then `stats phase "$SESSION_DIR" fix <estimated tokens>`.
- External task linked → sync-back happens at plate (`serve` push mode), not here.

## Judgment notes

- 1 task is a fine plan; 3 is the ceiling. If the fix wants a 4th task, it's a feature — send it to `/hoangsa:menu`.
- "Fix cả hai" plans still order root cause first; the temporary symptom patch is labeled as such in the commit message.
- If analysis can't find a root cause at all, say so and present hypotheses with evidence — don't invent certainty.
