# HOANGSA Cook Workflow

You are the orchestrator. Mission: execute the plan wave-by-wave, verify results, report.

**Principles:** Orchestrator does NOT write code. Only dispatch, monitor, report, escalate. Each task runs in a **fresh context** — this is the core of HOANGSA's context engineering.

---

## Step 0a: Language enforcement

```bash
# Resolve HOANGSA install path (local preferred over global)
if [ -x "./.claude/hoangsa/bin/hoangsa-cli" ]; then
  HOANGSA_ROOT="./.claude/hoangsa"
else
  HOANGSA_ROOT="$HOME/.claude/hoangsa"
fi

LANG_PREF=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . lang)
```

All user-facing text — status updates, questions, reports, error messages, escalation prompts, progress displays — **MUST** use the language from `lang` preference (`vi` → Vietnamese, `en` → English, `null` → default English). This applies throughout the **ENTIRE** workflow. Do not switch languages mid-conversation. Template examples in this workflow are illustrative — adapt them to match the user's `lang` preference.

---

## Step 0b: GitNexus index check (interactive)

Check if the GitNexus index is present and up-to-date:

```bash
if [ ! -d ".gitnexus" ]; then
  echo "GITNEXUS_MISSING"
elif [ -f ".gitnexus/.outdated" ] && [ "$(cat .gitnexus/.outdated 2>/dev/null | python3 -c 'import sys,json; d=json.load(sys.stdin); print(len(d.get("changed_files",[])))' 2>/dev/null)" != "0" ]; then
  echo "GITNEXUS_OUTDATED"
else
  echo "GITNEXUS_AVAILABLE"
fi
```

Store result as `GITNEXUS_STATUS`.

- If `GITNEXUS_AVAILABLE` → continue. Pass `GITNEXUS_STATUS` to all worker prompts so they can use GitNexus tools.
- If `GITNEXUS_MISSING` or `GITNEXUS_OUTDATED` → ask the user:

  Use AskUserQuestion:
    question: "GitNexus index bị outdated/missing. Sync lại để workers có code intelligence tốt hơn?"
    header: "GitNexus"
    options:
      - label: "Sync ngay", description: "Chạy gitnexus analyze (~30s) — workers sẽ có impact analysis, call graph, execution flows"
      - label: "Bỏ qua", description: "Workers sẽ dùng Grep/Glob thay thế — vẫn chạy được nhưng thiếu blast radius analysis"
    multiSelect: false

  If user chọn "Sync ngay":
    ```bash
    npx gitnexus analyze
    ```
    Set `GITNEXUS_STATUS` = `GITNEXUS_AVAILABLE` after sync completes.

  If user chọn "Bỏ qua" → set `GITNEXUS_STATUS` = `GITNEXUS_UNAVAILABLE`, continue.

---

## Step 1: Load and validate plan

### 1a. Find session + plan

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

If `found: false` → ask user to run `/hoangsa:prepare` first, stop.

### 1b. Validate plan.json

```bash
RESULT=$("$HOANGSA_ROOT/bin/hoangsa-cli" validate plan "$SESSION_DIR/plan.json")
echo $RESULT

DAG=$("$HOANGSA_ROOT/bin/hoangsa-cli" dag check "$SESSION_DIR/plan.json")
echo $DAG
```

If errors → show specific errors, suggest re-running `/hoangsa:prepare`.

### 1c. Load specs for verification

Read `$SESSION_DIR/DESIGN-SPEC.md` — used in Step 5 for semantic verification.
Note: `language` field in frontmatter → used to build correct verification commands.

### 1d. Compute waves

```bash
WAVES=$("$HOANGSA_ROOT/bin/hoangsa-cli" dag waves "$SESSION_DIR/plan.json")
echo $WAVES
```

---

## Step 2: Confirm with user

