---
name: hoangsa:ship
description: Ship — review code + security, then push or create PR. Use when the user says "ship", "push", "send", "create PR", or wants to ship their work with quality gates.
allowed-tools:
  - Read
  - Bash
  - Task
  - AskUserQuestion
---

<objective>
Review code changes (code quality + security) in parallel, block on critical issues, then let user push or create PR.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show ship
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the ship workflow loaded above.
</process>

