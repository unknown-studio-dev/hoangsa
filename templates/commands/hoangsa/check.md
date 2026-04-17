---
name: hoangsa:check
description: Status — show session progress with wave structure, budget usage, and artifacts. Use when the user asks "how's it going", "what's the status", "show progress", or wants an overview of the current session.
allowed-tools:
  - Read
  - Bash
---

<objective>
Read the active session's state and display a rich progress overview: session ID, stack, wave-by-wave task progress with budget usage, and a list of available artifacts (specs, plan, memory).
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/check.md`
2. `~/.claude/hoangsa/workflows/check.md`
Read the first path that exists.
</execution_context>

<process>
Follow the check workflow loaded above.
</process>

