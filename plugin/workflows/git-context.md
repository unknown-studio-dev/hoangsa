# Git Context — Shared Module

Referenced by HOANGSA workflows that create or resume sessions. Not a standalone workflow.

---

## When to apply

Every workflow that creates or resumes a session should check git context. The behavior differs by phase:

| Flow | Phase | Action |
|------|-------|--------|
| `/menu` | Session init (Step 1b) | Create branch for new task |
| `/fix` | Session init (Step 3) | Create branch for bugfix |
| `/cook` | Before execution (Step 1) | Verify correct branch |
| `/plate` | After commit (Step 6) | Offer push + PR + switch |

---

## Part A: Branch Detection (run once per session start)

### A1. Detect repository branching context

```bash
# Base branch
BASE_BRANCH=$(git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
if [ -z "$BASE_BRANCH" ]; then
  # Fallback: check common names
  for b in main master develop; do
    git show-ref --verify --quiet refs/heads/$b && BASE_BRANCH=$b && break
  done
fi

# Current branch
CURRENT_BRANCH=$(git branch --show-current)

# Dirty state
DIRTY=$(git status --porcelain)

# Branching strategy (from existing branches)
HAS_DEVELOP=$(git show-ref --verify --quiet refs/heads/develop && echo "yes" || echo "no")
HAS_RELEASE=$(git branch -a --format='%(refname:short)' | grep -q 'release/' && echo "yes" || echo "no")

# Naming convention (from recent feature branches)
RECENT_BRANCHES=$(git branch --sort=-committerdate --format='%(refname:short)' | \
  grep -v -E '^(main|master|develop|HEAD)$' | head -10)
```

### A2. Determine expected branch for session

Map session to branch name using detected conventions:

```
Session ID: feat/api-authentication     → Branch: feat/api-authentication
Session ID: fix/null-pointer-user       → Branch: fix/null-pointer-user
Session ID: refactor/install-cleanup    → Branch: refactor/install-cleanup
```

If existing branches use a different pattern (e.g., `feature/` instead of `feat/`), adapt:

```bash
# Check if repo uses feature/ prefix instead of feat/
if echo "$RECENT_BRANCHES" | grep -q '^feature/'; then
  # Adapt: feat/x → feature/x
  EXPECTED_BRANCH=$(echo "$SESSION_ID" | sed 's|^feat/|feature/|')
elif echo "$RECENT_BRANCHES" | grep -q '^bugfix/'; then
  # Adapt: fix/x → bugfix/x
  EXPECTED_BRANCH=$(echo "$SESSION_ID" | sed 's|^fix/|bugfix/|')
else
  # Use session ID directly as branch name
  EXPECTED_BRANCH="$SESSION_ID"
fi
```

### A3. Determine base branch for new branches

| Strategy | Task type | Base branch |
|----------|-----------|-------------|
| gitflow (HAS_DEVELOP=yes) | feat, refactor, chore, docs | `develop` |
| gitflow | fix, hotfix | `main`/`master` |
| trunk-based | all | `main`/`master` |

---

## Part B: Git State Check (at session start)

Run after session is created/resumed. Handle 4 scenarios:

### Scenario 1: Already on correct branch, clean state

```
CURRENT_BRANCH == EXPECTED_BRANCH && DIRTY is empty
→ Continue. No action needed.
```

### Scenario 2: Already on correct branch, dirty state

```
CURRENT_BRANCH == EXPECTED_BRANCH && DIRTY is not empty
→ This is work-in-progress on the right branch. Continue — changes will be committed via /plate later.
```

### Scenario 3: On wrong branch, clean state

```
CURRENT_BRANCH != EXPECTED_BRANCH && DIRTY is empty
```

Check if expected branch exists:

```bash
git show-ref --verify --quiet refs/heads/$EXPECTED_BRANCH
```

- Branch exists → `git checkout $EXPECTED_BRANCH`
- Branch doesn't exist → create it:

  Ask user to confirm:

  Use AskUserQuestion:
    question:
      vi: "Tạo branch '$EXPECTED_BRANCH' từ '$CREATE_BASE' cho task này?"
      en: "Create branch '$EXPECTED_BRANCH' from '$CREATE_BASE' for this task?"
    header: "Git Branch"
    options:
      - label:
          vi: "Tạo branch"
          en: "Create branch"
        description:
          vi: "git checkout -b $EXPECTED_BRANCH $CREATE_BASE"
          en: "git checkout -b $EXPECTED_BRANCH $CREATE_BASE"
      - label:
          vi: "Làm trên branch hiện tại"
          en: "Stay on current branch"
        description:
          vi: "Tiếp tục trên '$CURRENT_BRANCH' — không tạo branch mới"
          en: "Continue on '$CURRENT_BRANCH' — no new branch"
    multiSelect: false

  If create:
  ```bash
  git checkout -b "$EXPECTED_BRANCH" "$CREATE_BASE"
  ```

  If stay → store preference, continue on current branch.

### Scenario 4: On wrong branch, dirty state (CRITICAL)

```
CURRENT_BRANCH != EXPECTED_BRANCH && DIRTY is not empty
```

**This is the most important scenario** — user has uncommitted changes from another task and wants to start new work.

Ask user:

