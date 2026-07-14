# HOANGSA Prepare — Contract

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules, contract format, CLI reference, self-verification template.

## Mission

Decompose the approved DESIGN-SPEC + TEST-SPEC into a machine-validated execution plan: parallel-maximized task DAG, every task self-contained (fresh-context workers execute it with only its envelope), every spec test and edge case embedded — never referenced.

## Inputs

Session (`session latest`) with `DESIGN-SPEC.md` + `TEST-SPEC.md`. Missing → send the user to `/hoangsa:menu`, stop. From DESIGN-SPEC frontmatter: `language` (drives acceptance commands), `component` (namespace), `task_type`. Read the package/module name from the actual manifest (`Cargo.toml`, `package.json`, …) — never guess from directory names. `EXTERNAL-TASK.md` exists → carry its reference into plan metadata.

## Deliverables

1. `$SESSION_DIR/plan.json` — schema below, all gates green.
2. Context pack per task (`context pack "$SESSION_DIR" "$TASK_ID"` for each id from `plan task-ids`) — pack failure is a warning, not a blocker (cook regenerates).
3. User-approved plan, committed: `commit "prepare(<scope>): create execution plan for <component>" --files "$SESSION_DIR/plan.json"`.

## Hard gates — run BEFORE showing the user; never show an unvalidated plan

| # | Gate | Command |
|---|------|---------|
| 1 | Schema + embeddings | `validate plan "$SESSION_DIR/plan.json" --tests "$SESSION_DIR/TEST-SPEC.md"` |
| 2 | DAG sound | `dag check "$SESSION_DIR/plan.json"` |
| 3 | Budget sane | per-task ≤ 45k tokens (`budget estimate` per task); over → split the task |

Gate 1's `--tests` machine-checks: every Edge Cases row embedded in ≥1 impl task AND ≥1 test task; every spec test in some task's `test_cases`; `## E2E Tests` → an e2e task exists; `surface: ui` → ≥1 task flagged `ui: true`. On errors: auto-fix what's mechanical (paths, budget sums), recreate broken tasks, re-run gates — loop until green.

Manual review on top (string matching can't catch mangled meaning): acceptance commands runnable for THIS stack; `context_pointers` sufficient for an isolated worker; every edge_case has concrete input + expected (no vague "handles errors"); no orphan tasks.

## plan.json schema

```json
{
  "name": "<task_type>: <title>",
  "workspace_dir": "<absolute project root>",
  "spec_ref": "<component>-spec-v1.0",
  "language": "<from DESIGN-SPEC>",
  "budget_tokens": "<sum of tasks>",
  "tasks": [
    {
      "id": "<prefix>-<N>",
      "name": "<Verb + what — e.g. Implement UserService.create_user>",
      "type": "<impl|test|e2e|research|analysis>",
      "ui": "<true for tasks implementing a Visual Verification screen — else omit>",
      "complexity": "low|medium|high",
      "budget_tokens": "<per task>",
      "namespace": "<package/module from manifest — null if N/A>",
      "files": ["<absolute path>"],
      "depends_on": ["<task ids>"],
      "context_pointers": ["<absolute/path/file:L1-L2>"],
      "covers": ["<REQ-xx>"],
      "test_cases": [
        { "name": "<from TEST-SPEC>", "covers": "REQ-xx", "expected": "<exact outcome>", "verify": "<runnable command>" }
      ],
      "edge_cases": [
        { "case": "<from TEST-SPEC>", "input": "<concrete input>", "expected": "<behavior>", "covers": "REQ-xx" }
      ],
      "acceptance": "<runnable command>"
    }
  ]
}
```

## Decomposition rules

- **Coverage:** every `[REQ-xx]` → ≥1 task (`covers`). Every TEST-SPEC test → assigned to a task; test tasks `depends_on` their implementation tasks.
- **Embed, don't reference — workers never read TEST-SPEC.** Copy each test case into `test_cases` and each `## Edge Cases` row into `edge_cases` of BOTH the implementing task and its test task, verbatim with concrete inputs. An envelope without its edge cases is how they get silently dropped.
- **E2E:** every `## E2E Tests` test becomes a Phase 6 task (`"type": "e2e"`) — never folded into other tasks, never dropped.
- **UI flag:** `surface: ui` → every task implementing a screen/component from `## Visual Verification` gets `"ui": true`. The flag (not filename heuristics) triggers run-and-observe in cook/taste.
- **Phases:** 1 types/schemas → 2 interfaces → 3 implementations → 4 unit tests → 5 integration tests → 6 e2e. Within a phase maximize parallelism; `depends_on` encodes the order.
- **Context pointers:** `absolute/path:L1-L2`, the definitions a worker must see. With hoangsa-memory: `memory_symbol_context` per key symbol → derive `depends_on` + precise pointers; `memory_impact` on modified symbols — HIGH/CRITICAL → flag in plan, consider splitting. Without: Grep/Glob for references.
- **Acceptance per stack:** Rust `cargo test --package <pkg> <test>` · Python `pytest <path>::<test> -v` · TS/JS `npx jest <file> --testNamePattern="<name>"` · Go `go test ./<pkg>/... -run <Test> -v` · Java `./gradlew test --tests "<class>.<method>"`.

## Approval loop

Show the validated plan: name, stack, budget, waves (`dag waves`), per-wave task list with complexity + budget, traceability map REQ → tasks. Adjustments the user can ask for — split/merge tasks, add deps, change budget/acceptance — apply, then **re-run all gates automatically** before re-showing. Approved → Deliverable 3, report:

```
✅ Plan saved!  Tasks: <N> · Budget: <total> tokens
   Next: /hoangsa:cook
```

Then `stats phase "$SESSION_DIR" prepare <estimated tokens>` and emit the `common.md` self-verification table.

## Judgment notes

- Fresh-context execution is the constraint that shapes everything: if a task can't be done with only its envelope, its `context_pointers` are wrong — fix the plan, not the worker.
- `test_cases`/`edge_cases` may be `[]` only when there is genuinely nothing to verify (pure type definitions); an impl task for a requirement with Edge Cases rows MUST carry them.
- Don't add unrelated files to `files`/`context_pointers` — every extra file is context-rot tax on the worker.
