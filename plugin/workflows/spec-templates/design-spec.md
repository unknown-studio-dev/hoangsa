---
spec_version: "1.0"
project: "<project name>"
component: "<component/module name — snake_case>"
language: "<actual tech stack — or 'N/A' for pure content tasks>"
task_type: "<feat|fix|refactor|perf|test|docs|ci|infra|design|chore>"
category: "<code|ops|content>"
status: "draft"
---

## Overview
[<task_type>]: <Short title>

### Goal
<One sentence end result>

### Context
<Current state, why this is needed>

### Requirements
- [REQ-01] <Specific, verifiable requirement>
- [REQ-02] <...>

### Out of Scope
- <From CONTEXT.md>

---

<!-- CODE CATEGORY: include these sections -->

## Types / Data Models
<Language-appropriate type definitions>

## Interfaces / APIs
<Public function signatures, class methods, REST endpoints>
<Use actual language syntax — not pseudo-code>

<!-- OPS CATEGORY: include these sections -->

## Configuration / Pipeline
<Config files, pipeline stages, env vars, secrets (reference only — never include actual secrets)>

## Steps / Runbook
1. <Step with expected outcome>
   - Rollback: <how to undo this step>
2. <Next step...>

## Dependencies & Prerequisites
<What must exist/be configured before starting>

<!-- CONTENT CATEGORY: include these sections -->

## Structure / Outline
<Sections, pages, or components — with purpose of each>

## Deliverables
| Deliverable | Format | Location | Description |
|------------|--------|----------|-------------|
| ... | .md / .html / .yml | path/to/file | What it contains |

## Style & Guidelines
<Audience, tone, formatting rules, references to follow>

<!-- ALL CATEGORIES: include these sections -->

---

## Implementations

### Design Decisions
| # | Decision | Reasoning | Type |
|---|----------|-----------|------|
| 1 | ... | ... | LOCKED |
| 2 | ... | ... | FLEXIBLE |

### Affected Files

**If hoangsa-memory available:** Use `memory_impact({target: "symbolName", direction: "upstream"})` for each symbol being modified to discover all affected files (direct callers at d=1, indirect at d=2). This prevents missing files that import or call the changed code.

| File | Action | Description | Impact |
|------|--------|-------------|--------|
| `path/to/file` | CREATE / MODIFY / DELETE | What changes | d=1 / d=2 / N/A |

---

## Open Questions
- <Unresolved, if any>

## Constraints
- <Performance, security, compatibility, deadline>

---

## Acceptance Criteria

### Per-Requirement
| Req | Verification | Expected Result |
|-----|-------------|----------------|
| REQ-01 | <command or checklist item> | <expected result> |

### Overall
<Verification sequence appropriate to the task category>
