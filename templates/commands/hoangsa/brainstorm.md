---
name: hoangsa:brainstorm
description: Brainstorm an idea — explore intent, propose approaches, validate design → BRAINSTORM.md. Use BEFORE /hoangsa:menu when you have a vague idea and want to explore it collaboratively before committing to a spec. Output feeds directly into the menu workflow.
allowed-tools:
  - Read
  - Bash
  - Write
  - Glob
  - Grep
  - Task
  - AskUserQuestion
---

<objective>
Turn a vague idea into a validated design through collaborative dialogue.

Produces a BRAINSTORM.md that includes:
- Chosen approach with rationale and alternatives considered
- Architecture, components, data flow, interfaces
- Decisions (LOCKED/FLEXIBLE) and open questions
- Out of scope

This output feeds directly into /hoangsa:menu — menu auto-detects BRAINSTORM.md and uses it as context for DESIGN-SPEC + TEST-SPEC creation.
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show brainstorm
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the brainstorm workflow loaded above.
</process>
