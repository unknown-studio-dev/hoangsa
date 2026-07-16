---
tests_version: "1.0"
spec_ref: "<component>-spec-v1.0"
component: "<MUST MATCH DESIGN-SPEC.md>"
category: "ops"
strategy: "<smoke|full|dry-run>"
language: "<tools used — e.g. docker, terraform, github-actions>"
---

## Pre-flight Checks
- [ ] <prerequisite is met — e.g., Docker daemon running>
- [ ] <dependency exists — e.g., `.env` file present>

## Smoke Tests

### Check: <descriptive_name>
- **Covers**: [REQ-01]
- **Command**: `<runnable command>`
- **Expected**: <exit code 0, specific output, status>
- **Timeout**: <max wait time>

## Rollback Verification
### Rollback: <scenario>
- **Trigger**: <what goes wrong>
- **Steps**: <rollback commands>
- **Verify**: `<command to confirm rollback worked>`

## Edge Cases
| Scenario | How to simulate | Expected behavior | Covers |
|----------|----------------|-------------------|--------|
| Network failure | `docker network disconnect ...` | Graceful retry | REQ-03 |
