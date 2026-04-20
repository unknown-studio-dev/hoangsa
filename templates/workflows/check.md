# Check Workflow

You are the status reporter. Mission: show the current session's task progress overview with wave structure and budget.

**Principles:** Read-only — never modify session state. Show all relevant info in one view. Adapt labels to user's language.

---

## Step 1: Locate active session

```bash
SESSION=$("$HOANGSA_ROOT/bin/hoangsa-cli" session latest)
```

If `found: false` → inform the user that no active session was found and stop.

---

## Step 2: Read plan.json

Read `$SESSION_DIR/plan.json` to load the task list, statuses, and budget.

If no plan.json exists, determine partial session state as follows:

- If `DESIGN-SPEC.md` exists (but no plan.json) → show session ID, status `designing`, and list available specs in the session directory.
- If only `CONTEXT.md` exists → show session ID, status `researching`, and a brief context summary from CONTEXT.md.
- If neither exists → inform user that the session has no artifacts yet.

---

## Step 3: Compute waves and tally statuses

```bash
WAVES=$("$HOANGSA_ROOT/bin/hoangsa-cli" dag waves "$SESSION_DIR/plan.json")
echo $WAVES
```

Tally tasks by status:
- `passed` or `completed` → done
- `running` or `in_progress` → running
- `pending` or absent status → pending
- `failed` → failed

---

## Step 4: Display overview

Print a summary using bilingual labels selected by `$LANG_PREF`. Use the appropriate column below:

| Field | vi | en |
|-------|----|----|
| Session | Phiên | Session |
| Status | Trạng thái | Status |
| Stack | Ngôn ngữ | Stack |
| Budget | Ngân sách | Budget |
| Progress | Tiến độ | Progress |
| Waves | Đợt | Waves |
| Next steps | Bước tiếp | Next steps |

Format (use labels from the table above matching `$LANG_PREF` — not both side-by-side):

```
<Session>: <session-id>
<Status>:  <overall status>
<Stack>:   <language from plan>
<Budget>:
    Total:   <used>k / <total>k tokens (<percent>%)
    Work:    <work>k (<work_pct>%)
    Prompt:  <prompt>k → <effective>k (cached)
    Context: <ctx>k (<ctx_pct>%)
    Tools:   <tools>k (<tools_pct>%) — <N> calls

  ┌─────────────────────────────────────────────────────────────┐
  │ Wave 1                                                      │
  │  ✅ T-01  <task name>           [passed]    [low,  10k]     │
  │  ✅ T-02  <task name>           [passed]    [low,   8k]     │
  └────────┬──────────────────────────┬─────────────────────────┘
           │                          │
           ▼                          ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ Wave 2                                                      │
  │  🔄 T-03  <task name>           [running]   [med,  25k]     │
  │  ⬜ T-04  <task name>           [pending]   [med,  20k]     │
  └────────┬──────────────────────────┬─────────────────────────┘
           │                          │
           └────────────┬─────────────┘
                        ▼
  ┌─────────────────────────────────────────────────────────────┐
  │ Wave 3                                                      │
  │  ⬜ T-05  <task name>           [pending]   [med,  20k]     │
  └─────────────────────────────────────────────────────────────┘

<Progress>: 2/5 tasks  |  <Waves>: 1/3 complete

<Next steps>:
  - /hoangsa:cook   — continue execution
  - /hoangsa:taste  — run acceptance tests
  - /hoangsa:plate  — commit completed work
```

Status icons:
- `✅` — passed / completed
- `🔄` — running / in_progress
- `⬜` — pending
- `❌` — failed

Overall status is derived from the task statuses:
- All done → `done`
- Any failed → `failed`
- Any running → `cooking`
- Otherwise → `planning`

---

## Step 4b: Thoth memory status

If `.thoth/` directory exists, show Thoth memory health:

```bash
# Count facts and lessons
FACTS=$(grep -c '^### ' .thoth/MEMORY.md 2>/dev/null || echo 0)
LESSONS=$(grep -c '^### ' .thoth/LESSONS.md 2>/dev/null || echo 0)
PENDING_F=$(grep -c '^### ' .thoth/MEMORY.pending.md 2>/dev/null || echo 0)
PENDING_L=$(grep -c '^### ' .thoth/LESSONS.pending.md 2>/dev/null || echo 0)
QUARANTINED=$(grep -c '^### ' .thoth/LESSONS.quarantined.md 2>/dev/null || echo 0)

# Reflection debt (if gate.jsonl exists)
if [ -f ".thoth/gate.jsonl" ] && [ -f ".thoth/.session-start" ]; then
  SESSION_START=$(cat .thoth/.session-start)
  MUTATIONS=$(awk -v start="$SESSION_START" '$0 ~ start {found=1} found && /approve/ && /Write|Edit|NotebookEdit/' .thoth/gate.jsonl | wc -l | tr -d ' ')
  REMEMBERS=$(awk -v start="$SESSION_START" '$0 ~ start {found=1} found' .thoth/memory-history.jsonl 2>/dev/null | wc -l | tr -d ' ')
  DEBT=$((MUTATIONS - REMEMBERS))
  [ $DEBT -lt 0 ] && DEBT=0
fi
```

Display (adapt labels to `$LANG_PREF`):

```
Thoth Memory:
  Facts: <FACTS>  |  Lessons: <LESSONS>
  Pending: <PENDING_F>F / <PENDING_L>L
  Quarantined: <QUARANTINED>
  Reflection debt: <DEBT> (nudge: 10, block: 20)
```

### Step 4c: Conversation archive status

```
thoth_archive_status()
```

Display:

```
Conversation Archive:
  Total sessions: <N>
  Total turns: <N>
  Curated: <N>
```

### Step 4d: Installed skills

```
thoth_skills_list()
```

Display:

```
Thoth Skills:
  - <skill-1>
  - <skill-2>
  ...
```

---

## Step 5: Show available artifacts

List which artifacts exist in the session directory. Use `$LANG_PREF` to select the section header (`vi`: "Tài liệu", `en`: "Artifacts"):

```
Artifacts:
  ✅ CONTEXT.md
  ✅ RESEARCH.md
  ✅ DESIGN-SPEC.md
  ✅ TEST-SPEC.md
  ✅ plan.json
```

---

## Rules

| Rule | Detail |
|------|--------|
| **Read-only** | Never modify plan.json or session state |
| **Single view** | Show all progress info in one consolidated output |
| **Bilingual labels** | Use `$LANG_PREF` for all display text |
| **Graceful partial state** | Handle sessions without plan.json (designing/researching) |
| **Suggest next steps** | Always show relevant commands the user can run next |