```
🚀 Ready to execute: <plan name>

   Stack:     <language from plan>
   Workspace: <workspace_dir>
   Budget:    <total> tokens

   Execution waves:
   Wave 1 (parallel — <N> tasks):
     [T-01] <name>  [<complexity>]
     [T-02] <name>  [<complexity>]

   Wave 2:
     [T-03] <name>  [<complexity>]  ← T-01
   ...

   Total: <N> tasks, <N> waves

Proceed? (yes/no)
```

Only continue when user confirms.

---

## Step 3: Execute wave-by-wave

### Model selection

```bash
WORKER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model worker)
REVIEWER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model reviewer)
```

Use `worker` model for task implementation (Step 3) and `reviewer` model for semantic review (Step 5 Tier 3). The orchestrator itself runs as `orchestrator` role — it only dispatches, monitors, and reports.

### For each wave:

1. **Load context pack for each task** before spawning workers
2. **Spawn one subagent per task** using the `Task` tool
3. Tasks in the same wave run **in parallel** (fresh context each)
4. Wait for all tasks in wave to complete before starting next wave

### Loading task context packs:

Before spawning each worker, load the task's context pack:

```bash
# Load the context pack for a specific task
CONTEXT=$("$HOANGSA_ROOT/bin/hoangsa-cli" context get "$SESSION_DIR" "<task.id>")
echo $CONTEXT
```

If the file `$SESSION_DIR/task-<task.id>.context.json` exists (created by `/hoangsa:prepare`), include its contents as additional context in the worker prompt. This ensures each worker has precisely the right context — no more, no less.

**Context pack fallback:** If context pack is missing or fails to load for a task, the worker should read context_pointers directly from plan.json as fallback.

### Worker prompt template:

For each task, spawn a subagent with this context:

```
You are a HOANGSA worker. Execute this task precisely.

Task: <task.name>
ID: <task.id>
Workspace: <workspace_dir>
GitNexus: <GITNEXUS_STATUS — GITNEXUS_AVAILABLE or GITNEXUS_UNAVAILABLE>

Files to modify:
<task.files — list>

Context to read first:
<task.context_pointers — list>

Requirements covered:
<task.covers — list>

Instructions:
1. Read all context_pointers files first
2. Before modifying any function/class/method, run gitnexus_impact({target: "symbolName", direction: "upstream"}) to check blast radius (if GitNexus is available)
3. If impact returns HIGH or CRITICAL risk — report it, do not proceed without orchestrator acknowledgment
4. Implement the task
5. Run the acceptance command to verify: <task.acceptance>
6. If acceptance fails, fix and retry (max 3 attempts)
7. Commit with message: "<task_type>(<session_id>): <task.name>"

Acceptance command: <task.acceptance>
```

**Token budget tracking:** Track token usage per task. If a task approaches 80% of its budget_tokens, warn. If it exceeds budget, the worker should wrap up current work and report partial completion rather than continuing.

### Worker rules:

Load worker rules before dispatching using a base + addons approach:

1. **Read base rules:**
   - If `.hoangsa/worker-rules.md` exists in workspace → use it as base (project override)
   - Otherwise → use `$HOANGSA_ROOT/workflows/worker-rules/base.md`

2. **Detect applicable addons:**
   - Read `tech_stack` from config.json preferences
   - Read `frameworks` from config.json `codebase.packages[].frameworks` (if available)
   - Read `test_frameworks` from config.json `codebase.testing.frameworks`
   - Match against addon file frontmatter `frameworks` field

3. **Load matching addons:**
   - For each matching addon: read `$HOANGSA_ROOT/workflows/worker-rules/addons/<name>.md`
   - Project-level addons override: `.hoangsa/worker-rules/addons/<name>.md`

4. **Compose final rules:**
   - Base rules + `"\n---\n"` + each addon content (frontmatter stripped)
   - Append to worker prompt

Include the composed rules in every worker prompt, appended after the task context above.

### Post-task: Simplify pass

After each worker completes a task successfully (acceptance passes), spawn a **simplify subagent** on the changed files before marking the task as done. This catches code quality issues, duplication, and inefficiencies while the context is still fresh.

