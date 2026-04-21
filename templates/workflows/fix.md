# Fix Workflow

Analyze a bug, create a minimal fix plan, implement the fixes, and chain to taste.

> **MUST complete ALL steps in order. DO NOT skip any step. DO NOT stop before Step 6.**
>
> 1. Analyze bug → 2. Cross-layer trace → 3. Confirm fix plan → 4. Implement fixes → 5. Chain to taste → 6. Report + sync

---

---

## Step 0c: Task link detection

Apply the shared task-link detection from `task-link.md`:

1. Scan user input for task manager URLs (Linear, Jira, ClickUp, GitHub, Asana)
2. If found → fetch task details via MCP → save to `EXTERNAL-TASK.md`
3. Fetch and process attachments (see `task-link.md` Step 3b) — download to `$SESSION_DIR/attachments/`, classify by type. **Do NOT process videos here** — video analysis is deferred to Step 1b (media detection) which handles both user-provided and task-link media in one pass.
4. Set task status to "In Progress" (non-blocking, best-effort)
5. Extract from fetched task:
   - **Bug description** → supplement user's bug report in Step 1
   - **Labels/tags** → identify affected layer (frontend, backend, api, database, etc.)
   - **Comments** → may contain reproduction steps or prior investigation
   - **Related tasks/PRs** → clues about recent changes that may have caused the bug

If no task URL → skip, proceed normally.

---

## Step 1: Gather bug context

Ask the user to describe the bug (if not already provided):

```
What's the bug?
- Error message or unexpected behaviour
- Steps to reproduce (if known)
- Relevant file(s) or component(s) (if known)
```

Read any error output, stack traces, or failing test output the user provides.

If an external task was linked (Step 0c), merge its description and comments into the bug context — the user may not need to re-describe it.

---

## Step 1b: Media detection (auto)

Scan **two sources** for media files:

1. **User's input** — file paths or pasted screenshots/videos in the message
2. **Task-link attachments** — files downloaded to `$SESSION_DIR/attachments/` by Step 0c

**Detection patterns:**
- File paths ending in: `.png`, `.jpg`, `.jpeg`, `.webp`, `.gif` (images)
- File paths ending in: `.mp4`, `.mov`, `.webm`, `.avi`, `.mkv` (videos)
- Screenshots pasted or attached by the user

**Check task-link attachments:**
```bash
# If task-link downloaded attachments, scan them too
if [ -d "$SESSION_DIR/attachments" ]; then
  ls "$SESSION_DIR/attachments/"
fi
```

**If images detected (from either source):**
- Claude reads images natively — no processing needed
- Note the image paths for reference in bug analysis

**If videos detected (from either source):**
1. Invoke the `visual-debug` skill for video processing:
   - Check ffmpeg availability: `hoangsa-cli media check-ffmpeg`
   - **Always quote the path** and validate it contains no shell metacharacters: `hoangsa-cli media analyze "$VIDEO_PATH" --output-dir "/tmp/hoangsa-debug-$(date +%s)"`
   - Read the output `montage.png` (annotated frame grid with timestamps)
   - Read the output `diff-montage.png` (red overlay showing changes between frames)
2. Include visual analysis findings in the bug context for Step 2

**If no media detected (from either source):** Skip this step, proceed to Step 2.

---

## Step 2: Analyze the bug

### 2a-pre. Search past conversations

Search for past fixes of similar bugs:
```
memory_archive_search({query: "<error message or bug description>"})
```

If results found → check if same root cause was seen before, learn from prior fix approach.

### 2a. Initial analysis

Read the relevant source files to trace the root cause. Keep analysis focused — read only what is needed to understand the failure.

### 2b. Cross-layer tracing (auto)

Bugs often cross layer boundaries. A frontend bug may originate from a backend API returning wrong data, or a backend bug may stem from a database schema issue. This step spawns a research agent to trace the root cause across layers.

**When to trigger cross-layer tracing:**

| Signal | Action |
|--------|--------|
| Bug is in frontend code but error involves API response data | Trace to backend |
| Bug is in backend but relates to database query/schema | Trace to database layer |
| Bug is in API but involves auth/middleware | Trace to auth layer |
| Stack trace crosses package/service boundaries | Trace all involved layers |
| External task labels indicate a specific layer but symptoms appear elsewhere | Trace both layers |
| User reports "it worked before" + recent changes in another layer | Trace recent changes |

