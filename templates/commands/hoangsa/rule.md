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
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/rule.md`
2. `~/.claude/hoangsa/workflows/rule.md`
Read the first path that exists.
</execution_context>

<process>
Follow the rule workflow loaded above.
</process>
