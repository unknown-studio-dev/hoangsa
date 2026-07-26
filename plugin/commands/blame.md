---
description: Blame — the user is angry at something you did. Find the pattern behind it, write a lesson whose trigger fires before the next occurrence, and build a skill if it has happened more than once. Use when the user expresses frustration at the agent's behaviour, says "you did it again", "stop doing X", "why did you", or invokes this command directly.
allowed-tools:
  - Read
  - Bash
  - mcp__hoangsa-memory__memory_wakeup
  - mcp__hoangsa-memory__memory_recall
  - mcp__hoangsa-memory__memory_archive_search
  - mcp__hoangsa-memory__memory_remember_lesson
  - mcp__hoangsa-memory__memory_lesson_outcome
  - mcp__hoangsa-memory__memory_skill_propose
---

<objective>
The user invoked this because something you did wasted their time. Find the pattern — not the incident — behind it, record a lesson whose trigger will fire before the next occurrence, correct any existing lesson that failed to fire, and propose a skill when the pattern has recurred. End with artifacts, not with an apology.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show blame
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the blame workflow loaded above.
</process>
