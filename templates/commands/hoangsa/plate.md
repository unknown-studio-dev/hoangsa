---
name: hoangsa:plate
description: Commit — stage changed files and commit with conventional message. Use when the user says "commit", "save", "plate", or wants to commit completed work from a HOANGSA session.
allowed-tools:
  - Read
  - Bash
  - Write
  - AskUserQuestion
---

<objective>
Stage changed files and commit them with a conventional commit message derived from completed task descriptions.
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/plate.md`
2. `~/.claude/hoangsa/workflows/plate.md`
Read the first path that exists.
</execution_context>

<process>
Follow the plate workflow loaded above.
</process>

