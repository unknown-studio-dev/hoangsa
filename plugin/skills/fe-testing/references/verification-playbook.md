# Verification playbook

Reference for the `fe-testing` skill. The methodology that makes a frontend feature
*verifiable* rather than "looks done". Stack-agnostic; pairs with `web-testing.md` and
`mobile-testing.md`.

## Why features feel unverifiable

The usual failure isn't lack of tests — it's tests that don't correspond to the feature.
Symptoms: "tests pass but it's broken", "I don't know what to assert", "I tested it but a bug
still shipped". The fix is upstream of the test: **define what correct means, observably,
before you code.**

## Step 1 — Criteria template (fill before coding)

```
Feature: <one sentence — what the user can now do>

Happy path:
- Given <starting state>, When <user action>, Then <observable outcome>.

Edge / error paths:
- Given <error condition>, When <action>, Then <observable outcome>.
- Given <empty/invalid input>, When <action>, Then <observable outcome + no side effect>.

Boundaries:
- <loading state?  disabled controls?  optimistic vs confirmed?  empty list?>

Out of scope:
- <what this feature explicitly does NOT do — so you don't over-test>
```

Every "Then" must name something **a user sees or the API records**: visible text, a disabled
control, a navigation, a fired request, a persisted value. If a "Then" reads like
`state.isSaving === true`, rewrite it as what that state *produces* on screen. If you can't,
you don't understand the feature yet — ask the user.

## Step 2 — Map each criterion to a layer

| The "Then" is about… | Layer | Why |
|---|---|---|
| A computed/formatted value, a hook, a reducer | Unit | No UI needed; fastest, most precise. |
| Text/control appearing or changing after a user action | Component | This is where most criteria live. |
| Behavior that depends on a server response | Component + MSW | Runs real fetch/parse/error code. |
| Moving between screens, routing, persistence across reloads | E2E | Only the real app exercises this. |
| Exact pixels, spacing, theming, regression | Visual | Pixels only — always paired with a behavioral test. |

Heuristics:
- **Default to the component layer.** "User does X → sees Y" belongs there, and it's cheap.
- **One E2E per user-meaningful flow**, not per criterion. E2E is slow/flaky; spend it where
  crossing screens is the point.
- **Never let visual stand alone** — a screenshot diff can't tell you the button did anything.
- Count of tests is not the goal. One honest test per criterion beats ten that assert markup.

## Step 3 — Write, then prove honesty (mutation check)

After writing each test, break the code it covers and confirm the test goes **red**:

- Flip the guard: `if (!name)` → `if (name)`.
- Delete the handler body / the request call.
- Return the wrong value from the mocked endpoint.

If the test stays green, it's asserting the wrong thing (often a mock, or implementation
detail). Fix the test. Then restore the code and confirm green. A test you've never seen fail
proves nothing.

## Step 4 — Observe the real app

Green output is leg one. Boot the app and watch it satisfy each criterion (leg two):
Playwright headed/screenshot on web, Maestro on a simulator for mobile. Use the built-in
`verify` / `run` skills to launch and drive, and `visual-debug` to inspect screenshots and
recordings as an annotated montage. Tick each criterion off against what you actually see.

A feature is done when: every criterion has an honest (mutation-checked) automated test AND
you have watched the real app do it.

## False-green catalog — what makes tests lie

| Anti-pattern | Why it lies | Instead |
|---|---|---|
| Assert only `mockHandler.toHaveBeenCalled()` | Proves wiring, not outcome | Also assert the visible result |
| Whole-component/screen snapshot | Passes on bugs, fails on churn | Assert specific behavior |
| Query by CSS class / `testID` everywhere | Survives when accessible name regresses | Query by role/label/text |
| Mock your own feature module/hook | Real code path never runs | Mock only the network boundary |
| Missing `await`/`waitFor` on async | Asserts before re-render | Await user events and appearance |
| Test mirrors the implementation | Breaks on refactor, passes on bugs | Test observable behavior |
| Never seen the test fail | Might assert a tautology | Mutation check every new test |

## Quick loop (memorize)

```
criteria (observable) → pick layer → write test → mutation-check (see it red)
→ run & observe real app → tick each criterion
```
