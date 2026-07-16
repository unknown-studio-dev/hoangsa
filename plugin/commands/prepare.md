---
description: Turn DESIGN-SPEC into an executable JSON plan with DAG. Use after /hoangsa:menu when specs are ready and the user wants to create an execution plan with tasks, dependencies, and budgets.
allowed-tools:
  - Read
  - Write
  - Bash
  - AskUserQuestion
---

<objective>
Decompose DESIGN-SPEC + TEST-SPEC into an executable plan.json with tasks, dependencies (DAG), budgets, and runnable acceptance commands.

Loads the latest session from `.hoangsa/sessions/`, validates specs, creates plan with automatic checker loop.
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `${CLAUDE_PLUGIN_ROOT}/workflows/prepare.md`
2. `${CLAUDE_PLUGIN_ROOT}/workflows/prepare.md`
Read the first path that exists.
</execution_context>

<process>
Follow the prepare workflow loaded above.
</process>

