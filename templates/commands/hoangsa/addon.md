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
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/addon.md`
2. `~/.claude/hoangsa/workflows/addon.md`
Read the first path that exists.
</execution_context>

<process>
Follow the addon workflow loaded above.
</process>
