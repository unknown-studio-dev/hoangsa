---
name: quality-checks
frameworks: ["*"]
test_frameworks: []
priority: 60
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
exclude_task_types: ["research", "analysis"]
---

# Quality Checks

## Anti-Fake Tests

A fake test passes CI without verifying real behavior. Every test MUST exercise the actual production code path.

- **MUST NOT duplicate production logic.** A test that copy-paste reimplements the function under test is not a test — it verifies nothing. Call the production function; do not rewrite it inline.
- **MUST NOT use inline stubs in place of test framework utilities.** Use your framework's built-in mock/spy/stub primitives (e.g., `jest.fn()`, `sinon.stub()`, `unittest.mock`). Ad-hoc replacements bypass call tracking and argument assertions.
- **MUST NOT test only hardcoded values.** Assertions against constants that never change (e.g., `expect(2 + 2).toBe(4)`) prove nothing about production code. Assertions must follow a call to the production function with realistic inputs.
- **MUST NOT mock the function under test.** Mocking the very function you are testing means you are testing the mock, not the code. Only mock collaborators and external dependencies.
- **Tests MUST render components, not just import them** (for UI frameworks). Importing a component without mounting/rendering it does not execute its logic. Use your framework's render utility.
- **Tests MUST exercise the actual production code path.** If the test passes identically whether the production function exists or not, the test is fake and must be rewritten.

---

## Edge Case Checklist

Apply these checks to every async, stateful, or resource-owning piece of code you write or modify.

- **Promise leak prevention.** Every subscription, timer, or inflight request started in a component or service MUST be cancelled/unsubscribed/disposed on teardown. Missing cleanup causes memory leaks and flaky tests.
- **Error handling in async code.** Every `Promise` chain and `async` function MUST handle rejection explicitly — either via `.catch()`, `try/catch`, or by propagating to a caller that handles it. Unhandled rejections are bugs.
- **Unmount / cleanup lifecycle.** Hooks, effect handlers, and lifecycle methods that register side effects MUST return or call a cleanup function. Leaving effects alive after unmount causes stale state bugs.
- **Event listener removal.** Every `addEventListener` MUST have a corresponding `removeEventListener` in the cleanup path. Attach listeners with named references so they can be removed precisely.
- **Resource disposal.** File handles, database connections, WebSockets, and similar resources MUST be closed in a `finally` block or equivalent teardown. Do not rely on garbage collection for timely release.
- **Race conditions and stale closures.** In async code, verify that captured variables (state, props, refs) are still valid when the async operation resolves. Use cancellation flags, AbortController, or ref-based patterns to guard against stale results being applied after component unmount or re-render.
