# HOANGSA Taste Workflow

You are the test runner. Mission: run acceptance tests for all tasks in the plan, report results.

> **MUST complete ALL steps in order. DO NOT skip any step. DO NOT stop before Step 6.**
>
> 0. Setup (lang) → 1. Load session + plan → 2. Run acceptance tests → 3. Report failures → 4. Update statuses → 5. Report results → 6. Chain to plate

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

## Step 1: Load session and plan

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

If `found: false` → ask user to run `/hoangsa:prepare` first, stop.

Read `$SESSION_DIR/plan.json` — extract tasks and their `acceptance` commands.

---

## Step 1b: Model selection

```bash
MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model tester)
```

Use the resolved model for running test tasks. The `tester` role is lightweight (default: haiku in balanced profile) since it primarily runs commands and reports results.

---

## Step 2: Run acceptance tests per task

For each task in plan.json:

1. Run the task's `acceptance` command
2. Record pass or fail

```
Running acceptance tests...

  [T-01] <name>
    $ <acceptance command>
    ✅ passed

  [T-02] <name>
    $ <acceptance command>
    ❌ failed — <error summary>
```

---

## Step 2b: Test Quality Gate

For each task that passed acceptance, evaluate the quality of its tests. This is a prompt-based review — read the test file(s) and production file(s) side-by-side and judge quality. Do not run any code analysis tools.

**Read the files:**
- Identify the test file(s) created or modified by the task (look for `*.test.*`, `*.spec.*`, `__tests__/`, etc.)
- Read the corresponding production file(s) the tests are supposed to cover

**Check for fake test patterns:**
- **Copy-paste logic**: Does the test reproduce >3 lines of production logic verbatim instead of asserting outcomes?
- **Inline stubs**: Does the test create manual stub objects when the test framework provides proper mocking utilities?
- **Hardcoded assertions**: Do assertions only verify hardcoded literals without actually exercising the production code path?
- **Mocking the subject**: Does the test mock the very function it is supposed to test (testing the mock, not the code)?

**Check for edge case coverage:**
- **Async code**: Is there cleanup on unmount? Is error handling tested? Is timeout handling covered?
- **State management**: Are race conditions considered? Are stale closures possible? Are memory leaks guarded?
- **Promises**: Is rejection handling tested? Is settlement guarantee verified?
- **Event listeners**: Is listener removal on cleanup tested? Is multiple-attach prevention checked?

**Report per task:**

```
Test Quality Gate results:

  [T-01] <name>
    Test file:       <path>
    Production file: <path>
    Result: ✅ PASS — test quality OK

  [T-02] <name>
    Test file:       <path>
    Production file: <path>
    Result: ⚠️  WARN — <issue description> (non-blocking, reported to user)

  [T-03] <name>
    Test file:       <path>
    Production file: <path>
    Result: ❌ FAIL — fake test detected: <reason>
             copy-paste or coverage issue blocks commit — fix required
```

**Outcome rules:**
- `PASS` — no action needed, proceed
- `WARN` — report the issue to the user, do not block commit; user decides whether to address it
- `FAIL` — fake test detected; this task is re-marked as `failed` in plan.json and must be fixed before commit

If any task produces a `FAIL` result, update its status to `"failed"` in plan.json and include it in the Step 3 failure report.

---

## Step 3: Report failures

For each failed task, present a clear failure report:

```
❌ Task failed: <T-xx> — <name>

Acceptance command:
  $ <acceptance command>

Actual output:
  <stdout/stderr>
```

After all tasks are tested, if there are failures, present options:

```
<N> task(s) failed. What would you like to do?
  [1] /hoangsa:fix  — hotfix the failing task(s)
  [2] Fix manually  — mark as passed after you fix it
  [3] Skip          — mark as failed, move on
```

The taste workflow does NOT attempt to fix failures itself — that is the fix workflow's job. Taste is a reporter: test, record, report. This keeps the workflow focused and avoids wasting context on fix attempts that belong in a fresh-context fix session.

---

