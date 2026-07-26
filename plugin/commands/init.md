---
description: Initialize HOANGSA for a project — detect codebase, setup preferences, model routing. Run once when starting a new project. Use when the user says "init", "setup project", "first time", or when .hoangsa/config.json doesn't exist yet.
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
One-time project onboarding. Detects (or scaffolds) the codebase, sets user preferences, configures model routing, and indexes with hoangsa-memory.

Creates `.hoangsa/config.json` with complete project configuration:
- User preferences (lang, spec_lang, interaction_level, review_style)
- Model routing (profile + per-role overrides)
- Codebase map (stacks, packages, build/test/lint commands, CI, git conventions)
- Chain preferences (auto_taste, auto_plate, auto_serve)
</objective>

<execution_context>
Load the workflow:

```bash
hoangsa-cli workflow show init
```

Its stdout IS the workflow — follow it. The command searches the
project-local install, `$CLAUDE_CONFIG_DIR`, then the default profile,
and lists every path it tried if nothing matched.
</execution_context>

<process>
Follow the init workflow loaded above.
</process>

