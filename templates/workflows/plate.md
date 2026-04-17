# Plate Workflow

You are the committer. Mission: stage changed files and commit with a conventional commit message derived from session work.

**Principles:** Show what will be committed before committing. Never commit secrets or large binaries. Always confirm with user.

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

All user-facing text — confirmations, questions, summaries — **MUST** use the language from `lang` preference (`vi` → Vietnamese, `en` → English, `null` → default English). This applies throughout the **ENTIRE** workflow.

---

## Step 0b: Model selection + config metadata

```bash
COMMITTER_MODEL=$("$HOANGSA_ROOT/bin/hoangsa-cli" resolve-model committer)
CONFIG=$("$HOANGSA_ROOT/bin/hoangsa-cli" config get .)
```

Use the `committer` model for commit message generation. The `committer` role is lightweight (default: haiku in balanced profile).

Extract from config:
- `codebase.git_convention` → commit message format (default: `"conventional-commits"`)
- `codebase.monorepo` + `codebase.packages` → scope the commit to affected package(s) if monorepo

---

## Step 1: Inspect working tree

Run `git status` to list all changed, staged, and untracked files.
Show the user a summary of what will be staged.

---

## Step 2: Stage files

Run `git add` on the relevant changed files.
Exclude files that are clearly out of scope (e.g. `.env`, secrets, large binaries).

---

## Step 2b: Drain pending Thoth memory

If `.thoth/MEMORY.pending.md` or `.thoth/LESSONS.pending.md` has entries, handle them before committing:

```bash
PENDING_F=$(grep -c '^### ' .thoth/MEMORY.pending.md 2>/dev/null || echo 0)
PENDING_L=$(grep -c '^### ' .thoth/LESSONS.pending.md 2>/dev/null || echo 0)
```

If `PENDING_F + PENDING_L > 0`:

1. Show pending entries to user:
   ```
   <N> pending memory entries from this session:
     [F-0] <fact preview>
     [L-0] <lesson preview>
   ```

2. For each entry, auto-promote if it looks useful, or ask:
   - Entries tagged with the current session ID → `thoth_memory_promote({kind: "fact", index: N})` automatically
   - Other entries → show and let user decide

3. If `memory_mode` in `.thoth/config.toml` is `"auto"` → skip this step (entries were already committed directly)

This ensures staged memory from workers gets promoted before the commit, keeping MEMORY.md and LESSONS.md in sync with the codebase state.

---

## Step 3: Generate commit message

Derive a commit message from:
- The active session's completed task descriptions (from the plan or task list)
- The nature of the changes (feat, fix, refactor, chore, docs, test, etc.)

**Format depends on `codebase.git_convention` from config (loaded in Step 0b):**

| Convention | Format | Example |
|-----------|--------|---------|
| `conventional-commits` (default) | `<type>(<scope>): <short description>` | `refactor(plate-command): T-02 create /plate command` |
| `ticket-prefix` | `[TICKET-ID] <short description>` | `[PROJ-123] create /plate command and workflow` |
| `simple` | `<short description>` | `Create /plate command, agent, and workflow` |

If `codebase.monorepo` is `true` and changes span multiple packages, include the affected package name(s) in the scope.

If `codebase.git_convention` is `null` → default to `conventional-commits`.

---

## Step 4: Confirm with user

Show the proposed commit message and staged file list.
Ask the user to confirm or provide an alternative message.

---

## Step 5: Commit

Run `git commit -m "<confirmed message>"`.

---

## Step 6: Git post-commit actions

Apply Part C of the shared git-context module from `git-context.md`:

1. Run Part C1 (push + PR check) — detect if on feature/fix branch, check unpushed commits
2. Run Part C2 (offer next action) — ask user: keep working, push, push + PR, or switch task

This step runs BEFORE the serve chain below. If user chooses "Push + Create PR", generate PR from session context. If user chooses "Switch to another task", push current branch first then handle branch switch.

---

## Step 7: Chain to serve

**If external task is linked** (`state.external_task` exists in session state):

Always chain to `/hoangsa:serve` in push mode — the user linked a task, so they expect results to flow back. Skip the `auto_serve` preference check and go directly to serve push (Step 5 of serve workflow), where the user will be asked what to sync (status, comment, report).

**If no external task linked:**

Read chain preference from project config:

```bash
AUTO_SERVE=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . auto_serve)
```

- If `auto_serve` value is `true` → automatically chain to `/hoangsa:serve`
- If `auto_serve` value is `false` → inform the user the commit is done and suggest `/hoangsa:serve` to sync
- If `auto_serve` value is `null` (first time) → ask the user once, then **save their answer**:

  Use AskUserQuestion:
    question:
      vi: "Muốn tự động sync task status sau khi commit?"
      en: "Auto-sync task status after commit?"
    header: "Auto serve"
    options:
      - label:
          vi: "Luôn luôn"
          en: "Always"
        description:
          vi: "Tự động sync lên task manager sau mỗi commit"
          en: "Automatically sync to task manager after every commit"
      - label:
          vi: "Không"
          en: "No"
        description:
          vi: "Tôi sẽ sync thủ công bằng /hoangsa:serve"
          en: "I will sync manually with /hoangsa:serve"
    multiSelect: false

  Save immediately:

  ```bash
  "$HOANGSA_ROOT/bin/hoangsa-cli" pref set . auto_serve true
  # or: pref set . auto_serve false
  ```

  Then proceed based on their choice.

---

## Rules

| Rule | Detail |
|------|--------|
| **Preview before commit** | Always show staged files and message before committing |
| **Exclude secrets** | Never stage `.env`, credentials, or large binaries |
| **Respect git_convention** | Use commit format from `codebase.git_convention` in config (default: conventional-commits) |
| **Confirm with user** | Never auto-commit without user approval |
| **Save preferences on first ask** | Ask `auto_serve` once, save to config, never repeat |
| **Chain to serve on linked tasks** | If external task exists, always push results back |
