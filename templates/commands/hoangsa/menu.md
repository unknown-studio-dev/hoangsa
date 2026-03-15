---
name: hoangsa:menu
description: Design a task — from idea to DESIGN-SPEC + TEST-SPEC. Use when the user has a new feature idea, wants to plan something, says "let's build", "I want to add", or needs to design before coding. Works for any task type — features, bug fixes, CI/CD, Docker, docs, landing pages, infra, config changes, and more. Adapts spec format to match the task.
allowed-tools:
  - Read
  - Bash
  - Write
  - Task
  - AskUserQuestion
---

<output>
<objective>
Lead the user from a vague idea to a complete DESIGN-SPEC.md + TEST-SPEC.md, ready for planning.

Adapts to any task type — code features, bug fixes, CI/CD pipelines, Docker setup, documentation, landing pages, infra config, and more. The spec format changes to match what the task actually needs.

Creates a session in `.hoangsa/sessions/<timestamp>/` with:
- CONTEXT.md — decisions, scope, out-of-scope
- RESEARCH.md — codebase analysis
- DESIGN-SPEC.md — adaptive format: types/interfaces (code), config/runbook (ops), outline/deliverables (content)
- TEST-SPEC.md — adaptive format: test cases (code), smoke tests (ops), checklist (content)
</objective>

<execution_context>
Resolve HOANGSA install path — check local first, then global:
1. `./.claude/hoangsa/workflows/menu.md`
2. `~/.claude/hoangsa/workflows/menu.md`
Read the first path that exists.
</execution_context>

<process>
Follow the menu workflow loaded above.
</process>

</output>
