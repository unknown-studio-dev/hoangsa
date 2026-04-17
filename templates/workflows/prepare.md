# HOANGSA Prepare Workflow

You are the decomposer. Mission: turn spec into an executable JSON plan with **automatic checker loop**.

**Principles:** Plan must be self-contained. Acceptance must be a runnable command. Checker runs before showing user.

> **MUST complete ALL steps in order. DO NOT skip any step. DO NOT stop before Step 8.**
>
> 0. Setup (lang) → 1. Load session + specs → 2. Detect stack → 3. Generate plan → 4. DAG validation → 5. Checker loop (MANDATORY) → 6. Context packs → 7. User approval → 8. Save plan

---

## Step 0: Language enforcement

```bash
# Resolve HOANGSA install path (local preferred over global)
if [ -x "./.claude/hoangsa/bin/hoangsa-cli" ]; then
  HOANGSA_ROOT="./.claude/hoangsa"
else
  HOANGSA_ROOT="$HOME/.claude/hoangsa"
fi

LANG_PREF=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . lang)
```

All user-facing text — status updates, questions, reports, error messages, progress displays — **MUST** use the language from `lang` preference (`vi` → Vietnamese, `en` → English, `null` → default English). This applies throughout the **ENTIRE** workflow. Do not switch languages mid-conversation. Template examples in this workflow are illustrative — adapt them to match the user's `lang` preference.

---

## Step 0b: Model selection + config metadata

```bash
PLANNER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model planner)
INTERACTION=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . interaction_level)
```

Use the `planner` model for plan generation and task decomposition. This respects both the project's `profile` setting and any per-role `model_overrides` in config.json.

Adapt verbosity based on `interaction_level`:
- `"detailed"` → show full DAG explanation, budget breakdown per task, traceability matrix
- `"concise"` → show wave summary and total budget only
- `null` → default to `"detailed"`

---

## Step 1: Load session

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
echo $SESSION
```

If `found: false` → ask user to run `/hoangsa:menu` first, stop.

Read from session dir:
- `DESIGN-SPEC.md`
- `TEST-SPEC.md`

Validate immediately:

```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" validate spec "$SESSION_DIR/DESIGN-SPEC.md"
"$HOANGSA_ROOT/bin/hoangsa-cli" validate tests "$SESSION_DIR/TEST-SPEC.md"
```

If errors → show errors, ask user to fix spec first.

---

## Step 1b: Load external task reference

If `$SESSION_DIR/EXTERNAL-TASK.md` exists, read it and include external task reference (provider, task_id, acceptance criteria) in plan metadata. This ensures traceability from external task to plan.

---

## Step 1c: Thoth index check

```bash
if [ -f ".thoth/graph.redb" ]; then
  echo "THOTH_AVAILABLE"
else
  echo "THOTH_NOT_INDEXED"
