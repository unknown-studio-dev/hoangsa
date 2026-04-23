# HOANGSA Ship Workflow

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules + CLI reference + self-verification template.

You are the ship orchestrator. Mission: review code changes (code + security) in parallel, gate on quality, then push or create PR.

**Principles:** Never push without review. Block on critical issues. User always has final say.

---

## Step 0b: Model selection + config metadata

```bash
REVIEWER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model reviewer)
CONFIG=$("$HOANGSA_ROOT/bin/hoangsa-cli" config get .)
```

Use the `reviewer` model for code and security review agents. Extract `codebase.ci` from config — used in Step 5 to hint user to check CI after push.

---

## Step 1: Git state analysis

```bash
# Check current branch, base branch, uncommitted changes, unpushed commits
BRANCH=$(git branch --show-current)
BASE_BRANCH=$(git remote show origin 2>/dev/null | grep 'HEAD branch' | awk '{print $NF}')
UNCOMMITTED=$(git status --porcelain | wc -l | tr -d ' ')
UNPUSHED=$(git log @{u}..HEAD --oneline 2>/dev/null | wc -l | tr -d ' ')
DIFF_STAT=$(git diff --stat $BASE_BRANCH...HEAD 2>/dev/null)
```

### Step 1a: Validate git state

- If `UNCOMMITTED > 0` → ask user: "Có thay đổi chưa commit. Commit trước khi ship?" with options:
  - "Chạy /hoangsa:plate trước"
  - "Bỏ qua — ship những gì đã commit"

- If `UNPUSHED == 0` and `UNCOMMITTED == 0` → inform "Không có gì để ship." and stop.

- Otherwise show summary:

```
📦 Ready to ship:
   Branch: <BRANCH>
   Base:   <BASE_BRANCH>
   Unpushed commits: <UNPUSHED>
   Changed files: <from DIFF_STAT>
```

### Step 1b: Blast-radius analysis

Run change detection to understand the impact scope of the branch:

```
memory_detect_changes({diff: "$(git diff $BASE_BRANCH...HEAD)"})
```

Present blast-radius summary alongside the git state:

```
Impact analysis:
  Symbols changed: <N> (d=0)
  Direct dependents: <N> (d=1 — WILL BREAK if API changed)
  Indirect dependents: <N> (d=2 — should test)

  High-risk symbols:
    - <symbol> — <N> direct callers
    - <symbol> — <N> direct callers
```

If any d=1 dependents exist for changed symbols, flag them in the review as "high-risk changes."

---

## Step 2: Parallel review

Launch 2 agents in parallel using the Agent tool:

**Agent A — Code Review:**
Spawn a subagent that:
- Gets the diff: `git diff $BASE_BRANCH...HEAD`
- Reviews for: bugs, logic errors, code quality, naming, dead code
- Uses `memory_detect_changes` blast-radius output to prioritize review on high-impact symbols
- Runs `memory_symbol_context` on any symbol flagged as high-risk to understand its full dependency graph
- Uses confidence scoring (0-100 scale):
  - 0: False positive
  - 25: Might be real
  - 50: Moderate confidence
  - 75: High confidence — verified real issue
  - 100: Certain — confirmed critical
- Filters issues with confidence < 80
- Returns structured list: each issue has severity (CRITICAL/HIGH/MEDIUM/LOW), file, line, description, confidence

**Agent B — Security Review:**
Spawn a subagent that:
- Gets the diff: `git diff $BASE_BRANCH...HEAD`
- Reviews for: injection (SQL, command, XSS), authentication/authorization issues, secrets/credentials exposure, unsafe deserialization, path traversal, SSRF, insecure crypto, hardcoded secrets
- Uses `memory_impact` on security-sensitive functions (auth, crypto, input parsing) to verify no callers bypass validation
- Same confidence scoring as Agent A
- Returns structured list: each issue has severity, file, line, description, confidence

---

## Step 3: Aggregate results

Merge results from both agents into unified report:

```
## Ship Review Report

### Summary
- Code issues: X (Y critical, Z high)
- Security issues: X (Y critical, Z high)
- Verdict: PASS / BLOCKED

### Critical Issues (must fix)
1. [SECURITY] <description> — <file>:<line>
2. [CODE] <description> — <file>:<line>

### High Issues (should fix)
...

### Medium/Low Issues (consider)
...
```

