# Worker Rules

Rules that every HOANGSA worker subagent MUST follow when implementing a task.
These rules are non-negotiable unless explicitly overridden by project config.

> **Customization:** Copy this file to `.hoangsa/worker-rules.md` in your project
> and modify as needed. The project-level file takes priority over this default.

---

## 1. Scope Control

- **Only modify files listed in `task.files`.** If you discover a file that also needs changes, report it — do NOT modify it yourself.
- **No refactoring outside scope.** Do not "improve" surrounding code, rename variables in untouched functions, or clean up imports you didn't add.
- **No new dependencies** unless the DESIGN-SPEC explicitly requires them. If you believe a dependency is needed, report it as a blocker.
- **Do not delete or modify existing tests** unless the task explicitly covers test changes. Adding new tests is fine; breaking existing ones is not.
- **No feature creep.** Implement exactly what the task describes. No "while I'm here" additions.

---

## 2. Code Quality

- **Match the project's existing style.** Indentation, naming conventions (camelCase vs snake_case), quote style, bracket placement — follow what's already there.
- **Do not add comments, docstrings, or type annotations** to code you did not write or change. Only add comments where the logic is not self-evident in code you authored.
- **No over-engineering.** No premature abstractions, no helper utilities for one-time operations, no design-for-the-future patterns. Three similar lines > a premature abstraction.
- **No unnecessary error handling.** Do not add validation, fallbacks, or try/catch for scenarios that cannot happen according to the spec. Trust internal code and framework guarantees. Only validate at system boundaries (user input, external APIs).
- **No backward-compatibility shims.** No renaming unused `_vars`, no re-exporting removed types, no `// removed` comments. If something is removed, remove it completely.

---

## 3. Security

- **Never hardcode secrets, API keys, tokens, or credentials.** Use environment variables or config files.
- **Do not introduce OWASP Top 10 vulnerabilities:** no SQL injection, XSS, command injection, path traversal, or insecure deserialization.
- **Sanitize at system boundaries.** Validate and sanitize user input, external API responses, and file paths at entry points.
- **If you notice existing insecure code** in files you're modifying, fix it only if it's within your task scope. Otherwise, report it.

---

## 4. Git Discipline

- **Atomic commit after acceptance passes.** One commit per task, containing only files relevant to that task.
- **Commit message format:** `<type>(<session_id>): <task.name>`
- **Do not commit:** `.env` files, credentials, large binaries, IDE config, OS-generated files, or files not in `task.files`.
- **Do not amend, rebase, or force-push** existing commits.

---

## 5. Acceptance

- **Read all `context_pointers` before writing any code.** Understand the existing code first.
- **Run the acceptance command** before committing. Do not commit if acceptance fails.
- **Max 3 retry attempts** if acceptance fails:
  1. Attempt 1 — fix based on error output
  2. Attempt 2 — re-read context, look for missed patterns
  3. Attempt 3 — try alternative approach
- **If all 3 attempts fail:** stop, report the failure with full error details (command, stdout, stderr). Do NOT keep retrying.

---

## 6. Context Hygiene

- **Read only what you need.** Start with `context_pointers`, then `task.files`. Do not explore the entire codebase.
- **Do not read files unrelated to the task.** Every file read consumes context window — keep it focused.
- **If you need information not in your context:** report it as a blocker rather than guessing.

---

## 7. GitNexus — Code Intelligence

If the orchestrator tells you GitNexus is available (`GITNEXUS_AVAILABLE`), use it to understand code before modifying it. GitNexus provides a pre-indexed knowledge graph of the codebase — it's faster and more accurate than grepping.

### Before editing a symbol (function, class, method):

```
gitnexus_impact({target: "symbolName", direction: "upstream"})
```

Check the blast radius. If risk is HIGH or CRITICAL, report it to the orchestrator before proceeding — do not silently push through.

### When you need to understand a symbol's callers/callees:

```
gitnexus_context({name: "symbolName"})
```

This gives you the full picture — who calls it, what it calls, which execution flows it participates in. Use this instead of grepping for function names.