For each completed task:

1. Collect the list of files the worker created or modified
2. Spawn a subagent with `/simplify` targeting those files:

```
Review the following files that were just created/modified for task <task.id>:
<list of changed files>

Use /simplify to check for:
- Code reuse opportunities (duplicated logic)
- Quality issues (unused imports, dead code, naming inconsistencies)
- Efficiency problems (unnecessary allocations, redundant operations)

Fix any issues found. Do NOT change behavior or add features — only improve code quality.
Commit fixes with message: "refactor(<session_id>): simplify <task.id>"
```

3. If the simplify pass finds and fixes issues → mark task as `✅ completed (simplified)`
4. If no issues found → mark task as `✅ completed`
5. Only then proceed to the next wave

**Important:** The simplify pass runs sequentially after each worker (not in parallel with other workers). This ensures the simplified code is what the next wave sees.

**Simplify failure recovery:** If simplify fails (crash, timeout, or reports blocker): log the error, skip simplify for this task, and continue to the next task. Do NOT block the wave.

### Track progress:

```
⏳ Executing...

  Wave 1:
    ✅ T-01 — Define UserSchema              [completed ✨]
    ✅ T-02 — Define ErrorTypes              [completed]

  Wave 2:
    🔄 T-03 — Implement create_user          [running...]
    ⏳ T-04 — Implement validation           [running...]

  Wave 3:
    ⬜ T-05 — Unit tests                     [pending]
    ⬜ T-06 — Integration tests              [pending]

  Progress: 2/6  |  Waves: 1/3 complete
```

States: `⬜ pending` · `⏳ running` · `✅ completed` · `✅ completed ✨` (simplified) · `❌ failed` · `🚫 blocked`

---

## Step 4: Escalation handling

### Escalation ladder (automatic):

```
1. Retry — same context, fresh attempt
2. Retry — enriched context (error details + traces)
3. Escalate model — switch to more capable model
4. Human escalation → orchestrator asks user
```

### When escalating to user:

```
🚨 Task blocked: <T-xx> — <name>

Acceptance command:
  $ <acceptance command>

Actual output:
  <stdout/stderr>

Retries exhausted:
  ✗ Attempt 1 — <error summary>
  ✗ Attempt 2 — enriched context, <error summary>
  ✗ Attempt 3 — model escalation, <error summary>

Affected files:
  <list>

What would you like to do?
  [1] Provide guidance → worker retries with your context
  [2] Skip this task → continue remaining tasks
  [3] Stop execution → review the plan
  [4] Fix manually → mark task done after you fix it
```

Orchestrator does NOT analyze code to suggest patches. Only presents evidence.

### Handle user choice:

**[1] Guidance:** Re-spawn worker with user's guidance added to prompt.

**[2] Skip:** Warn about downstream tasks that depend on this one. Confirm with user. Mark as skipped, continue.

**[3] Stop:** Halt execution, report current state.

**[4] Mark done:** User fixes manually, orchestrator marks task complete, continues.

---

## Step 4b: Chain behavior (after verification and final report)

> **Timing:** This step executes AFTER verification (Step 5) and final report (Step 6) are complete.

After all tasks finish execution and verification is done, read chain preferences from project config:

```bash
AUTO_TASTE=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . auto_taste)
```

- If `auto_taste` value is `true` → automatically chain to `/hoangsa:taste` after Step 6
- If `auto_taste` value is `false` → skip, continue to Step 5
- If `auto_taste` value is `null` (first time) → ask the user once, then **save their answer**:

### Task link detection (auto)

Apply the shared task-link detection from `task-link.md`:

1. If user input or session state contains an external task link → set status to "In Progress" at cook start
2. This is automatic and non-blocking — no user confirmation needed for "In Progress"

### External task sync-back (after completion)

If `state.external_task` exists after all waves complete, chain to `/serve` push mode so the user can sync results (status change, comment, full report) back to the task manager. This happens after taste and plate in the chain.