**Always trigger if:** The project has multiple layers (FE+BE, or monorepo with multiple packages) and the initial analysis in 2a does not find a clear root cause within the reported layer.

**How to trace:**

Spawn a research subagent (using Task tool) with this prompt:

```
You are a HOANGSA cross-layer bug tracer. Your job is to determine whether this bug originates from a different layer than where the symptoms appear.

Bug report:
<bug description from Step 1>

Symptom layer: <frontend / backend / database / etc.>
Symptom files: <files identified in Step 2a>

Instructions:
1. Read the symptom code to understand what data/behavior it expects
2. Trace the data flow backward:
   - If FE bug → check the API endpoint it calls → check the backend handler → check the data source
   - If BE bug → check the database queries/schema → check upstream services → check middleware
   - If API bug → check request validation → check auth middleware → check client-side request
3. Look for mismatches:
   - API contract vs actual response shape
   - Expected types vs actual types
   - Recent changes in upstream layers (git log --since="2 weeks ago" on related files)
4. Check for recent changes that may have broken the contract:
   - git log on API route handlers, schema files, shared types
   - Any migration files added recently
5. Report your findings:

## Cross-Layer Analysis

**Root cause layer:** <where the actual bug is>
**Symptom layer:** <where the bug appears>
**Trace path:** <layer1> → <layer2> → <layer3>

**Finding:**
<one paragraph explaining what crosses the boundary and why>

**Evidence:**
- <file:line> — <what's wrong>
- <file:line> — <what it should be>

**Recommendation:**
- Fix in <layer> by <action>
- Also update <other layer> if needed
```

### 2c. Consolidate findings

Merge initial analysis (2a) with cross-layer findings (2b):

```
🔍 Bug Analysis

Root cause: <one-line description>
Origin layer: <frontend / backend / API / database / shared>
Symptom layer: <where the user sees the bug>

Trace:
  ┌──────────────────┐     ┌──────────────────┐     ┌──────────────────┐
  │  <symptom layer>  │────►│  <intermediate>   │────►│ <root cause>     │
  └──────────────────┘     └──────────────────┘     └──────────────────┘

  Example:
  ┌──────────────────┐     ┌──────────────────┐     ┌──────────────────┐
  │ React component  │────►│   API call        │────►│ Express handler  │
  └──────────────────┘     └──────────────────┘     │ returns wrong    │
                                                     │ shape            │
                                                     └──────────────────┘

Affected files:
  - <file path> — <what needs to change> [ROOT CAUSE]
  - <file path> — <what needs to change> [SYMPTOM FIX]
  - <file path> — <what needs to change> [CONTRACT UPDATE]

Cross-layer notes:
  <if applicable — e.g., "API response shape changed in commit abc123 but frontend types were not updated">
```

If cross-layer analysis found the real root cause is in a different layer:
- Clearly communicate this to the user — the fix may need to happen somewhere unexpected
- If the root cause is in a layer the user didn't expect, confirm before proceeding:

Use AskUserQuestion:
  question: "Bug xuất phát từ <root_layer>, không phải <symptom_layer>. Fix ở đâu?"
  header: "Root cause"
  options:
    - label: "Fix gốc (<root_layer>)", description: "Sửa đúng nguyên nhân gốc — recommended"
    - label: "Fix cả hai", description: "Sửa gốc + patch tạm ở <symptom_layer>"
    - label: "Patch tạm (<symptom_layer>)", description: "Chỉ sửa triệu chứng — root cause vẫn còn"
  multiSelect: false

---

## Step 3: Create minimal fix plan

Create a fix plan with 1–3 tasks. Each task must be:
- Independently verifiable (has an acceptance command)
- Scoped to <10k tokens of work
- Targeted — no scope creep beyond the bug

For cross-layer bugs: always include the root cause layer AND any affected contract/type layers (API types, shared interfaces). If the symptom layer only needs a type import update, include it. Do NOT create a plan that only patches the symptom without fixing root cause.

If cross-layer fix is needed, tasks should be ordered by dependency:
1. Fix root cause layer first
2. Update contracts/types/schemas
3. Fix symptom layer (if separate patch needed)

Initialize session state:

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

