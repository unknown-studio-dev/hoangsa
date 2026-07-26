---
name: hoangsa:rule
description: Manage HOANGSA rules — add/remove/list rules interactively. Use when the user wants to add a new rule, remove an existing rule, or see the current rule list, or says "rule", "rules", "thêm rule", "xóa rule".
allowed-tools:
  - Read
  - Bash
  - AskUserQuestion
---

<objective>
Manage HOANGSA rules interactively — add new rules via guided wizard, remove existing rules, or list all active rules.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show rule
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the rule workflow loaded above.
</process>
