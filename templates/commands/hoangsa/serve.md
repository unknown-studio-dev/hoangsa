---
name: hoangsa:serve
description: Sync task status to external task managers (ClickUp/Asana/Linear/Jira/GitHub) via MCP. Bidirectional — pull task details from a link, or push status/comments/reports back after work is done.
allowed-tools:
  - Read
  - Write
  - Bash
  - Task
  - AskUserQuestion
  - WebFetch
---

<objective>
Bidirectional sync between HOANGSA sessions and external task managers via MCP.

**Pull (intake):** User pastes a task URL → fetch title, description, acceptance criteria, comments → save as session context for /menu.

**Push (sync-back):** After work is done → ask user what to update on the task (status change, add comment with work summary, full report with files/tests/commits).

On first run: auto-discovers MCP servers, prompts user to select a task manager, verifies connection, saves config.
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/serve.md`
2. `~/.claude/hoangsa/workflows/serve.md`
Read the first path that exists.
</execution_context>

<process>
Follow the serve workflow loaded above.
</process>