Classify verdict:
- BLOCKED if any CRITICAL or HIGH issues
- PASS if only MEDIUM/LOW or no issues

---

## Step 4: Gate decision

If verdict is BLOCKED (CRITICAL or HIGH issues found):

Use AskUserQuestion:
  question: "Review phát hiện <N> issues nghiêm trọng. Xử lý thế nào?"
  header: "Blocked"
  options:
    - label: "Xem chi tiết + fix"
      description: "Xem từng issue, tự fix rồi chạy /hoangsa:ship lại"
    - label: "Override — ship anyway"
      description: "Bỏ qua warnings, tiếp tục push/PR (không khuyến khích)"
    - label: "Hủy"
      description: "Dừng lại, không push"

Handling:
- "Xem chi tiết + fix" → show full report with file:line details, exit (user fixes and runs /ship again)
- "Override" → continue to Step 5 with override warning flag
- "Hủy" → exit

If verdict is PASS → auto-proceed to Step 5.

---

## Step 5: Ship action

Use AskUserQuestion:
  question: "Review passed. Bạn muốn làm gì?"
  header: "Action"
  options:
    - label: "Push + tạo PR (Recommended)"
      description: "Push branch lên remote và tạo Pull Request với review summary"
    - label: "Chỉ push"
      description: "Push code lên remote, không tạo PR"
    - label: "Chỉ xem report"
      description: "Xem report chi tiết, không push"

### If "Push + tạo PR":

```bash
git push -u origin $(git branch --show-current)
```

Then create PR:

```bash
# Derive PR title from branch name using conventional commit format
PR_TITLE="<task_type>(<scope>): <description derived from branch name>"

gh pr create --title "$PR_TITLE" --body "$(cat <<'EOF'
## Summary
<bullet points from review report>

## Review Results
- Code review: <PASS/N issues found>
- Security review: <PASS/N issues found>
<if override was used: ⚠️ Shipped with override — review issues were acknowledged but not fixed>

## Test plan
<from session TEST-SPEC if available, otherwise "Manual testing required">

---
Generated with HOANGSA /ship
EOF
)"
```

Show PR URL to user after creation.

### If "Chỉ push":

```bash
git push -u origin $(git branch --show-current)
```

Show confirmation: "Pushed <N> commits to origin/<branch>"

### After any push (both "Push + tạo PR" and "Chỉ push"):

If `codebase.ci` from config (loaded in Step 0b) is non-null/non-empty, hint the user:

```
💡 CI: <ci platform> — check pipeline status after push
   <e.g., "GitHub Actions — check .github/workflows/ pipeline">
```

### If "Chỉ xem report":

Display full review report, exit without pushing.

## Step 6: Chain to serve (optional)

Follow the plate→serve chaining pattern:

```bash
# Check if external task is linked
STATE=$("$HOANGSA_ROOT/bin/hoangsa-cli" state get "$SESSION_DIR")
# Parse external_task from state
```

- If `external_task` exists in session state → auto-chain to `/hoangsa:serve` in push mode (sync PR URL + status back to task manager)
- If no external task → check `auto_serve` preference:
  - `true` → chain to /serve
  - `false` → skip
  - `null` (first time) → ask user, save preference

After chaining (or skipping), show final summary:

```
Ship complete!
   Branch:  <BRANCH>
   Action:  <push / push+PR / report only>
   Review:  <PASS / PASS with override>
   PR:      <URL if created>
```

---

## Step 7: Reflect to memory

Read `$HOANGSA_ROOT/templates/snippets/memory-reflect-end.md` and follow it.
The snippet decides whether anything from this ship is worth persisting — in
most runs the answer is no, and that is the correct outcome.

Apply with the review report + any user corrections in mind: a reviewer flag
the user accepted may be worth a lesson; a preference the user stated for
this PR's scope may be worth a preference.

---

## Rules

Universal rules live in `common.md §Universal rules`. Ship-specific additions:

| Rule | Detail |
|------|--------|
| **Never push without review** | Steps 2-3 always run before any push |
| **Block on CRITICAL/HIGH** | User must explicitly override |
| **User has final say** | Always ask, never auto-push |
| **Chain to serve** | If external task linked, sync results |
