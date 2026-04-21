# HOANGSA Cook Workflow

You are the orchestrator. Mission: execute the plan wave-by-wave, verify results, report.

**Principles:** Orchestrator does NOT write code. Only dispatch, monitor, report, escalate. Each task runs in a **fresh context** — this is the core of HOANGSA's context engineering.

> **MUST complete ALL steps in order. DO NOT skip any step. DO NOT stop before Step 6.**
>
> 1. Load & validate plan → 2. Confirm with user → 3. Execute waves → 4. Escalation + quality gate → 5. Verification (3-tier) → 6. Final report + chain

---

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

### 1c. Git context check

Apply the shared git-context module from `git-context.md`:

1. Run Part A (detect branching context) — detect base branch, current branch, dirty state
2. Run Part B (git state check) — verify on correct branch for this session, switch if needed
3. Run Part D (stash recovery) — notify if stashed work exists for this task

The expected branch is derived from the session ID in `state.json`. If user is on wrong branch, prompt to switch before executing tasks.

### 1d. Load specs for verification

Read `$SESSION_DIR/DESIGN-SPEC.md` — used in Step 5 for semantic verification.
Note: `language` field in frontmatter → used to build correct verification commands.

### 1e. Compute waves

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

   ┌─────────────────────────────────────────────────┐
   │ Wave 1 (parallel — <N> tasks)                   │
   │  [T-01] <name>              [<complexity>]      │
   │  [T-02] <name>              [<complexity>]      │
   └────────┬──────────────┬─────────────────────────┘
            │              │
            ▼              ▼
   ┌─────────────────────────────────────────────────┐
   │ Wave 2                                          │
   │  [T-03] <name>              [<complexity>]      │
   └────────┬────────────────────────────────────────┘
            │
            ▼
          (...)

   Total: <N> tasks, <N> waves

Proceed? (yes/no)
```

Only continue when user confirms.

---

## Step 3: Execute wave-by-wave

### Session memory warm-up

```
memory_wakeup()
```

Load compact memory index at cook start to ensure recall is fast during execution.

### Model selection

```bash
WORKER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model worker)
REVIEWER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model reviewer)
```

Use `worker` model for task implementation (Step 3) and `reviewer` model for semantic review (Step 5 Tier 3). The orchestrator itself runs as `orchestrator` role — it only dispatches, monitors, and reports.

### Load config metadata

```bash
CONFIG=$("$HOANGSA_ROOT/bin/hoangsa-cli" config get .)
INTERACTION=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . interaction_level)
```

Extract from config:
- `codebase.testing` → test frameworks, config files, file patterns — used in Step 5 Tier 2 to select the correct test runner and config
- `codebase.linters` → linter config — used in Step 5 Tier 1 for static analysis commands
- `codebase.packages` → package info including build/test commands

**Apply `interaction_level` throughout:**
- `"detailed"` → show per-task progress with acceptance output, full verification results, budget breakdown
- `"concise"` → show wave-level progress, only report failures and final summary
- `null` → default to `"detailed"`

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

**Agent type selection:** Use typed agent definitions based on task type:
- Implementation tasks → `$HOANGSA_ROOT/agents/hoangsa-worker-impl.md` (sonnet, 25 turns, full tools + thoth)
- Research/analysis tasks → `$HOANGSA_ROOT/agents/hoangsa-worker-readonly.md` (sonnet, 15 turns, read-only)
- Simplify pass → `$HOANGSA_ROOT/agents/hoangsa-simplify.md` (haiku, 10 turns, edit-only)
- Quality review → `$HOANGSA_ROOT/agents/hoangsa-reviewer.md` (haiku, 15 turns, read-only + bash)

For each task, spawn a subagent with this context. **State filtering:** Only pass the task envelope below — do NOT include session history, other tasks' results, plan metadata, or orchestrator conversation. Each worker gets a clean, focused context.

**Excluded state** (never pass to workers):
- `messages` — orchestrator conversation history
- `plan.json` — full plan with all tasks (worker only needs its own task)
- Other tasks' results or context packs
- Session-level metadata (state.json, DESIGN-SPEC frontmatter)
- Skill content (workers get the registry, load on demand — see below)

**Prompt ordering (cache-optimized):** Static content FIRST, dynamic content LAST. This maximizes Anthropic prompt cache hit rates — the identical static prefix across all workers in a wave produces cache hits.

```
You are a HOANGSA worker. Execute this task precisely.