### When tracing a bug or finding related code:

```
gitnexus_query({query: "description of what you're looking for"})
```

Returns execution flows ranked by relevance. Better than `Grep` for understanding how pieces connect.

### Rules:

- **Impact before edit.** Run `gitnexus_impact` on every symbol you're about to modify. This is not optional — it prevents breaking callers you didn't know about.
- **HIGH/CRITICAL = report.** If impact analysis returns HIGH or CRITICAL risk, report it to the orchestrator with the affected symbols. Do not proceed without acknowledgment.
- **Fallback gracefully.** If a GitNexus tool call fails (timeout, error), fall back to Grep/Glob. Do not block on it.
- **GitNexus unavailable = skip.** If the orchestrator does not pass `GITNEXUS_AVAILABLE`, use Grep/Glob as usual. Do not attempt GitNexus calls.

---

## 7. Communication

- **Report, don't guess.** If something is ambiguous, unclear, or missing from the spec — report it as a blocker. Do not make assumptions about intended behavior.
- **On failure, provide evidence:** the exact command run, full stdout/stderr, and what you tried. Do not summarize or truncate error output.
- **Do not apologize or explain your reasoning at length.** State what you did, what passed, what failed. Be terse.
- **Respect user's language preference.** If the orchestrator specifies a `lang` preference (e.g., `vi` for Vietnamese, `en` for English), all status messages, error reports, and commit messages descriptions must use that language. Commit message prefixes (`feat`, `fix`, `refactor`) stay in English per conventional-commits spec.

---

## 8. Anti-Fake Tests

A fake test passes CI without verifying real behavior. Every test MUST exercise the actual production code path.

- **MUST NOT duplicate production logic.** A test that copy-paste reimplements the function under test is not a test — it verifies nothing. Call the production function; do not rewrite it inline.
- **MUST NOT use inline stubs in place of test framework utilities.** Use your framework's built-in mock/spy/stub primitives (e.g., `jest.fn()`, `sinon.stub()`, `unittest.mock`). Ad-hoc replacements bypass call tracking and argument assertions.
- **MUST NOT test only hardcoded values.** Assertions against constants that never change (e.g., `expect(2 + 2).toBe(4)`) prove nothing about production code. Assertions must follow a call to the production function with realistic inputs.
- **MUST NOT mock the function under test.** Mocking the very function you are testing means you are testing the mock, not the code. Only mock collaborators and external dependencies.
- **Tests MUST render components, not just import them** (for UI frameworks). Importing a component without mounting/rendering it does not execute its logic. Use your framework's render utility.
- **Tests MUST exercise the actual production code path.** If the test passes identically whether the production function exists or not, the test is fake and must be rewritten.

---

## 9. Edge Case Checklist

Apply these checks to every async, stateful, or resource-owning piece of code you write or modify.

- **Promise leak prevention.** Every subscription, timer, or inflight request started in a component or service MUST be cancelled/unsubscribed/disposed on teardown. Missing cleanup causes memory leaks and flaky tests.
- **Error handling in async code.** Every `Promise` chain and `async` function MUST handle rejection explicitly — either via `.catch()`, `try/catch`, or by propagating to a caller that handles it. Unhandled rejections are bugs.
- **Unmount / cleanup lifecycle.** Hooks, effect handlers, and lifecycle methods that register side effects MUST return or call a cleanup function. Leaving effects alive after unmount causes stale state bugs.
- **Event listener removal.** Every `addEventListener` MUST have a corresponding `removeEventListener` in the cleanup path. Attach listeners with named references so they can be removed precisely.
- **Resource disposal.** File handles, database connections, WebSockets, and similar resources MUST be closed in a `finally` block or equivalent teardown. Do not rely on garbage collection for timely release.
- **Race conditions and stale closures.** In async code, verify that captured variables (state, props, refs) are still valid when the async operation resolves. Use cancellation flags, AbortController, or ref-based patterns to guard against stale results being applied after component unmount or re-render.
