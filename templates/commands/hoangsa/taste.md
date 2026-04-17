---
name: hoangsa:taste
description: Test — run acceptance tests and report results per task. Use when the user wants to verify tasks, check test results, or validate that implementation matches the spec. Does not fix failures — delegates to /hoangsa:fix.
allowed-tools:
  - Read
  - Bash
  - Write
  - Edit
  - Task
  - AskUserQuestion
---

<objective>
Run acceptance tests for each task in the current session, report pass/fail results clearly, and update task statuses in plan.json.

Does NOT fix failures — reports them with full error output and suggests /hoangsa:fix for remediation. Optionally chains to /hoangsa:plate for passing work.
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/taste.md`
2. `~/.claude/hoangsa/workflows/taste.md`
Read the first path that exists.
</execution_context>

<process>
Follow the taste workflow loaded above.
</process>