## Step 3b: Track lesson outcomes (if Thoth available)

After all tests are run, update lesson outcome scores based on results. This feeds Thoth's auto-quarantine system — lessons that consistently lead to failures get demoted automatically.

```bash
if [ -f ".thoth/graph.redb" ]; then
  echo "THOTH_AVAILABLE"
fi
```

If Thoth is available:

1. Read `.thoth/LESSONS.md` and identify lessons whose triggers mention symbols, modules, or patterns touched by the tested tasks
2. For each relevant lesson:
   - If the related task **passed** → `thoth_lesson_outcome({signal: "success", triggers: ["<lesson trigger text>"], note: "task <T-xx> passed acceptance"})`
   - If the related task **failed** and the lesson was supposed to prevent that kind of failure → `thoth_lesson_outcome({signal: "failure", triggers: ["<lesson trigger text>"], note: "task <T-xx> failed: <error summary>"})`
3. Only score lessons with clear relevance — do not score every lesson for every task

This is lightweight — skip if no lessons match the tested modules. The goal is gradual signal accumulation, not exhaustive scoring.

### Skill proposal check

After scoring all lesson outcomes, check if any lesson cluster has reached the skill proposal threshold:

1. Count lessons by topic/domain (e.g., lessons about editing source files, lessons about migration, etc.)
2. If a cluster has ≥5 lessons with cumulative success signals → call `thoth_skill_propose` to draft a consolidated skill
3. Include `source_triggers` (the trigger text of each consolidated lesson) so Thoth can track provenance
4. Report the draft to the user: "New skill draft: `.thoth/skills/<slug>.draft/` — run `thoth skills install` to accept"

Skip this check if no lessons were scored in this pass.

---

## Step 4: Update task statuses

For each task, update status in plan.json:

- Passed → `"status": "passed"`
- Failed (after all attempts) → `"status": "failed"`

Update plan.json directly by reading it, modifying the task status, and writing it back:

```bash
# Read plan.json, update task status, write back
python3 -c "
import json, sys
with open('$SESSION_DIR/plan.json', 'r') as f:
    plan = json.load(f)
for task in plan.get('tasks', []):
    if task['id'] == '<task_id>':
        task['status'] = '<status>'
with open('$SESSION_DIR/plan.json', 'w') as f:
    json.dump(plan, f, indent=2)
print(f'Updated <task_id> → <status>')
"
```

Run this for each task after testing.

---

## Step 5: Report results

```
🍽️  Taste results: <plan name>

  [T-01] Define UserSchema         ✅ passed
  [T-02] Define ErrorTypes         ✅ passed
  [T-03] Implement create_user     ❌ failed (3 attempts)

  Summary: 2/3 passed

Next steps:
  - /hoangsa:plate  — commit passing work
  - /hoangsa:fix    — hotfix the failing task
  - /hoangsa:check  — view full session status
```

---

## Step 6: Chain to plate

Read chain preference from project config:

```bash
AUTO_PLATE=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . auto_plate)
```

- If `auto_plate` value is `true` → automatically invoke `/hoangsa:plate`
- If `auto_plate` value is `false` → skip, just show next steps
- If `auto_plate` value is `null` (first time) → ask the user once, then **save their answer**:

  Use AskUserQuestion (adapt labels to user's `lang` preference):
    question: "Auto-commit after taste passes?"
    header: "Auto plate"
    options:
      - label: "Always", description: "Automatically commit when all tests pass"
      - label: "No", description: "I will commit manually with /hoangsa:plate"
    multiSelect: false

  Save immediately:

  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_plate true
  # or: pref set . auto_plate false
  ```

  Then proceed based on their choice.

---

## Rules

| Rule | Detail |
|------|--------|
| **Test only, don't fix** | Report failures clearly, delegate fixing to /hoangsa:fix |
| **Don't skip without asking** | Always confirm with user before marking failed |
| **Update plan.json statuses** | After every test run |
| **Report clearly** | Pass/fail per task with full command output shown |
| **Save preferences on first ask** | Ask once, save to config, never repeat |