## Worker Rules

<COMPOSED_RULES — static, identical for all workers in this wave>

---

## Task Envelope

Task: <task.name>
ID: <task.id>
Workspace: <workspace_dir>
Thoth: <THOTH_STATUS — THOTH_AVAILABLE or THOTH_UNAVAILABLE>

Files to modify:
<task.files — list>

Context to read first:
<task.context_pointers — list>

Requirements covered:
<task.covers — list>

## Skill Registry (load on demand)

Available skills — read the full SKILL.md only if relevant to your task:
- git-flow: Git branching, task switching, PR creation → .claude/skills/hoangsa/git-flow/SKILL.md
- visual-debug: Screenshot/video analysis for visual bugs → .claude/skills/hoangsa/visual-debug/SKILL.md

To use a skill: read_file("<path>") to get full instructions, then follow them.
Do NOT read skills unless your task specifically requires them.

## Instructions

1. Read all context_pointers files first
2. Before modifying any function/class/method, run memory_impact({target: "symbolName", direction: "upstream"}) to check blast radius (if Thoth is available)
2b. Before starting, search for past work on this area:
    memory_archive_search({query: "<task.name> <primary module>"})
    Use findings to avoid repeating past mistakes or duplicating solutions.
3. If impact returns HIGH or CRITICAL risk — report it, do not proceed without orchestrator acknowledgment
4. Implement the task
5. Run the acceptance command to verify: <task.acceptance>
6. If acceptance fails, fix and retry (max 3 attempts)
7. Commit with message: "<task_type>(<scope>): <task.name>" — `<scope>` = primary module/package from task.files, NOT session_id/branch name

Acceptance command: <task.acceptance>

After task passes — save key findings for future reference:
  memory_turn_save({role: "assistant", text: "Task <task.id>: <one-line summary of what was done and key finding>"})

After task passes — verify change scope:
  memory_detect_changes({diff: "<git diff of this task's commit>"})
  If unexpected symbols are affected, report to orchestrator.
