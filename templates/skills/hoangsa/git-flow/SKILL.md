---
name: git-flow
description: "This skill should be used when the user wants to start a new task, switch between tasks, park or resume work, finish a task (push + PR), clean up merged branches, or sync with upstream. Triggers on phrases like 'start task', 'new task', 'work on', 'switch to', 'park this', 'resume', 'continue where I left off', 'done with this', 'finish task', 'create PR', 'clean branches', 'delete merged', 'rebase', 'pull latest', or 'sync with main'."
---

<objective>
Manage git branching, task switching, and code lifecycle. Complements `/plate` (commit) and `/serve` (sync) by handling everything around them: branch creation, dirty state detection, task transitions, PR creation, and cleanup.
</objective>

<triggers>
- "start new task" / "new feature" / "work on TASK-123"
- "switch to task X" / "let me work on something else"
- "park this" / "save for later" / "I'll come back to this"
- "resume" / "continue where I left off" / "back to task X"
- "done with this" / "finish" / "create PR" / "push and PR"
- "clean up branches" / "delete merged branches"
- "pull latest" / "rebase" / "sync with main"
- "I have uncommitted changes" / "is it safe to switch?"
</triggers>

<detection>
**Core Principle:** Detect, don't assume. Infer branching strategy, naming conventions, and base branch from the repository's git history. Ask the user only when detection is ambiguous.

Run once per session before any flow:

```bash
# 1. Detect base branch
git symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@'
# Fallback: check for main, master, develop

# 2. Detect branching strategy from existing branches
git branch -a --format='%(refname:short)'

# 3. Detect naming convention from recent branches
git branch --sort=-committerdate --format='%(refname:short)' | head -20

# 4. Current state
git status --porcelain
git stash list
git log --oneline -5
```

**Strategy detection rules:**
- Branches like `develop`, `release/*`, `hotfix/*` → **gitflow**
- Only `main`/`master` + `feature/*` → **trunk-based**
- Mixed or unclear → ask user

**Naming convention detection:**
- Extract prefix patterns: `feature/`, `feat/`, `fix/`, `bugfix/`, `chore/`
- Extract separator: `-`, `_`, `/`
- Extract task ID pattern: `PROJ-123`, `#123`, no ID
- Use the most common pattern found
</detection>

<flows>
<flow name="start-task">
Trigger: "start task", "new task", "work on X"

1. Run detection (above)
2. Ask what the task is (or extract from context/external task)
3. Generate branch name using detected convention
4. Determine base branch:
   - gitflow feature → from `develop`
   - gitflow hotfix → from `main`/`master`
   - trunk-based → from `main`/`master`
5. Check for dirty state → if dirty, trigger Park flow first
6. Create and checkout branch:
   ```bash
   git checkout -b <branch-name> <base-branch>
   ```
7. Confirm to user: branch name, base, ready to work
</flow>

<flow name="switch-task">
Trigger: "switch to", "work on X instead", user starts talking about different task

1. Detect dirty state (`git status --porcelain`)
2. If dirty → ask user:
   - **Commit** → chain to `/plate`, then switch
   - **Stash** → `git stash push -m "WIP: <current-task-context>"`
   - **Discard** → confirm twice, then `git checkout -- .`
3. Switch to target:
   - Branch exists → `git checkout <branch>`
   - Branch doesn't exist → trigger Start Task flow
4. If target branch has stashed work → notify user, offer to pop
</flow>

<flow name="park-work">
Trigger: "park this", "save for later", "I need to step away"

Two strategies — ask user preference (save to config on first ask):

- **WIP commit** (default): stage all → commit with `wip: <context>` → can be squashed later
- **Stash**: `git stash push -m "PARK: <task-context> [<branch>]"`

```bash
# WIP commit approach
git add -A
git commit -m "wip: <task-description>"

# Stash approach
git stash push -m "PARK: <task-description> [$(git branch --show-current)]"
```

After parking, optionally switch to another branch (ask user).
</flow>

<flow name="resume-work">
Trigger: "resume", "continue", "back to task X", "where was I"

