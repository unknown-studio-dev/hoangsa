# HOANGSA Update Workflow

You are the update agent. Mission: check for HOANGSA updates, show changelog, obtain user confirmation, and execute clean installation.

**Principles:** Always show what changed before updating. Never update without confirmation. Detect install type (local vs global) automatically.

---

## Step 0: Language enforcement

```bash
# Resolve HOANGSA install path (local preferred over global)
if [ -x "./.claude/hoangsa/bin/hoangsa-cli" ]; then
  HOANGSA_ROOT="./.claude/hoangsa"
else
  HOANGSA_ROOT="$HOME/.claude/hoangsa"
fi

LANG_PREF=$("$HOANGSA_ROOT/bin/hoangsa-cli" pref get . lang 2>/dev/null || echo "en")
```

All user-facing text — version info, changelog display, confirmation prompts, completion messages — **MUST** use the language from `lang` preference (`vi` → Vietnamese, `en` → English). This applies throughout the **ENTIRE** workflow.

---

## Step 1: Detect installed version

Detect whether HOANGSA is installed locally or globally by checking both locations and validating install integrity:

```bash
# Check local first (takes priority only if valid)
LOCAL_VERSION_FILE="" LOCAL_MARKER_FILE="" LOCAL_DIR=""
if [ -f "./.claude/hoangsa/VERSION" ]; then
  LOCAL_VERSION_FILE="./.claude/hoangsa/VERSION"
  LOCAL_MARKER_FILE="./.claude/hoangsa/workflows/update.md"
  LOCAL_DIR="$(cd "./.claude" 2>/dev/null && pwd)"
fi
GLOBAL_VERSION_FILE="" GLOBAL_MARKER_FILE="" GLOBAL_DIR=""
if [ -f "$HOME/.claude/hoangsa/VERSION" ]; then
  GLOBAL_VERSION_FILE="$HOME/.claude/hoangsa/VERSION"
  GLOBAL_MARKER_FILE="$HOME/.claude/hoangsa/workflows/update.md"
  GLOBAL_DIR="$(cd "$HOME/.claude" 2>/dev/null && pwd)"
fi

# Only treat as LOCAL if the resolved paths differ (prevents misdetection when CWD=$HOME)
IS_LOCAL=false
if [ -n "$LOCAL_VERSION_FILE" ] && [ -f "$LOCAL_VERSION_FILE" ] && [ -f "$LOCAL_MARKER_FILE" ] && grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+' "$LOCAL_VERSION_FILE"; then
  if [ -z "$GLOBAL_DIR" ] || [ "$LOCAL_DIR" != "$GLOBAL_DIR" ]; then
    IS_LOCAL=true
  fi
fi

if [ "$IS_LOCAL" = true ]; then
  cat "$LOCAL_VERSION_FILE"
  echo "LOCAL"
elif [ -n "$GLOBAL_VERSION_FILE" ] && [ -f "$GLOBAL_VERSION_FILE" ] && [ -f "$GLOBAL_MARKER_FILE" ] && grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+' "$GLOBAL_VERSION_FILE"; then
  cat "$GLOBAL_VERSION_FILE"
  echo "GLOBAL"
else
  echo "UNKNOWN"
fi
```

Parse output:
- If last line is "LOCAL": local install is valid; installed version is first line; use `--local`
- If last line is "GLOBAL": local missing/invalid, global install is valid; installed version is first line; use `--global`
- If "UNKNOWN": proceed to install step (treat as version 0.0.0)

**If VERSION file missing:**
```
## HOANGSA Update

**Installed version:** Unknown

Your installation doesn't include version tracking.

Running fresh install...
```

Proceed to install step (treat as version 0.0.0 for comparison).

---

## Step 2: Check latest version

Check npm for latest version:

```bash
npm view hoangsa-cc version 2>/dev/null
```

**If npm check fails:**
```
Couldn't check for updates (offline or npm unavailable).

To update manually: `npx hoangsa-cc --global`
```

Exit.

---

## Step 3: Compare versions

Compare installed vs latest:

**If installed == latest:**
```
## HOANGSA Update

**Installed:** X.Y.Z
**Latest:** X.Y.Z

You're already on the latest version.
```

Exit.

**If installed > latest:**
```
## HOANGSA Update

**Installed:** X.Y.Z
**Latest:** A.B.C

You're ahead of the latest release (development version?).
```

Exit.

---

## Step 4: Show changes and confirm

**If update available**, fetch and show what's new BEFORE updating:

1. Fetch changelog from GitHub raw URL
2. Extract entries between installed and latest versions
3. Display preview and ask for confirmation:

```
## HOANGSA Update Available

**Installed:** 1.5.10
**Latest:** 1.5.15

### What's New
────────────────────────────────────────────────────────────

## [1.5.15] - 2026-01-20

### Added
- Feature X

## [1.5.14] - 2026-01-18

### Fixed
- Bug fix Y

────────────────────────────────────────────────────────────

⚠️  **Note:** The installer performs a clean install of HOANGSA folders:
- `commands/hoangsa/` will be wiped and replaced
- `hoangsa/` will be wiped and replaced
- `agents/hoangsa-*` files will be replaced

(Paths are relative to your install location: `~/.claude/` for global, `./.claude/` for local)

Your custom files in other locations are preserved:
- Custom commands not in `commands/hoangsa/` ✓
- Custom agents not prefixed with `hoangsa-` ✓
- Custom hooks ✓
- Your CLAUDE.md files ✓

If you've modified any HOANGSA files directly, they'll be automatically backed up to `hoangsa-local-patches/` before the update.
```

Use AskUserQuestion:
- Question: "Proceed with update?"
- Options:
  - "Yes, update now"
  - "No, cancel"

**If user cancels:** Exit.

---

## Step 5: Run update

Run the update using the install type detected in Step 1:

**If LOCAL install:**
```bash
npx -y hoangsa-cc@latest --local
```

**If GLOBAL install (or unknown):**
```bash
npx -y hoangsa-cc@latest --global
```

Capture output. If install fails, show error and exit.

Clear the update cache so statusline indicator disappears:

```bash
rm -f "./.claude/cache/hoangsa-update-check.json"
rm -f "$HOME/.claude/cache/hoangsa-update-check.json"
```

The SessionStart hook writes to the detected runtime's cache directory, so all paths must be cleared to prevent stale update indicators.

---

## Step 6: Display result

Format completion message (changelog was already shown in confirmation step):

```
╔═══════════════════════════════════════════════════════════╗
║  HOANGSA Updated: v1.5.10 → v1.5.15                     ║
╚═══════════════════════════════════════════════════════════╝

⚠️  Restart Claude Code to pick up the new commands.

[View full changelog](https://github.com/glittercowboy/hoangsa/blob/main/CHANGELOG.md)
```

---

## Step 7: Check local patches

After update completes, check if the installer detected and backed up any locally modified files:

Check for hoangsa-local-patches/backup-meta.json in the config directory.

**If patches found:**

```
⚠️  Local patches were backed up before the update.

Backed up files are in `hoangsa-local-patches/` with details in `backup-meta.json`.
Please review these patches manually and reapply any needed changes to the new version.
```

**If no patches:** Continue normally.

---

## Rules

| Rule | Detail |
|------|--------|
| **Show changelog first** | Never update without showing what changed |
| **Confirm before updating** | Always ask user before executing install |
| **Detect install type** | Auto-detect local vs global, never ask user |
| **Clear cache after update** | Remove update-check cache to reset statusline |
| **Report local patches** | Warn user if modified files were backed up |
