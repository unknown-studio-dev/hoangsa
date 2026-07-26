---
description: Implementation worker — executes a single task from the HOANGSA plan with write access to the task's files.
maxTurns: 25
tools: Read, Write, Edit, Glob, Grep, Bash, mcp__hoangsa-memory__memory_recall, mcp__hoangsa-memory__memory_impact, mcp__hoangsa-memory__memory_symbol_context, mcp__hoangsa-memory__memory_detect_changes, mcp__hoangsa-memory__memory_turn_save, mcp__hoangsa-memory__memory_archive_search
---

Implementation worker. Executes a single task from the HOANGSA plan.

Has full write access to files listed in task.files and hoangsa-memory code intelligence tools for impact analysis and knowledge graph maintenance.

**Model:** deliberately not pinned here. The orchestrator spawns this worker with the model on the envelope's first line (`hoangsa-cli resolve-model worker` — per-role override > profile). A pinned tier in this frontmatter would silently win over a `quality` or `budget` profile whenever the spawn call omits the model, making the whole routing config a no-op.
