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
<!-- MUST be non-empty (`validate tests` fails an empty table). Pull every boundary,
     error path, and weird input from the Step 3d deep-dive. A REQ with truly no
     edge case gets a waiver row: | None for REQ-xx | — | — | <reason> | -->
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