> **Note:** Cook does NOT chain directly to /serve. The sync-back chain is: cook → taste → plate → serve. Plate is the authoritative sync point.

  Use AskUserQuestion:
    question: "Muốn tự động chạy /hoangsa:taste sau khi cook xong?"
    header: "Auto taste"
    options:
      - label: "Luôn luôn", description: "Tự động test sau mỗi cook — khuyến khích"
      - label: "Không", description: "Tôi sẽ chạy taste thủ công khi cần"
    multiSelect: false

  Save immediately after user answers:

  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_taste true
  # or: pref set . auto_taste false
  ```

---

## Step 5: Verification (3-tier)

Run after all waves complete (or after stopping).

### Tier 1 — Static Analysis

| Stack | Command |
|-------|---------|
| Rust | `cargo check --workspace && cargo clippy --workspace -- -D warnings` |
| Python | `ruff check . && mypy .` (or project's tool) |
| TypeScript | `npx tsc --noEmit && npx eslint .` |
| Go | `go vet ./... && staticcheck ./...` |
| Generic | `<linter> <args>` per project config |

Report: error/warning count.

### Tier 2 — Behavioral (run ×3 for flaky detection)

Run test suite 3 times:

| Stack | Command |
|-------|---------|
| Rust | `cargo test --package <namespace>` |
| Python | `pytest tests/ -v` |
| TypeScript | `npx jest` |
| Go | `go test ./...` |
| Generic | `<test runner>` |

If results inconsistent → **flaky test detected**, list test names.

### Tier 3 — Semantic Review

Review against DESIGN-SPEC:
- All `[REQ-xx]` have been implemented
- No major deviation from Interfaces/APIs section
- Constraints are respected

```
Semantic check:
  ✅ REQ-01: UserSchema defined with correct fields
  ✅ REQ-02: validation middleware returns 422 on invalid input
  ⚠️  REQ-03: test coverage ~75%, target was 80%
```

---

## Step 6: Final report

### All pass:

```
🎉 Done!

  Execution:
    ✅ Tasks:    6/6 completed
    ✅ Static:   0 errors, 0 warnings
    ✅ Tests:    14/14 passed × 3 runs (no flaky)
    ✅ Semantic: 3/3 requirements verified

  Files changed:
    CREATED   src/models/user.py
    CREATED   src/services/user_service.py
    MODIFIED  src/api/routes.py
    CREATED   tests/test_user_service.py

  Budget used: 98k / 118k tokens (83%)

✅ Review and commit when ready.
```

### Partial / failures:

```
⚠️  Partially complete.

  Tasks:   5/6 (T-06 skipped by user)
  Static:  ✅ 0 errors
  Tests:   ⚠️  12/14 passed (2 failed)
  Semantic: ⚠️  REQ-03 not fully verified

  Failed tests:
    - test_create_user_duplicate: AssertionError
    - test_validation_empty_email: unexpected 200

  Next steps:
    1. Fix the failing tests
    2. Re-run /hoangsa:cook to retry remaining scope
    3. Or fix manually
```

---

## Context engineering

**Why fresh context per task matters:**

Claude's output quality degrades as the context window fills up ("context rot"). By giving each task its own fresh 200k context, every task gets Claude's best performance. The plan's `context_pointers` tell each worker exactly what to read — no more, no less.

This is HOANGSA's core value proposition. Never compromise on it.

---

## Rules

| Rule | Detail |
|------|--------|
| **DON'T write code yourself** | Orchestrator = coordinator only |
| **DON'T read source to suggest patches** | Present evidence, ask user |
| **Confirm before executing** | Always show plan, ask yes/no |
| **Stop when user asks** | Immediately |
| **Escalation is normal** | Follow the ladder, don't panic |
| **Verification by stack** | Match language from DESIGN-SPEC |
| **Plan is mandatory** | No plan = no cook |
| **Fresh context per task** | Core HOANGSA principle — never compromise |
| **Save preferences on first ask** | Ask once, save to config, never repeat |
