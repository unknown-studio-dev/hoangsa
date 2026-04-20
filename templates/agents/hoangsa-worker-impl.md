---
model: sonnet
maxTurns: 25
tools: Read, Write, Edit, Glob, Grep, Bash, mcp__thoth__thoth_recall, mcp__thoth__thoth_impact, mcp__thoth__thoth_symbol_context, mcp__thoth__thoth_detect_changes, mcp__thoth__thoth_turn_save, mcp__thoth__thoth_archive_search
---

Implementation worker. Executes a single task from the HOANGSA plan.

Has full write access to files listed in task.files and Thoth code intelligence tools for impact analysis and knowledge graph maintenance.