```

**THOTH_ACTOR:** Set `THOTH_ACTOR=hoangsa/cook-wave-<N>` environment variable when spawning workers. This selects the `hoangsa/cook-*` gate policy (longer recall window, lower relevance threshold) so workers have less friction during implementation.

**Token budget tracking:** Track token usage per task. If a task approaches 70% of its budget_tokens, warn. If it exceeds budget, the worker should wrap up current work and report partial completion rather than continuing.

**Per-turn tracking:** After each tool call round, estimate tokens consumed:
- Content sent: len(prompt_text) / 4
- Content received: len(response_text) / 4
- Accumulated total = sum of all turns

After task completes, record usage:
```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" stats record '<json>'
```

### Worker rules — Middleware Chain:

Load worker rules using a **middleware composition chain** (inspired by Deep Agents pattern). Each addon is a middleware unit with explicit priority and injection position.

#### Chain composition order:

```
┌─────────────────────────────────────────────────────┐
│  1. before_base addons  (priority-sorted, ascending) │
│  2. BASE rules          (worker-rules/base.md)       │
│  3. after_base addons   (priority-sorted, ascending) │  ← default position
│  4. PROJECT overrides   (.hoangsa/worker-rules.md)   │
│  5. tail addons         (priority-sorted, ascending) │  ← security/permission addons
└─────────────────────────────────────────────────────┘
```

#### Steps:

1. **Read base rules:**
   - Use `$HOANGSA_ROOT/workflows/worker-rules/base.md` as base

2. **Detect applicable addons:**
   - Read `tech_stack` from config.json preferences
   - Read `frameworks` from config.json `codebase.packages[].frameworks` (if available)
   - Read `test_frameworks` from config.json `codebase.testing.frameworks`
   - Match against addon file frontmatter `frameworks` field

3. **Load matching addons with middleware metadata:**
   - For each matching addon: read `$HOANGSA_ROOT/workflows/worker-rules/addons/<name>.md`
   - Project-level addons override: `.hoangsa/worker-rules/addons/<name>.md`
   - Parse frontmatter for `priority` (default: 50), `inject_position` (default: `after_base`), `pre_invoke_gate`, `allowed_tools`, and the gating fields below.

4. **Sort addons deterministically:**
   - Group by `inject_position`: `before_base`, `after_base`, `tail`
   - Within each group: sort by `priority` ascending, then by `name` alphabetically (for same priority)
   - This deterministic ordering maximizes Anthropic prompt cache hit rates

5. **Apply gates — drop in this order, each filter is independent:**

   5a. **Task-type gate** (reads `task.type` from current task in plan.json):
   - If addon has `exclude_task_types` and `task.type` is in the list → SKIP
   - If addon has `include_task_types` non-empty and `task.type` NOT in the list → SKIP
   - Example: `quality-checks.md` has `exclude_task_types: ["research", "analysis"]` — it won't ship to research/audit workers.

   5b. **Worker-role gate** (reads the role of the agent being spawned — `impl` / `readonly` / `simplify` / `reviewer`):
   - If addon has `exclude_worker_roles` and current role is in the list → SKIP
   - If addon has `include_worker_roles` non-empty and current role NOT in the list → SKIP
   - Rationale: framework edge-case checklists are wasted on simplify / reviewer workers.

   5c. **Pre-invoke gate** (shell command in `pre_invoke_gate`):
   - Run the command. If it exits non-zero, SKIP this addon with a warning.
   - Example: `thoth.md` uses `hoangsa-cli pref get . thoth_strict | grep -q true` so the whole addon vanishes when Thoth is not in strict mode.

6. **Enforce allowed_tools** (capability gating):
   - Collect `allowed_tools` arrays from all matching addons
   - If ANY addon specifies a non-empty `allowed_tools`, compute the **union** of all addon allowed_tools lists
   - If the union is non-empty, append this block to the worker prompt (after the rules):
     ```
     ## Tool Restrictions
     You are ONLY allowed to use these tools: <comma-separated union list>
     Do NOT use any tool not on this list. If you need a restricted tool, report it as a blocker.
     ```
   - If all addons have empty `allowed_tools` → no restriction (worker uses full toolset)

7. **Compose final rules:**
   - Concatenate in chain order: `before_base addons + "\n---\n" + base + "\n---\n" + after_base addons + "\n---\n" + project overrides + "\n---\n" + tail addons`
   - Strip frontmatter from each addon before concatenation
   - Append to worker prompt

Include the composed rules in every worker prompt at the `<COMPOSED_RULES>` position shown in the template above (BEFORE the task envelope — static-first for cache optimization).

### Post-task: Simplify pass

After each worker completes a task successfully (acceptance passes), spawn a **simplify subagent** on the changed files before marking the task as done. This catches code quality issues, duplication, and inefficiencies while the context is still fresh.

**Conditional:** Check simplify_pass preference before spawning simplify agents:

```bash
SIMPLIFY_PASS=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . simplify_pass | python3 -c "import sys,json; print(json.load(sys.stdin).get('value',True))")
```

If `SIMPLIFY_PASS` is `false`, skip the entire simplify section — mark tasks as `✅ completed` directly. Report: `⏭️ Simplify pass skipped (simplify_pass=false)`.

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
Commit fixes with message: "refactor(<scope>): simplify <task.id>" — `<scope>` = primary module/package from changed files, NOT session_id/branch name
```

3. If the simplify pass finds and fixes issues → mark task as `✅ completed (simplified)`
4. If no issues found → mark task as `✅ completed`
5. Only then proceed to the next wave