fi
```

Store result as `THOTH_STATUS`.

- If `THOTH_AVAILABLE` → continue. Use Thoth in Step 3 for dependency analysis.
- If `THOTH_NOT_INDEXED` → ask the user:

  Use AskUserQuestion:
    question: "Thoth index chưa có. Tạo index để plan có dependency graph chính xác hơn?"
    header: "Thoth"
    options:
      - label: "Index ngay", description: "Chạy thoth index (~30s) — hiểu callers, callees, impact để tạo depends_on và context_pointers chính xác"
      - label: "Bỏ qua", description: "Dùng Grep/Glob — vẫn tạo plan được nhưng depends_on có thể thiếu"
    multiSelect: false

  If user chọn "Index ngay":
    ```bash
    npx thoth --json index
    ```
    Set `THOTH_STATUS` = `THOTH_AVAILABLE`.

  If user chọn "Bỏ qua" → set `THOTH_STATUS` = `THOTH_NOT_INDEXED`, continue.

---

## Step 2: Read context

From DESIGN-SPEC frontmatter:
- `language` → determine stack for acceptance commands
- `component` → namespace/package name
- `task_type` → affects phase ordering

From filesystem:
- `workspace_dir` → absolute path of project root (where manifest file lives: `Cargo.toml`, `package.json`, `go.mod`, `pyproject.toml`, etc.)
- Read manifest for exact package/module name — **DON'T guess from directory names**

### Acceptance command templates by stack:

| Language | Template |
|----------|---------|
| `rust` | `cargo test --package <pkg> <test_name>` |
| `python` | `pytest <path>::<test_name> -v` |
| `typescript` / `javascript` | `npx jest <file> --testNamePattern="<name>"` |
| `go` | `go test ./<pkg>/... -run <TestName> -v` |
| `java` | `./gradlew test --tests "<class>.<method>"` |
| other | `<test runner> <args>` |

---

## Step 3: Decompose into tasks

### 3a. Map requirements → tasks

For each `[REQ-xx]` in DESIGN-SPEC → ≥1 task covers that requirement.
Record traceability: `"covers": ["REQ-xx"]` in each task.

### 3b. Map test cases → tasks

For each test case in TEST-SPEC → assign to a task that implements it.
Test tasks always `depends_on` the corresponding implementation tasks.

### 3c. Standard phase ordering

```
Phase 1: Type/Schema definitions   (parallel ok)
Phase 2: Interface/API definitions  (parallel ok)
Phase 3: Implementations            (parallel where possible)
Phase 4: Unit tests                 (parallel ok)
Phase 5: Integration tests          (sequential when interdependent)
```

Within a phase → maximize parallel, minimize sequential chains.

### 3d. Budget per task

| Complexity | Tokens | When |
|-----------|--------|------|
| `low` | 8,000–15,000 | 1 function, type changes, docs |
| `medium` | 15,000–30,000 | 1 module, refactor component |
| `high` | 30,000–45,000 | Complex algo, external integration |

**Hard limit: 45,000/task.** Exceeds → split the task.

### 3e. Context pointers & dependency analysis

**If Thoth available:** For each key symbol mentioned in the DESIGN-SPEC (types, functions, classes to create or modify):

1. Run `thoth_symbol_context({name: "symbolName"})` to get callers, callees, and process participation
2. Use callers/callees to:
   - Identify correct `depends_on` between tasks (if task A modifies a symbol called by task B's symbol → B depends on A)
   - Generate precise `context_pointers` — include caller definitions that workers need to see
3. Run `thoth_impact({target: "symbolName", direction: "upstream"})` for symbols being modified → if HIGH/CRITICAL risk, flag in the plan and consider splitting the task

**If Thoth unavailable:** Use Grep/Glob to find references and imports. Less precise but functional.

**Context pointer format:** `absolute/path/to/file:START_LINE-END_LINE`

Priority: function/class definitions the worker needs to implement.
Don't add unrelated files.

---

## Step 4: Create plan.json

```json
{
  "name": "<task_type>: <title>",
  "workspace_dir": "<absolute path — project root>",
  "spec_ref": "<component>-spec-v1.0",
  "language": "<from DESIGN-SPEC frontmatter>",
  "budget_tokens": "<sum of all tasks>",
  "tasks": [
    {
      "id": "<prefix>-<N>",
      "name": "<Verb + what — e.g. Implement UserService.create_user>",
      "complexity": "low|medium|high",
      "budget_tokens": "<per task>",
      "namespace": "<package/module name from manifest — null if N/A>",
      "files": ["<absolute path>"],
      "depends_on": ["<task ids>"],
      "context_pointers": ["<absolute/path/file:L1-L2>"],
      "covers": ["<REQ-xx>"],
      "acceptance": "<runnable command>"
    }
  ]
}
```

Save to `$SESSION_DIR/plan.json`.

---

## Step 5: Checker loop — MANDATORY before showing user

### Check 1 — Validate with hoangsa-cli

```bash
RESULT=$("$HOANGSA_ROOT/bin/hoangsa-cli" validate plan \
  "$SESSION_DIR/plan.json")
echo $RESULT
```

### Check 2 — DAG validation

```bash
DAG=$("$HOANGSA_ROOT/bin/hoangsa-cli" dag check \
  "$SESSION_DIR/plan.json")
echo $DAG
```

Common DAG errors: (1) Circular dependency — break the cycle by splitting the task with the circular ref into two tasks. (2) Orphaned task (no dependencies but not in wave 1) — move to wave 1 or add correct dependency. (3) Missing dependency target — fix the dep reference or remove it.

### Check 3 — Traceability (manual review)

```
✓ Every [REQ-xx] in DESIGN-SPEC is covered by ≥1 task
✓ Every test case in TEST-SPEC is assigned to ≥1 task
✓ No orphan tasks
```

### Check 4 — Quality (manual review)

```
✓ acceptance is a runnable command (not prose)
✓ No task exceeds 45,000 tokens
✓ context_pointers sufficient for worker to implement
✓ namespace read from manifest (not guessed)
```

### If errors:

1. **Auto-fix** what's possible (path format, budget sum)
2. **Recreate broken tasks** if needed (split large tasks, add missing traceability)
3. **Re-run checker** after fix
4. Only show user after **all checks pass**

---

## Step 5b: Generate context packs

After all checker loops pass, generate a context pack for each task using `context pack`:

```bash
for TASK_ID in $("$HOANGSA_ROOT/bin/hoangsa-cli" plan task-ids "$SESSION_DIR/plan.json"); do
  "$HOANGSA_ROOT/bin/hoangsa-cli" context pack \
    "$SESSION_DIR/plan.json" "$TASK_ID" \
    --output "$SESSION_DIR/task-${TASK_ID}.context.json"
