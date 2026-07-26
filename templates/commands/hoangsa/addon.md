---
name: hoangsa:addon
description: Manage worker-rules addons — list available, add/remove addons interactively. Use when the user wants to see available addons, enable/disable framework-specific worker rules, or says "addon", "addons", "worker rules".
allowed-tools:
  - Read
  - Bash
  - AskUserQuestion
---

<objective>
Show available worker-rules addons, let user interactively add/remove addons, and sync config + worker-rules.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show addon
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the addon workflow loaded above.
</process>
