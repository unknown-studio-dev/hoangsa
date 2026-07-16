### Dimension 6: Test Quality

Goal: Evaluate test coverage, quality, and gaps.

```
Scan for:

1. COVERAGE GAPS
   - Critical paths without tests (auth, payment, data validation)
   - Public API functions without corresponding test files
   - If test files exist: check if they actually test meaningful scenarios
   - Evidence: untested function/module, its importance, risk
   - How to detect: Use Glob to find source files (`src/**/*.{ts,js,rs,py}`), then Glob for matching test files (`**/*.test.*|**/*.spec.*|**/test_*`). Source files without corresponding test files = coverage gap.

2. TEST SMELLS
   - Tests without assertions (test runs but verifies nothing)
   - Tests that always pass (testing implementation, not behavior)
   - Excessive mocking (tests that don't verify real behavior)
   - Flaky test indicators (timeouts, sleep, race conditions in tests)
   - Tests >100 lines (too complex, testing too many things)
   - Evidence: test file:line, the smell
   - How to detect: Grep test files for `it\(|test\(` blocks, then Read to check if they contain `expect|assert|should`. Grep for `sleep|setTimeout|\.skip` in test files.

3. MISSING TEST TYPES
   - No integration tests (only unit tests)
   - No E2E tests for critical user flows
   - No error case testing (only happy path)
   - No edge case testing (empty input, null, boundary values)
   - Evidence: what type is missing, what it should cover

4. TEST INFRASTRUCTURE
   - Missing CI integration (tests don't run on push/PR)
   - Slow test suite (>5 minutes)
   - No test data management (fixtures, factories)
   - Evidence: CI config gaps, test run timing
```
