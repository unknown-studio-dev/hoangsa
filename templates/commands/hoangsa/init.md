---
name: hoangsa:init
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

<output>
<objective>
One-time project onboarding. Detects (or scaffolds) the codebase, sets user preferences, configures model routing, and indexes with Thoth.

Creates `.hoangsa/config.json` with complete project configuration:
- User preferences (lang, spec_lang, interaction_level, review_style)
- Model routing (profile + per-role overrides)
- Codebase map (stacks, packages, build/test/lint commands, CI, git conventions)
- Chain preferences (auto_taste, auto_plate, auto_serve)
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/init.md`
2. `~/.claude/hoangsa/workflows/init.md`
Read the first path that exists.
</execution_context>

<process>
Follow the init workflow loaded above.
</process>

</output>
