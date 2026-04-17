---
name: hoangsa:research
description: Research a topic — codebase analysis + external research → RESEARCH.md. Use when the user wants to understand a codebase area, investigate a topic, find best practices, or needs context before designing a feature.
allowed-tools:
  - Read
  - Bash
  - Write
  - Glob
  - Grep
  - Task
  - AskUserQuestion
  - WebSearch
  - WebFetch
---

<objective>
Research a topic deeply by combining codebase analysis with external research, producing a RESEARCH.md artifact.

Creates a RESEARCH.md that includes:
- Codebase analysis — relevant symbols, patterns, and execution flows
- External research — documentation, best practices, prior art
- Summary of findings and recommendations
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/research.md`
2. `~/.claude/hoangsa/workflows/research.md`
Read the first path that exists.
</execution_context>

<process>
Follow the research workflow loaded above.
</process>

