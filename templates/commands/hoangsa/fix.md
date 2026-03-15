---
name: hoangsa:fix
description: Hotfix — analyze bug → cross-layer root cause tracing → minimal fix plan → implement → auto-chain to taste. Traces bugs across FE/BE/API boundaries to find the real root cause. Use when the user reports a bug, error, failing test, or wants a quick targeted fix without the full menu→prepare→cook flow.
allowed-tools:
  - Read
  - Bash
  - Write
  - Edit
  - Task
  - AskUserQuestion
---

<output>
<objective>
Analyze a bug with cross-layer root cause tracing (FE↔BE↔API↔DB), create a minimal fix plan (1–3 tasks, each <10k tokens), implement the fixes, run /simplify on changed files, then auto-chain to /hoangsa:taste.

Spawns a research agent to trace bugs across layer boundaries — a frontend bug may originate from a backend API, and vice versa.

If a task manager link is provided, syncs fix results back after completion.

Faster than the full menu → prepare → cook flow — designed for hotfixes and targeted bug repairs.
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/fix.md`
2. `~/.claude/hoangsa/workflows/fix.md`
Read the first path that exists.
</execution_context>

<process>
Follow the fix workflow loaded above.
</process>

</output>