done
```

Each `task-XXX.context.json` bundles the task's `context_pointers` file contents so cook-phase workers start with pre-loaded context and do not need to re-read the codebase from scratch.

If `context pack` fails for a task → log a warning but do not block plan approval. Missing context packs will be regenerated during cook phase.

---

## Step 6: Show plan to user

```bash
WAVES=$("$HOANGSA_ROOT/bin/hoangsa-cli" dag waves \
  "$SESSION_DIR/plan.json")
echo $WAVES
```

Display:

```
📋 Plan: <name>
   Stack:   <language>
   Budget:  <total> tokens
   Tasks:   <N> total
   Checker: ✅ all checks passed

──────────────────────────────────────────
Wave 1 (parallel):
  [T-01] Define UserSchema                      [low,  10k]
  [T-02] Define ErrorTypes                      [low,   8k]

Wave 2 (parallel):
  [T-03] Implement UserService.create_user      [med,  25k]  ← T-01
  [T-04] Implement validation middleware        [med,  20k]  ← T-02

Wave 3:
  [T-05] Write unit tests: user_service_*       [med,  20k]  ← T-03, T-04
  [T-06] Write integration tests: auth_flow     [high, 35k]  ← T-05

──────────────────────────────────────────
Traceability:
  REQ-01 → T-03  REQ-02 → T-04  REQ-03 → T-05, T-06

Budget: 118,000 tokens total
```

---

## Step 7: User adjustments

| Command | Action |
|---------|--------|
| "split T-03 into 2" | Split, recalculate, reassign deps |
| "merge T-01 and T-02" | Merge, update downstream depends_on |
| "add dep: T-04 after T-03" | Update depends_on, re-validate DAG |
| "reduce budget T-06" | Update, recalculate total |
| "change acceptance T-05" | Update command |

After each change → **re-run checker automatically**.

---

## Step 8: Save plan

When user approves:

```bash
# Final validation
"$HOANGSA_ROOT/bin/hoangsa-cli" validate plan "$SESSION_DIR/plan.json"

# Commit
"$HOANGSA_ROOT/bin/hoangsa-cli" commit \
  "prepare($SESSION_ID): create execution plan for <component>" \
  --files "$SESSION_DIR/plan.json"
```

Report:
```
✅ Plan saved!
   File:   .hoangsa/sessions/<id>/plan.json
   Tasks:  <N>
   Budget: <total> tokens

   Next: /hoangsa:cook
```

---

## Context engineering rules

Each task in the plan will execute in a **fresh context window** during cook phase. This prevents context rot — the quality degradation that occurs as Claude fills its context window. The plan must be self-contained enough that each task can execute independently with only its `context_pointers` and `acceptance` criteria.

---

## Self-verification checklist

Before reporting completion in Step 8, output this table. Every row MUST show DONE or SKIPPED:

```
| Step | Status |
|------|--------|
| 0. Setup (lang) | DONE / SKIPPED |
| 1. Load session + specs | DONE / SKIPPED |
| 2. Detect stack | DONE / SKIPPED |
| 3. Generate plan | DONE / SKIPPED |
| 4. DAG validation | DONE / SKIPPED |
| 5. Checker loop | DONE / SKIPPED |
| 6. Context packs | DONE / SKIPPED |
| 7. User approval | DONE / SKIPPED |
| 8. Save plan | DONE / SKIPPED |
```

If any step shows SKIPPED without explicit user approval, go back and complete it before stopping.

---

## Rules

| Rule | Detail |
|------|--------|
| **Checker first, show later** | Never show unvalidated plan |
| **Acceptance = runnable command** | Match the actual stack |
| **Namespace from manifest** | Read file — don't guess |
| **Paths = absolute** | workspace_dir, files, context_pointers |
| **Max 45k/task** | Exceeds → split |
| **Traceability mandatory** | Every REQ must have a task covering it |
| **Fresh context per task** | Plan must be self-contained |