Use AskUserQuestion:
  question:
    vi: "Có thay đổi chưa commit trên branch '$CURRENT_BRANCH'. Xử lý thế nào?"
    en: "Uncommitted changes on branch '$CURRENT_BRANCH'. How to handle?"
  header: "Dirty State"
  options:
    - label:
        vi: "Commit rồi chuyển"
        en: "Commit then switch"
      description:
        vi: "Chạy /plate để commit, sau đó chuyển sang branch mới"
        en: "Run /plate to commit, then switch to new branch"
    - label:
        vi: "Stash rồi chuyển"
        en: "Stash then switch"
      description:
        vi: "git stash push — lưu tạm, lấy lại sau bằng git stash pop"
        en: "git stash push — save temporarily, restore later with git stash pop"
    - label:
        vi: "Làm trên branch hiện tại"
        en: "Stay on current branch"
      description:
        vi: "Không chuyển branch — commit những thay đổi này cùng task mới"
        en: "Don't switch — commit these changes with the new task"
  multiSelect: false

Handle choice:

- **Commit then switch:**
  1. Chain to `/plate` workflow (commit current changes)
  2. Then proceed to Scenario 3 logic (checkout/create branch)

- **Stash then switch:**
  ```bash
  git stash push -m "WIP: work on $CURRENT_BRANCH before switching to $EXPECTED_BRANCH"
  ```
  Then proceed to Scenario 3 logic.

- **Stay:** Continue on current branch. Log a note that changes from a different context exist.

---

## Part C: Post-Commit Extension (for /plate workflow)

After plate commits successfully, add these options before the existing serve chain:

### C1. Push + PR check

```bash
# Check if branch has upstream
UPSTREAM=$(git rev-parse --abbrev-ref --symbolic-full-name @{u} 2>/dev/null)

# Check if there are unpushed commits
UNPUSHED=$(git log @{u}..HEAD --oneline 2>/dev/null | wc -l | tr -d ' ')
```

### C2. Offer next action

If on a feature/fix branch (not main/master/develop):

Use AskUserQuestion:
  question:
    vi: "Commit xong. Tiếp theo?"
    en: "Committed. What's next?"
  header: "Next Step"
  options:
    - label:
        vi: "Tiếp tục làm"
        en: "Keep working"
      description:
        vi: "Ở lại branch này, tiếp tục code"
        en: "Stay on this branch, keep coding"
    - label:
        vi: "Push"
        en: "Push"
      description:
        vi: "Push branch lên remote"
        en: "Push branch to remote"
    - label:
        vi: "Push + Tạo PR"
        en: "Push + Create PR"
      description:
        vi: "Push rồi tạo Pull Request"
        en: "Push then create Pull Request"
    - label:
        vi: "Chuyển task khác"
        en: "Switch to another task"
      description:
        vi: "Push branch hiện tại rồi checkout sang task khác"
        en: "Push current branch then checkout to another task"
  multiSelect: false

Handle choice:

- **Keep working:** Continue — proceed to existing serve chain if applicable.

- **Push:**
  ```bash
  git push -u origin $(git branch --show-current)
  ```
  Then proceed to serve chain.

- **Push + Create PR:**
  ```bash
  git push -u origin $(git branch --show-current)
  ```
  Generate PR title from session/task context:
  ```bash
  gh pr create --title "<type>(<scope>): <description>" \
    --body "$(cat <<'PREOF'
  ## Summary
  <bullet points from session tasks>

  ## Test plan
  <from taste results if available>

  ---
  Session: <session_id>
  PREOF
  )" --base "$BASE_BRANCH"
  ```
  Show PR URL. Then proceed to serve chain.

- **Switch to another task:**
  ```bash
  git push -u origin $(git branch --show-current)
  ```
  Then ask what to switch to:

  Use AskUserQuestion:
    question:
      vi: "Chuyển sang task nào?"
      en: "Switch to which task?"
    header: "Switch Task"
    options:
      - label:
          vi: "Branch có sẵn"
          en: "Existing branch"
        description:
          vi: "Chọn từ danh sách branches"
          en: "Choose from branch list"
      - label:
          vi: "Task mới"
          en: "New task"
        description:
          vi: "Tạo branch mới cho task mới"
          en: "Create new branch for new task"
      - label:
          vi: "Về $BASE_BRANCH"
          en: "Back to $BASE_BRANCH"
        description:
          vi: "Checkout về branch chính"
          en: "Checkout to base branch"
    multiSelect: false

  Handle:
  - **Existing branch:** List recent branches, let user pick, checkout.
  - **New task:** Ask for description, derive branch name, create and checkout.
  - **Back to base:** `git checkout $BASE_BRANCH && git pull`

---

## Part D: Stash Recovery Notification

At session start (Part B), also check for relevant stashes:

```bash
# Check if there are stashes from this branch or session
git stash list | grep -i "$EXPECTED_BRANCH\|$SESSION_ID"
```

If found, notify user:

```
💡 Found stashed work related to this task:
  stash@{0}: WIP: work on feat/api-auth before switching to fix/login-bug

  Apply? (git stash pop)
```

---

## Integration instructions for each workflow

### For `/menu` (Step 1b — after session init)

After `hoangsa-cli session init` creates the session:
1. Run Part A (detect branching context)
2. Run Part B (git state check) — creates branch for new feature
3. Run Part D (stash recovery)
4. Continue to Step 2 (gather requirements)

### For `/fix` (Step 3 — after session init)

After session is created in Step 3:
1. Run Part A (detect branching context)
2. Run Part B (git state check) — creates branch for bugfix
3. Run Part D (stash recovery)
4. Continue to Step 4 (implement fixes)

### For `/cook` (Step 1 — before execution)

After plan is loaded:
1. Run Part A (detect branching context)
2. Run Part B (git state check) — verify correct branch, switch if needed
3. Run Part D (stash recovery)
4. Continue to Step 2 (execute tasks)

### For `/plate` (Step 6 — after commit)

After commit succeeds:
1. Run Part C (post-commit extension) — push/PR/switch options
2. Then proceed to existing serve chain logic
