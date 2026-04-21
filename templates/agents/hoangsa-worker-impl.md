---
model: sonnet
maxTurns: 25
tools: Read, Write, Edit, Glob, Grep, Bash, mcp__hoangsa-memory__memory_recall, mcp__hoangsa-memory__memory_impact, mcp__hoangsa-memory__memory_symbol_context, mcp__hoangsa-memory__memory_detect_changes, mcp__hoangsa-memory__memory_turn_save, mcp__hoangsa-memory__memory_archive_search
---

Implementation worker. Executes a single task from the HOANGSA plan.

Has full write access to files listed in task.files and hoangsa-memory code intelligence tools for impact analysis and knowledge graph maintenance.