**Important:** The simplify pass runs sequentially after each worker (not in parallel with other workers). This ensures the simplified code is what the next wave sees.

**Simplify failure recovery:** If simplify fails (crash, timeout, or reports blocker): log the error, skip simplify for this task, and continue to the next task. Do NOT block the wave.

### Post-wave: Scope verification

After all tasks in a wave complete (and simplify passes finish):

1. **Verify wave scope:** Run `memory_detect_changes({diff: "<git diff of wave commits>"})` to confirm only expected symbols were affected across the wave.

### Track progress:

```
⏳ Executing...

  ┌─────────────────────────────────────────────────────────┐
  │ Wave 1                                                  │
  │  ✅ T-01 — Define UserSchema         [completed ✨]     │
  │  ✅ T-02 — Define ErrorTypes         [completed]        │
  └────────┬──────────────────────┬─────────────────────────┘
           │                      │
           ▼                      ▼
  ┌─────────────────────────────────────────────────────────┐
  │ Wave 2                                                  │
  │  🔄 T-03 — Implement create_user     [running...]       │
  │  ⏳ T-04 — Implement validation      [running...]       │
  └────────┬──────────────────────┬─────────────────────────┘
           │                      │
           ▼                      ▼
  ┌─────────────────────────────────────────────────────────┐
  │ Wave 3                                                  │
  │  ⬜ T-05 — Unit tests                [pending]          │
  │  ⬜ T-06 — Integration tests         [pending]          │
  └─────────────────────────────────────────────────────────┘

  Progress: 2/6  |  Waves: 1/3 complete
```

States: `⬜ pending` · `⏳ running` · `✅ completed` · `✅ completed ✨` (simplified) · `❌ failed` · `🚫 blocked`

---

## Step 4: Escalation handling

### Escalation ladder (automatic):

```
  ┌──────────────────────────────────────┐
  │ 1. Retry — same context              │
  └──────────────┬───────────────────────┘
                 │ fail
                 ▼
  ┌──────────────────────────────────────┐
  │ 2. Retry — enriched context          │
  │    (error details + traces)          │
  └──────────────┬───────────────────────┘
                 │ fail
                 ▼
  ┌──────────────────────────────────────┐
  │ 3. Escalate model                    │
  │    (switch to more capable model)    │
  └──────────────┬───────────────────────┘
                 │ fail
                 ▼
  ┌──────────────────────────────────────┐
  │ 4. Human escalation                  │
  │    (orchestrator asks user)          │
  └──────────────────────────────────────┘
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
- If `auto_taste` value is `false` → skip
- If `auto_taste` value is `null` (first time) → ask the user once, then **save their answer**:

  Use AskUserQuestion (adapt text to `$LANG_PREF`):
    question: "Run /hoangsa:taste automatically after cook completes?"
    header: "Auto taste"
    options:
      - label: "Always", description: "Auto-test after every cook — recommended"
      - label: "No", description: "I'll run taste manually when needed"
    multiSelect: false

  Save immediately after user answers:

  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_taste true
  # or: pref set . auto_taste false
  ```

### Task link detection (auto)

Apply the shared task-link detection from `task-link.md`:

1. If user input or session state contains an external task link → set status to "In Progress" at cook start
2. This is automatic and non-blocking — no user confirmation needed for "In Progress"

### External task sync-back (after completion)

If `state.external_task` exists after all waves complete, chain to `/serve` push mode so the user can sync results (status change, comment, full report) back to the task manager. This happens after taste and plate in the chain.

> **Note:** Cook does NOT chain directly to /serve. The sync-back chain is: cook → taste → plate → serve. Plate is the authoritative sync point.

---

## Step 4d: Code Quality Gate

Run after all waves complete, before verification. This is the final quality sweep on ALL changed files collectively — catches cross-task issues that per-task simplify misses.

**Conditional:** Check quality_gate preference:

```bash
QUALITY_GATE=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . quality_gate | python3 -c "import sys,json; print(json.load(sys.stdin).get('value',True))")
```

