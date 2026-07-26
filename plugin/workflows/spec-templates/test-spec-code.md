---
tests_version: "1.0"
spec_ref: "<component>-spec-v1.0"
component: "<MUST MATCH DESIGN-SPEC.md>"
category: "code"
strategy: "<unit|integration|property|mixed>"
surface: "<ui|api|cli|internal>"
language: "<same as DESIGN-SPEC>"
---

## Unit Tests

### Test: <descriptive_test_name>
- **Covers**: [REQ-01]
- **Input**: <concrete values>
- **Setup**: <mocks/fixtures if needed>
- **Expected**: <exact output>
- **Verify**: `<runnable test command>`

## Integration Tests

### Test: <descriptive_test_name>
- **Covers**: [REQ-02]
- **Setup**: <environment, fixtures>
- **Steps**:
  1. <step>
  2. <step>
- **Expected**: <outcome>
- **Verify**: `<runnable test command>`

## E2E Tests
<!-- REQUIRED when surface: ui|api|cli. Drive the WHOLE flow like a real user/client
     — through the UI, the HTTP endpoint, or the CLI binary — not through internal APIs. -->

### Test: <flow_name>
- **Covers**: [REQ-xx, ...]
- **Entry point**: <URL / screen / CLI invocation>
- **Steps**: <user-observable actions — click, type, call endpoint>
- **Expected**: <observable outcome — visible text, HTTP status + body, exit code>
- **Verify**: `<runnable command — playwright / maestro / curl script / CLI>`

## Edge Cases
<!-- MUST be non-empty (`validate tests` fails an empty table), and MUST be derived from
     the DESIGN-SPEC ## Risk Sweep: every row marked APPLIES there needs ≥1 row here.
     Also cover the error paths in ## Behavior / Logic. A REQ with truly no edge case
     gets a waiver row: | None for REQ-xx | — | — | <reason> |

     Concurrency rows must say how to FORCE the interleaving — "handles concurrency" is
     not a test. Spell out the two actors and the moment they collide, e.g.
     | two writers, same key | 2 concurrent create(k) | exactly one wins, other gets Conflict | REQ-02 |
     | TOCTOU on config file | replace file between stat() and open() | operation aborts, no partial write | REQ-03 |
     | replayed request | POST same idempotency key twice | one row created, both return 200 | REQ-04 |
     | killed mid-write | SIGKILL after step 2 of 3 | no half-written state on restart | REQ-05 | -->
| Case | Input | Expected | Covers |
|------|-------|----------|--------|

## Visual Verification
<!-- REQUIRED when surface: ui (≥1 row). Each state gets verified against the REAL
     running app (fe-testing flow 5) — screenshots are the evidence. Delete for non-UI. -->
| Screen / Component | States to verify | How |
|--------------------|------------------|-----|
| <name> | empty / loading / error / success / disabled / long-text overflow / responsive | run app + screenshot each state |

## Test Data / Fixtures
<Mock data, factories, sample inputs>

## Coverage Target
- Target: ≥ <X>%
- Critical paths: 100%
