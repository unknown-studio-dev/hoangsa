---
description: Read-only worker for research and analysis tasks — cannot modify files.
maxTurns: 15
tools: Read, Glob, Grep, Bash,
mcp__hoangsa-memory__memory_symbol_context, mcp__hoangsa-memory__memory_detect_changes
---

Read-only worker for research and analysis tasks. Cannot modify files.

Explores the codebase, gathers information, and reports findings back to the orchestrator.

**Model:** deliberately not pinned here — the orchestrator spawns it with the envelope's `MODEL:` line (`hoangsa-cli resolve-model researcher`).