If `QUALITY_GATE` is `false`, skip all analyzer agents. Report: `⏭️ Quality gate skipped (quality_gate=false)`. Proceed directly to Step 5.

### Collect changed files

```bash
# Get all files changed in this session
CHANGED_FILES=$(git diff --name-only HEAD~$(git log --oneline --since="$(cat $SESSION_DIR/state.json | python3 -c "import sys,json; print(json.load(sys.stdin).get('started_at','1 hour ago'))")" | wc -l) HEAD 2>/dev/null || git diff --name-only HEAD~10 HEAD)
```

### Run quality analyzers (conditional)

Spawn subagents in parallel where applicable. Each analyzer is advisory — it reports issues but does NOT auto-fix. The orchestrator collects all findings and presents a single quality report.

**Always run:**

1. **code-reviewer** (final sweep) — general code quality review on all changed files
   ```
   Review these files for bugs, logic errors, security vulnerabilities, and code quality:
   <CHANGED_FILES>
   Focus on cross-file consistency and integration issues.
   Report only HIGH confidence issues.
   ```

2. **pr-test-analyzer** (test coverage check) — verify tests cover changed behavior
   ```
   Analyze test coverage for these changed files:
   <CHANGED_FILES>
   Focus on behavioral coverage, not line coverage.
   Identify critical gaps and missing edge cases.
   ```

**Conditional — run if error handling was changed:**

3. **silent-failure-hunter** — detect swallowed errors and inadequate error handling
   ```
   Check for silent failures in changed files:
   <CHANGED_FILES filtered to files with try/catch/except/error changes>
   Zero tolerance for swallowed errors.
   ```
   Skip if no error handling patterns detected in changed files.

**Conditional — run if comments were added/modified:**

4. **comment-analyzer** — verify comment accuracy and detect comment rot
   ```
   Analyze comments in changed files:
   <CHANGED_FILES filtered to files with comment changes>
   Check for misleading comments and documentation gaps.
   ```
   Skip if no comment changes detected.

**Conditional — run if types were added/modified:**

5. **type-design-analyzer** — evaluate type encapsulation and invariant quality
   ```
   Analyze type designs in changed files:
   <CHANGED_FILES filtered to files with type/interface/struct/class definitions>
   Rate encapsulation, invariant expression, and usefulness.
   ```
   Skip if no type definition changes detected.

### Quality Gate decision

Collect all analyzer results and present:

```
🔍 Code Quality Gate

  Analyzers run: <N>/5

  code-reviewer:         ✅ 0 issues  |  ⚠️ 2 warnings  |  ❌ 1 critical
  pr-test-analyzer:      ✅ good coverage  |  ⚠️ 2 gaps identified
  silent-failure-hunter: ✅ no silent failures  |  ⏭️ skipped (no error handling changes)
  comment-analyzer:      ⏭️ skipped (no comment changes)
  type-design-analyzer:  ⚠️ UserAccount type — encapsulation 6/10

  Critical issues (must fix):
    1. [code-reviewer] SQL injection risk in user_service.py:45

  Warnings (recommended):
    1. [pr-test-analyzer] Missing edge case: empty input for create_user
    2. [code-reviewer] Unused import in routes.py
    3. [type-design-analyzer] UserAccount exposes mutable internal state
```

**Gate rules:**
- **Critical issues (❌)** → MUST fix before proceeding. Spawn fix workers automatically, then re-run affected analyzers.
- **Warnings (⚠️)** → present to user, recommend fixing but don't block.
- **All clear (✅)** → proceed to Step 5 verification.

**Quality Gate failure recovery:** If an analyzer crashes or times out, skip it and note in report. Do NOT block the pipeline for analyzer failures.

---

## Step 5: Verification (3-tier)

Run after all waves complete and quality gate passes (or after stopping).

### Tier 1 — Static Analysis