1. Find parked work:
   ```bash
   # Check for WIP commits on branches
   git branch --sort=-committerdate --format='%(refname:short)' | while read b; do
     git log -1 --format='%s' "$b" 2>/dev/null | grep -q '^wip:' && echo "$b"
   done

   # Check stash list
   git stash list | grep 'PARK:'
   ```
2. Show user what's parked with context
3. User selects which to resume
4. Handle dirty state on current branch (Switch Task logic)
5. Checkout target branch
6. If stash → `git stash pop`
7. If WIP commit → notify user (they can `git reset HEAD~1` to unstage, or keep working and squash later)
</flow>

<flow name="finish-task">
Trigger: "done", "finish", "create PR", "push this"

1. If uncommitted changes → chain to `/plate`
2. Push branch:
   ```bash
   git push -u origin $(git branch --show-current)
   ```
3. Create PR:
   ```bash
   gh pr create --title "<title>" --body "<body>" --base <base-branch>
   ```
   - Title: from branch name or task description
   - Body: summary of commits, link to external task if exists
   - Base: detected base branch
4. Show PR URL to user
5. Chain to `/serve` if external task linked (update status to "In Review")
6. Ask: stay on branch or switch to base?
</flow>

<flow name="cleanup">
Trigger: "clean branches", "delete merged", "prune"

```bash
# Find merged branches
git branch --merged <base-branch> --format='%(refname:short)' | grep -v -E '^(main|master|develop)$'

# Show list, confirm with user
# Delete confirmed branches
git branch -d <branch>

# Prune remote tracking
git remote prune origin
```
</flow>

<flow name="sync-upstream">
Trigger: "pull latest", "rebase", "sync", "update from main"

1. Detect preferred strategy (rebase vs merge) from git log:
   ```bash
   # If merge commits exist → merge strategy
   git log --oneline --merges -5
   ```
2. Stash dirty changes if needed
3. Execute:
   ```bash
   # Rebase strategy (default for feature branches)
   git fetch origin
   git rebase origin/<base-branch>

   # Merge strategy
   git fetch origin
   git merge origin/<base-branch>
   ```
4. Handle conflicts: show conflicted files, help resolve
5. Pop stash if stashed
</flow>
</flows>

<rules>
| Rule | Detail |
|------|--------|
| **Detect, don't assume** | Infer strategy and conventions from git history |
| **Never lose work** | Always handle dirty state before branch operations |
| **Confirm destructive actions** | Double-confirm discards, force-pushes, branch deletes |
| **Chain, don't duplicate** | Use `/plate` for commits, `/serve` for sync |
| **Save preferences** | Ask once, save to config, don't repeat |
| **User's language** | Respect `lang` pref for all user-facing text |
| **Dirty State Guard** | Before ANY branch operation, always check `git status --porcelain`. If output is non-empty, handle dirty state BEFORE proceeding. Never silently discard changes. |
</rules>

<preferences>
| Key | Values | Default |
|-----|--------|---------|
| `git_park_strategy` | `wip_commit`, `stash` | `wip_commit` |
| `git_sync_strategy` | `rebase`, `merge` | auto-detect |

Read/write via:
```bash
"$HOANGSA_ROOT/bin/hoangsa-cli" pref get . <key>
"$HOANGSA_ROOT/bin/hoangsa-cli" pref set . <key> <value>
```
</preferences>

<integration>
This skill is integrated into HOANGSA workflows via the shared `git-context.md` module:

| Workflow | Integration Point | Behavior |
|----------|-------------------|----------|
| `/menu` | Step 1c (after session init) | Create branch for new feature |
| `/fix` | Step 3 (after session init) | Create branch for bugfix |
| `/cook` | Step 1c (before execution) | Verify correct branch |
| `/plate` | Step 6 (after commit) | Push + PR + switch options |

The skill provides the knowledge; `git-context.md` provides the executable steps that workflows reference.
</integration>

<references>
- **`references/flows.md`** — Edge cases, conflict resolution guides, dirty state decision tree, PR templates, and advanced scenarios (interactive rebase, cherry-pick, finding lost work)
</references>
