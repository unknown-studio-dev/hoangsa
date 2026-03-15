---
name: hoangsa:index
description: Index the workspace with gitnexus for fast file search and navigation
allowed-tools:
  - Bash
---

<output>
<objective>
Run gitnexus analyze on the workspace to build/refresh the codebase index, then clear the .outdated flag if present.

Routes to the index workflow which handles:
- gitnexus installation check
- Running gitnexus analyze
- Clearing the .gitnexus/.outdated flag
- Reporting indexed file count and duration
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
1. Check if gitnexus is installed
2. Install gitnexus if missing
3. Run gitnexus analyze
4. Wait for completion
5. Delete .gitnexus/.outdated if exists
6. Report results
</process>

</output>
