---
name: hoangsa:update
description: Update HOANGSA to latest version with changelog display
allowed-tools:
  - Bash
  - AskUserQuestion
---

<objective>
Check for HOANGSA updates, install if available, and display what changed.

Routes to the update workflow which handles:
- Version check via `hoangsa-cli update --check` (reads the install manifest,
  resolves the latest release tag)
- Changelog fetching and display
- User confirmation with clean install warning
- Update execution via `hoangsa-cli update`, which clears the update cache
- Restart reminder
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show update
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
**Follow the update workflow** loaded above.

The workflow handles all logic including:
1. Installed version detection (local/global)
2. Latest version checking via npm
3. Version comparison
4. Changelog fetching and extraction
5. Clean install warning display
6. User confirmation
7. Update execution
8. Cache clearing
</process>

