---
name: hoangsa:ship
description: Ship — review code + security, then push or create PR. Use when the user says "ship", "push", "send", "create PR", or wants to ship their work with quality gates.
allowed-tools:
  - Read
  - Bash
  - Write
  - Agent
  - AskUserQuestion
---

<objective>
Review code changes (code quality + security) in parallel, block on critical issues, then let user push or create PR.
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/ship.md`
2. `~/.claude/hoangsa/workflows/ship.md`
Read the first path that exists.
</execution_context>

<process>
Follow the ship workflow loaded above.
</process>

