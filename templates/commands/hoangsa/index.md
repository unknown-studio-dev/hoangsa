---
name: hoangsa:index
description: Index the workspace with hoangsa-memory for code intelligence and navigation
allowed-tools:
  - Bash
---

<objective>
Run hoangsa-memory index on the workspace to build/refresh the codebase index.

Routes to the index workflow which handles:
- hoangsa-memory installation check
- Running hoangsa-memory index .
- Reporting indexed symbol count and duration
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/index.md`
2. `~/.claude/hoangsa/workflows/index.md`
Read the first path that exists.
</execution_context>

<process>
**Follow the index workflow** loaded above.

The workflow handles all logic including:
1. Check if hoangsa-memory is installed
2. Install hoangsa-memory if missing
3. Run hoangsa-memory index .
4. Wait for completion
5. Report results
</process>

