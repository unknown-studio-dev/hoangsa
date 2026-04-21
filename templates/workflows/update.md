# HOANGSA Update Workflow

You are the update agent. Mission: check for HOANGSA updates, show changelog, obtain user confirmation, and execute clean installation.

**Principles:** Always show what changed before updating. Never update without confirmation. Detect install type (local vs global) automatically. Installation is driven by the native `curl | sh` installer — **no Node, no npm, no cargo**.

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

Resolve the latest GitHub release tag via the public API (no auth required for public repos, subject to the anonymous rate limit):

```bash
HOANGSA_REPO="${HOANGSA_REPO:-unknown-studio-dev/hoangsa}"
LATEST_TAG=$(curl -fsSL --retry 3 --retry-delay 2 \
  "https://api.github.com/repos/$HOANGSA_REPO/releases/latest" \
  | grep -E '"tag_name"[[:space:]]*:' \
  | head -n 1 \
  | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')
# Strip leading "v" for comparison with VERSION file contents.
LATEST_VERSION="${LATEST_TAG#v}"
printf '%s\n' "$LATEST_VERSION"
```

**If the check fails** (network down, rate limited, empty response):
```
Couldn't check for updates (offline or GitHub API unavailable).

To update manually:
  curl -fsSL https://github.com/unknown-studio-dev/hoangsa/releases/latest/download/install.sh | sh
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

Re-run the native installer pinned to the resolved tag. The installer forwards mode flags (`--global` / `--local`) straight into `hoangsa-cli install`, so the same script covers both install types:

**If LOCAL install:**
```bash
curl -fsSL "https://github.com/$HOANGSA_REPO/releases/download/$LATEST_TAG/install.sh" \
  | HOANGSA_VERSION="$LATEST_TAG" sh -s -- --local
```

**If GLOBAL install (or unknown):**
```bash
curl -fsSL "https://github.com/$HOANGSA_REPO/releases/download/$LATEST_TAG/install.sh" \
  | HOANGSA_VERSION="$LATEST_TAG" sh -s -- --global
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

[View full changelog](https://github.com/unknown-studio-dev/hoangsa/blob/main/CHANGELOG.md)
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
| **Native installer only** | Update path is always the `curl | sh` installer — never invoke `npm` or `npx` |
