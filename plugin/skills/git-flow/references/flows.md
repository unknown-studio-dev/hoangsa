# Git Flow — Detailed Reference

## Branching Strategy Detection

### Gitflow Pattern

Detected when repository has branches matching:
- `develop` or `development`
- `release/*` or `release-*`
- `hotfix/*` or `hotfix-*`
- `feature/*`

Branch creation rules:
- Features → from `develop`
- Hotfixes → from `main`/`master`
- Releases → from `develop`

### Trunk-Based Pattern

Detected when repository has:
- Only `main`/`master` as long-lived branch
- Short-lived feature branches
- No `develop` branch

Branch creation rules:
- All work → from `main`/`master`
- Branches are short-lived (days, not weeks)

### GitHub Flow Pattern

Similar to trunk-based but with:
- PR-centric workflow
- Deploy from `main` after merge
- No release branches

## Branch Naming Convention Detection

### Common Patterns

```
# Pattern: type/description
feature/add-user-auth
fix/login-redirect-loop
chore/update-deps

# Pattern: type/TASK-ID-description
feature/PROJ-123-add-user-auth
fix/BUG-456-login-redirect

# Pattern: type/ticket-number
feature/123
fix/456

# Pattern: username/type/description
john/feature/add-auth
```

### Detection Algorithm

```bash
# Get recent branches (exclude main/master/develop)
BRANCHES=$(git branch -a --sort=-committerdate --format='%(refname:short)' | \
  grep -v -E '^(main|master|develop|HEAD|origin/(main|master|develop|HEAD))$' | \
  head -20)

# Extract prefix pattern (most common first segment before /)
echo "$BRANCHES" | sed 's|/.*||' | sort | uniq -c | sort -rn | head -5

# Extract separator after prefix
echo "$BRANCHES" | sed 's|^[^/]*/||' | head -5
# Look for: kebab-case, snake_case, camelCase

# Extract task ID pattern
echo "$BRANCHES" | grep -oE '[A-Z]+-[0-9]+|#[0-9]+' | head -5
```

## Edge Cases

### Conflict Resolution During Rebase

When `git rebase` encounters conflicts:

1. Show conflicted files: `git diff --name-only --diff-filter=U`
2. For each file, show the conflict markers
3. Help user resolve (offer suggestions based on code understanding)
4. After resolution:
   ```bash
   git add <resolved-file>
   git rebase --continue
   ```
5. If user wants to abort: `git rebase --abort`

### Conflict Resolution During Merge

Similar to rebase but:
- Single conflict resolution pass (not commit-by-commit)
- After resolution: `git add <files> && git commit`
- Abort: `git merge --abort`

### Detached HEAD Recovery

If user is in detached HEAD state:

1. Detect: `git symbolic-ref HEAD 2>/dev/null` fails
2. Show current commit: `git log -1 --oneline`
3. Offer options:
   - Create branch from current position: `git checkout -b <name>`
   - Return to previous branch: `git checkout -`
   - Return to specific branch: `git checkout <branch>`

### Stash Conflicts

When `git stash pop` has conflicts:

1. Stash is NOT dropped (still in stash list)
2. Resolve conflicts manually
3. After resolution, manually drop: `git stash drop`

## Advanced Scenarios

### Interactive Rebase (Squash WIP Commits)

When user has multiple WIP commits to clean up before PR:

```bash
# Count WIP commits
WIP_COUNT=$(git log --format='%s' origin/<base>..HEAD | grep -c '^wip:')

# Offer to squash
git rebase -i HEAD~$WIP_COUNT
# In the editor: mark all but first as 'squash'
```

Since interactive rebase requires editor interaction, instead use:

```bash
# Soft reset to squash all into one
git reset --soft HEAD~$WIP_COUNT
git commit -m "<clean message>"
```

### Cherry-Pick

When user wants to bring specific commits to current branch:

```bash
# Show commits on source branch
git log --oneline <source-branch> -20

# Cherry-pick selected commits
git cherry-pick <commit-hash>
```

### Finding Lost Work

When user can't find their changes:

```bash
# Check reflog for recent activity
git reflog --all | head -20

# Find dangling commits
git fsck --lost-found

# Search for commits by message
git log --all --oneline --grep="<search-term>"

# Search for commits that changed a file
git log --all --oneline -- <file-path>
```

## Dirty State Decision Tree

```
git status --porcelain → output?
├── Empty → safe to proceed
└── Non-empty → dirty state detected
    ├── Ask user what to do:
    │   ├── "Commit" → chain to /plate → proceed
    │   ├── "Stash" → git stash push -m "context" → proceed
    │   ├── "Discard" → confirm TWICE → git checkout -- . && git clean -fd → proceed
    │   └── "Cancel" → abort the operation
    └── After handling → verify clean: git status --porcelain
```

## PR Body Templates

### Feature PR

```markdown
## Summary
- <1-3 bullet points describing the feature>

## Changes
<list of key files changed and why>

## Testing
- [ ] Unit tests added/updated
- [ ] Manual testing completed
- [ ] Edge cases considered

## Related
- Task: <external task link if exists>
- Design: <design doc link if exists>
```

### Bugfix PR

```markdown
## Problem
<description of the bug>

## Root Cause
<what was causing it>

## Fix
<what was changed and why>

## Testing
- [ ] Regression test added
- [ ] Original issue no longer reproduces
```

## Preference Auto-Detection

### Park Strategy

Detect from git history:
```bash
# If user has WIP commits in history → prefer wip_commit
git log --all --oneline --grep='^wip:' | head -1

# If user has named stashes → prefer stash
git stash list | grep -c 'PARK:\|WIP:'
```

### Sync Strategy

Detect from git history:
```bash
# Count merge commits vs linear history
MERGES=$(git log --oneline --merges -20 | wc -l)
TOTAL=$(git log --oneline -20 | wc -l)

# If >30% are merges → merge strategy
# Otherwise → rebase strategy
```

## Integration with HOANGSA

### Chain Points

| From | To | When |
|------|----|------|
| Flow 1 (Start) | `/serve` pull | User provides external task URL |
| Flow 2 (Switch) | `/plate` | Dirty state → user chooses commit |
| Flow 3 (Park) | — | Self-contained |
| Flow 5 (Finish) | `/plate` → push → PR → `/serve` | Full completion chain |
| Flow 6 (Cleanup) | — | Self-contained |
| Flow 7 (Sync) | — | Self-contained |

### Session Context

When inside a HOANGSA session:
- Task descriptions available from session state
- External task reference from `state.external_task`
- Use session context for branch names and commit messages

When outside a session:
- Infer from user's request and git history
- No automatic chaining to `/serve`
