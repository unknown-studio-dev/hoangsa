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

## Behavior / Logic
<!-- REQUIRED for category: code (`validate spec` fails without it). Signatures without
     logic mean the worker invents the logic — this section is what it implements.
     One block per REQ with non-trivial behavior. A REQ that is genuinely pure data
     (type alias, constant table) gets one waiver line: `[REQ-xx] N/A — <reason>`. -->

### [REQ-01] <what this does>
**Trigger:** <the call / event / route / CLI invocation that starts it>
**Preconditions:** <what must already be true; who is allowed to do it>
**Steps:**
1. <deterministic step — validate / read / compute / write, with the exact condition and the exact source of truth>
2. <...>
**Postconditions:** <what is true after; what must NEVER happen>

**Decision table** (include when branching is non-trivial — one row per reachable branch):
| Condition | Result |
|-----------|--------|
| <condition> | <what happens> |

**Error paths** (include when anything can fail):
| Failure | Detected how | Behavior (return / raise / retry / rollback) |
|---------|--------------|----------------------------------------------|
| <failure> | <check> | <what the caller observes> |

**State transitions** (include when the component holds state):
`<state A> --<event>--> <state B>`

## Risk Sweep
<!-- REQUIRED for category: code. `validate spec` fails if any class row is missing.
     Every row: APPLIES (→ concrete handling + the TEST-SPEC Edge Cases row that proves it)
     or N/A with a real reason. "N/A" with no reason is a defect, not a shortcut.
     Answer from research/code where you can; ask the user where you cannot. -->

| Risk class | Applies? | Handling (or why N/A) | Edge case ref |
|------------|----------|-----------------------|---------------|
| Boundary / empty input | APPLIES / N/A | <zero, one, max, empty collection, null> | <Edge Cases row> |
| Invalid / malformed input | APPLIES / N/A | <wrong type, unicode, injection, truncated payload> | ... |
| Concurrency & TOCTOU | APPLIES / N/A | <every check-then-act on shared state: file, DB row, cache, env, global. Name the race window and the guard — lock, transaction, atomic rename, CAS, unique index, single-writer> | ... |
| Idempotency & retry | APPLIES / N/A | <same call twice / replayed request — dedupe key, upsert, safe at-least-once?> | ... |
| Partial failure & rollback | APPLIES / N/A | <multi-step write that dies halfway — what state is left behind, who cleans it> | ... |
| Auth & permission | APPLIES / N/A | <who may call it, what happens when they may not> | ... |
| Limits (size / timeout / rate) | APPLIES / N/A | <max payload, slow dependency, unbounded loop or growth> | ... |
| Backward compat / migration | APPLIES / N/A | <old data, old callers, old config still in the wild> | ... |

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
<!-- Every row MUST have been surfaced to the user before the spec is approved
     (menu Step 6c). Status is RESOLVED (answer recorded below) or DEFERRED (the user
     explicitly chose to decide later). A row with neither = a question that was never
     asked, and `validate spec` fails on it. Assumptions count as open questions:
     if you filled a gap yourself, it belongs here with what breaks if you guessed wrong.
     Nothing unresolved → one row: | None | RESOLVED | — | — | -->

| Question | Status | Answer / Decision | Impact if wrong |
|----------|--------|-------------------|-----------------|
| <question> | RESOLVED / DEFERRED | <user's answer, or what we assume until they decide> | <what breaks> |

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