| Stack | Command |
|-------|---------|
| Rust | `cargo check --workspace && cargo clippy --workspace -- -D warnings` |
| Python | `ruff check . && mypy .` (or project's tool) |
| TypeScript | `npx tsc --noEmit && npx eslint .` |
| Go | `go vet ./... && staticcheck ./...` |
| Generic | `<linter> <args>` per project config |

Report: error/warning count.

### Tier 2 — Behavioral (run ×N for flaky detection (N = test_runs preference, default 3))

**Conditional:** Read test_runs count:

```bash
TEST_RUNS=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . test_runs | python3 -c "import sys,json; print(json.load(sys.stdin).get('value',3))")
```

Run test suite `$TEST_RUNS` times (instead of hardcoded 3). If `TEST_RUNS=1`, skip flaky detection entirely — report single run results.

Run test suite $TEST_RUNS times:

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

  Budget used: 45k / 65k tokens (69%)
    Work:         28k (62%)
    System prompt: 3k → 300 (cached, 90% saved)
    Context:       5k (11%)
    Tool calls:   8k (18%) — 10 calls × 800
    Margin:        1k (2%)

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

## Self-verification checklist

Before reporting completion in Step 6, output this table. Every row MUST show DONE or SKIPPED:

```
| Step | Status |
|------|--------|
| 0. Setup (lang + Thoth) | DONE / SKIPPED |
| 1. Load & validate plan | DONE / SKIPPED |
| 2. Confirm with user | DONE / SKIPPED |
| 3. Execute all waves | DONE / SKIPPED |
| 4. Escalation handling | DONE / SKIPPED |
| 4d. Code Quality Gate | DONE / SKIPPED |
| 5. Verification (3-tier) | DONE / SKIPPED |
| 6. Final report | DONE / SKIPPED |
```

If any step shows SKIPPED without explicit user approval, go back and complete it before stopping.

---

## State persistence

Update `state.json` at key checkpoints so progress survives context compaction or crash:

```bash
# At Step 2 (after user confirms):
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_DIR" '{"status":"cooking"}'

# At Step 6 (final report — all pass):
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_DIR" '{"status":"done"}'

# At Step 6 (partial/failures):
"$HOANGSA_ROOT/bin/hoangsa-cli" state update "$SESSION_DIR" '{"status":"failed"}'
```

---

## Context engineering

**Why fresh context per task matters:**

Claude's output quality degrades as the context window fills up ("context rot"). By giving each task its own fresh 200k context, every task gets Claude's best performance. The plan's `context_pointers` tell each worker exactly what to read — no more, no less.

This is HOANGSA's core value proposition. Never compromise on it.

**Context optimization techniques (adapted from Deep Agents):**

1. **State filtering** — Workers receive only their task envelope. Excluded: orchestrator messages, other tasks' results, plan metadata, session state. This keeps worker context focused and clean.

2. **Progressive skill disclosure** — Workers receive a skill registry (name + 1-line description + path), not full skill content. Skills are loaded on-demand only when the task requires them. This saves ~500-1000 tokens per unused skill.

3. **Middleware chain composition** — Worker rules are composed in a deterministic order (before_base → base → after_base → project → tail), sorted by priority then name within each group. Deterministic ordering maximizes Anthropic prompt cache hit rates.

4. **Deterministic tool sorting** — MCP tools and addon rules are always sorted alphabetically by name. This ensures the same configuration always produces the same prompt prefix, enabling efficient prompt caching (~90% cost savings on cached portions).

5. **Context eviction** — Workers are instructed to evict large tool results (>100 lines) from working memory: extract key findings, note file path + line range for re-reading, discard the rest. This prevents context rot from accumulating massive Grep/Read outputs. (Adapted from DeepAgents `FilesystemMiddleware.wrap_tool_call` pattern — prompt-level enforcement rather than code-level.)

6. **Capability gating** — Addons can restrict worker tool access via `allowed_tools` frontmatter field. When enforced, workers receive an explicit tool restriction block in their prompt. This prevents research-only workers from accidentally writing files, or security-sensitive addons from allowing shell execution. (Adapted from DeepAgents `SandboxBackendProtocol` capability gating pattern.)

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