If no active session exists, auto-create a fix session. Derive the slug from the bug context (root cause summary from Step 2) — the user never types it. Derive slug by extracting 2-4 key nouns/verbs from the root cause summary, joined by hyphens, lowercase, max 32 chars. Examples: 'null-pointer-user-service', 'broken-login-redirect', 'missing-auth-header'.

```bash
# SLUG auto-derived from bug summary (e.g. "null-pointer-user-service", "broken-login-redirect")
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session init fix "$SLUG")
# → { "id": "fix/null-pointer-user-service", ... }
```

### Git context check

Apply the shared git-context module from `git-context.md`:

1. Run Part A (detect branching context) — detect base branch, current branch, dirty state
2. Run Part B (git state check) — handle dirty state, create/checkout branch for bugfix
3. Run Part D (stash recovery) — notify if stashed work exists for this task

The expected branch is derived from `SESSION_ID` (e.g., `fix/null-pointer-user-service`). For gitflow repos, fix branches are created from `main`/`master` (not `develop`).

Write a minimal `plan.json` to `$SESSION_DIR/plan.json` with:
- `task_type: "fix"`
- `status: "cooking"`
- 1–3 tasks covering the fix

Show the plan to the user:

```
Bug fix plan: <bug summary>

  Root cause: <layer> — <description>

  Tasks:
    [T-01] <fix description>  — <acceptance command>
    [T-02] <fix description>  — <acceptance command>  (if needed)

Proceed? (yes/no)
```

Only continue when user confirms.

---

## Step 4: Implement fixes

### Model selection + config metadata

```bash
MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model worker)
INTERACTION=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . interaction_level)
CONFIG=$("$HOANGSA_ROOT/bin/hoangsa-cli" config get .)
```

Extract from config:
- `codebase.testing` → test frameworks and config — used to build acceptance commands if not specified in plan
- `codebase.packages` → package build/test commands for verification

**Apply `interaction_level`:**
- `"detailed"` → show full cross-layer trace, all affected files with reasoning
- `"concise"` → show root cause + fix plan only, skip trace details
- `null` → default to `"detailed"`

### For each task:

Spawn a subagent using the `Task` tool with this prompt:

```
You are a HOANGSA worker. Execute this fix task precisely.

Task: <task.name>
ID: <task.id>
Workspace: <workspace_dir>
Thoth: <THOTH_STATUS — THOTH_AVAILABLE or THOTH_UNAVAILABLE>

Files to modify:
<task.files — list>

Context to read first:
<task.context_pointers — list>

Bug context:
<root cause summary from Step 2>

Cross-layer notes:
<if applicable — trace path and contract mismatches>

Instructions:
1. Read all context_pointers files first
2. Use memory_symbol_context({name: "buggySymbol"}) to understand all callers and callees before fixing (if Thoth is available)
3. Run memory_impact({target: "symbolName", direction: "upstream"}) on every symbol you modify — if HIGH/CRITICAL, report to orchestrator
4. Implement the minimal fix — do not change anything outside scope
5. Run the acceptance command to verify: <task.acceptance>
6. If acceptance fails, fix and retry (max 3 attempts)
7. Commit with message: "fix(<scope>): <task.name>" — `<scope>` = primary module/package from task.files, NOT session_id/branch name
8. After committing, verify change scope:
   memory_detect_changes({diff: "<git diff of your commit>"})
   Confirm only expected symbols were affected — hotfix must be minimal.

Acceptance command: <task.acceptance>
```

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
Commit fixes with message: "refactor(<scope>): simplify <task.id>" — `<scope>` = primary module/package from changed files, NOT session_id/branch name
```

3. If the simplify pass finds and fixes issues → mark task as `✅ completed (simplified)`
4. If no issues found → mark task as `✅ completed`

**Important:** The simplify pass runs sequentially after each worker (not in parallel with other workers). This ensures the simplified code is what subsequent tasks see.

Track progress:

```
Fixing...

  T-01 <fix description>  ✅ / ✅ ✨ / 🔄 / ❌
  T-02 <fix description>  ✅ / ✅ ✨ / 🔄 / ❌
