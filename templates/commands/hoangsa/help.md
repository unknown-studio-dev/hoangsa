---
name: hoangsa:help
description: Show HOANGSA commands and workflow
allowed-tools: []
---

<objective>
Display HOANGSA help — the 3-phase workflow and available commands.
</objective>

<process>
Display this help:

```
HOANGSA — 3-Phase Development System

Setup:     /hoangsa:init
Main flow: /hoangsa:brainstorm → /hoangsa:menu → /hoangsa:prepare → /hoangsa:cook

Commands:
  /hoangsa:init      First-time project setup — detect codebase, preferences, model routing
  /hoangsa:brainstorm Brainstorm — explore idea → propose approaches → BRAINSTORM.md
  /hoangsa:menu      Design a task — idea → DESIGN-SPEC + TEST-SPEC
  /hoangsa:prepare   Turn spec into executable JSON plan with DAG
  /hoangsa:cook      Execute plan wave-by-wave, verify results
  /hoangsa:taste     Run acceptance tests, report results
  /hoangsa:fix       Hotfix — analyze bug → fix → auto-test
  /hoangsa:plate     Commit completed work with conventional message
  /hoangsa:check     Show session progress overview
  /hoangsa:research  Deep research — codebase + external → RESEARCH.md
  /hoangsa:audit     Audit codebase — code smells, security, tech debt → AUDIT-REPORT.md
  /hoangsa:serve     Sync task status to external task manager
  /hoangsa:index     Re-index codebase with hoangsa-memory
  /hoangsa:help      Show this help
  /hoangsa:update    Update HOANGSA to latest version

How it works:
  0. INIT       — One-time setup: detect stack, set preferences, choose model profile
  1. BRAINSTORM — (optional) Explore idea, propose approaches, validate design
  2. MENU       — Discuss requirements, research codebase, create specs
  3. PREPARE    — Decompose into tasks with dependencies and acceptance criteria
  4. COOK       — Execute tasks in fresh contexts, verify with 3-tier checks

Key concepts:
  • Context engineering — each task gets fresh 200k context (no context rot)
  • Wave execution — DAG-aware parallel execution
  • Model routing — 8 roles × 3 profiles, per-role overrides
  • Atomic commits — each task gets its own commit
  • 3-tier verification — static + behavioral ×3 + semantic review
  • Escalation ladder — retry → enrich → model escalate → human
  • Persistent preferences — ask once, save to config, never repeat

Config lives in .hoangsa/config.json
Session data lives in .hoangsa/sessions/<type>/
```
</process>

