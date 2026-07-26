---
name: hoangsa:check
description: Status — show session progress with wave structure, budget usage, and artifacts. Use when the user asks "how's it going", "what's the status", "show progress", or wants an overview of the current session.
allowed-tools:
  - Read
  - Bash
  - mcp__hoangsa-memory__memory_archive_status
  - mcp__hoangsa-memory__memory_skills_list
---

<objective>
Read the active session's state and display a rich progress overview: session ID, stack, wave-by-wave task progress with budget usage, and a list of available artifacts (specs, plan, memory).
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show check
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the check workflow loaded above.
</process>

