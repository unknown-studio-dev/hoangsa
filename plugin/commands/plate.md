---
description: Commit — stage changed files and commit with conventional message. Use when the user says "commit", "save", "plate", or wants to commit completed work from a HOANGSA session.
allowed-tools:
  - Read
  - Bash
  - mcp__hoangsa-memory__memory_detect_changes
  - AskUserQuestion
---

<objective>
Stage changed files and commit them with a conventional commit message derived from completed task descriptions.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show plate
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the plate workflow loaded above.
</process>

