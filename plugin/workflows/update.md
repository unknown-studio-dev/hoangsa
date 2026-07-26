# HOANGSA Update Workflow

> **Boot:** Read `$HOANGSA_ROOT/workflows/common.md` first — universal rules + CLI reference + self-verification template.

You are the update agent. Mission: check for HOANGSA updates, show changelog, obtain user confirmation, and execute clean installation.

**Principles:** Always show what changed before updating. Never update without confirmation. Version detection and the upgrade itself are `hoangsa-cli update` — this workflow decides *whether* to run it and shows the user *why*. Installation is driven by the native `curl | sh` installer — **no Node, no npm, no cargo**.

---

## Step 1: Check for an update

One call. `hoangsa-cli update --check` reads the installed version from
`<install dir>/manifest.json` — the file the installer actually writes — and
resolves the latest release tag:

```bash
"$HOANGSA_BIN" update --check
```

It prints JSON and exits **10** when an update is available, 0 when up to date,
1 on failure:

```json
{ "status": "ok", "current": "0.6.0", "latest": "v0.7.0",
  "update_available": true,
  "command": "curl -fsSL .../install.sh | sh -s -- --global" }
```

Add `--local` to check a project install instead of the global one.

**Do not reimplement this check in shell.** The version used to be read from
`<config>/hoangsa/VERSION`, a file nothing has ever written, against a
hardcoded `~/.claude` that is wrong for anyone using `CLAUDE_CONFIG_DIR`. The
result was a checker that reported "not installed" everywhere, in a shape
indistinguishable from a real missing install.

**If `status` is `error`:** show `error` and the `hint` field, then exit.

**If `update_available` is false:** tell the user they are on `current` and
exit. `current` ahead of `latest` means a development build — say so, do not
offer a downgrade.

---

## Step 2: Show changes and confirm

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

(Paths are relative to your install location: your Claude config dir for global — `$CLAUDE_CONFIG_DIR` when set, otherwise `~/.claude/` — or `./.claude/` for local)

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

## Step 3: Run update

```bash
"$HOANGSA_BIN" update --yes          # add --local for a project install
```

The subcommand runs the release installer for the resolved tag, then clears the
update-check cache in every config dir it knows about, so the statusline badge
disappears without a second command. If it exits non-zero, show the `error`
field and stop.

---

## Step 4: Display result

Format completion message (changelog was already shown in confirmation step):

```
╔═══════════════════════════════════════════════════════════╗
║  HOANGSA Updated: v1.5.10 → v1.5.15                     ║
╚═══════════════════════════════════════════════════════════╝

⚠️  Restart Claude Code to pick up the new commands.

[View full changelog](https://github.com/unknown-studio-dev/hoangsa/blob/main/CHANGELOG.md)
```

---

## Step 5: Check local patches

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

Universal rules live in `common.md §Universal rules`. Update-specific additions:

| Rule | Detail |
|------|--------|
| **Show changelog first** | Never update without showing what changed |
| **Confirm before updating** | Always ask user before executing install |
| **Detect install type** | Auto-detect local vs global, never ask user |
| **Never hand-roll the check** | Version detection and the upgrade both belong to `hoangsa-cli update`; the cache clear happens there too |
| **Report local patches** | Warn user if modified files were backed up |
| **Native installer only** | Update path is always the `curl | sh` installer — never invoke `npm` or `npx` |