```

---

## Step 4b: Persist bug lesson (if Thoth available)

After a successful fix, persist the root cause as a lesson so future agents avoid the same trap:

```
memory_remember_lesson({
  when: "touching <module/file where bug lived>",
  then: "<root cause pattern — what was wrong and why>",
  stage: true
})
```

### Update invalidated facts

If the bug revealed that an existing fact in MEMORY.md is wrong (e.g., "X uses Y" was incorrect):

```
memory_replace({kind: "fact", old_text: "<incorrect fact substring>", new_text: "<corrected fact>"})
```

This keeps the memory accurate — bugs often expose incorrect assumptions that were persisted as facts.

### Skill proposal check

If this fix revealed a pattern seen in ≥5 existing lessons (e.g., same module keeps breaking, same root cause pattern):

1. Check LESSONS.md for clusters of related lessons
2. If cluster ≥5 with good success rates → `memory_skill_propose({slug: "<pattern-name>", body: "<SKILL.md content>", source_triggers: ["<trigger1>", "<trigger2>", ...]})`
3. Report draft to user

Skip this step if Thoth is unavailable or the fix was trivial (typo, missing import).

### Save fix summary for future reference

```
memory_turn_save({role: "assistant", text: "Fix: <bug summary> | Root cause: <root cause layer> — <description> | Files: <changed files>"})
```

This creates a searchable record of the fix that future `memory_archive_search` can find.

---

## Step 5: Auto-chain to taste

After all fix tasks complete, automatically chain to `/hoangsa:taste` to verify the fix did not introduce regressions.

If taste reports failures after this fix attempt, do NOT auto-chain back to fix. Present results to user and let them decide: retry /hoangsa:fix, fix manually, or skip.

Inform the user:

```
Fix applied. Running tests via /hoangsa:taste...
```

Then invoke the taste workflow.

---

## Step 6: Sync-back to task manager

If an external task was linked (Step 0c), after taste completes:

1. Compose a fix summary comment:

```markdown
## 🔧 Bug Fix — HOANGSA

**Session:** <session_id>
**Root cause:** <one-line from Step 2c>
**Origin layer:** <layer where root cause was>
**Symptom layer:** <layer where bug appeared>

### Fix applied
- T-01: <description> ✅
- T-02: <description> ✅

### Files changed
| File | Action |
|------|--------|
| `src/api/handler.ts` | MODIFIED — fixed response shape |
| `src/components/UserList.tsx` | MODIFIED — updated type handling |

### Test results
- ✅ All acceptance tests passed
- ✅ No regressions detected

### Commits
- `abc1234` fix: correct API response shape
- `def5678` fix: update frontend type handling
```

2. Ask user what to sync (via `/serve` push mode — Step 5c):

Use AskUserQuestion:
  question: "Bug fixed. Cập nhật gì lên task manager?"
  header: "Sync"
  options:
    - label: "Status → Done + Comment", description: "Đóng task + thêm comment tóm tắt fix — recommended"
    - label: "Status → In Review", description: "Chuyển sang review, chưa đóng"
    - label: "Comment only", description: "Thêm comment nhưng không đổi status"
    - label: "Skip", description: "Không sync lần này"
  multiSelect: false

3. Execute sync via MCP based on user's choice
4. Report result with link back to task

---

## Self-verification checklist

Before reporting completion, output this table. Every row MUST show DONE or SKIPPED:

```
| Step | Status |
|------|--------|
| 0. Setup (lang + Thoth + task link) | DONE / SKIPPED |
| 1. Analyze bug | DONE / SKIPPED |
| 2. Cross-layer trace | DONE / SKIPPED |
| 3. Confirm fix plan | DONE / SKIPPED |
| 4. Implement fixes | DONE / SKIPPED |
| 5. Chain to taste | DONE / SKIPPED |
| 6. Report + sync | DONE / SKIPPED |
```

If any step shows SKIPPED without explicit user approval, go back and complete it before stopping.

---

## Rules

| Rule | Detail |
|------|--------|
| **Minimal fix only** | Do not refactor or expand scope |
| **1–3 tasks max** | Keep it tight — hotfix, not a feature |
| **Confirm before implementing** | Show plan, ask yes/no |
| **Always chain to taste** | Verify no regressions after fix |
| **Fresh context per task** | Core HOANGSA principle — never compromise |
| **Cross-layer tracing is default** | Always trace if multi-layer project and root cause unclear |
| **Sync-back if task linked** | Auto-ask user what to update on task manager after fix |
| **Root cause over symptoms** | Recommend fixing the origin layer, not just patching symptoms |
