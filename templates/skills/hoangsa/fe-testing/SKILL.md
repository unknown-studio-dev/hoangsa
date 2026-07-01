---
name: fe-testing
description: "This skill should be used when the user is building, changing, or trying to verify a frontend feature on web (React/Vite) or React Native/Expo, and is unsure how to prove it actually works. Triggers on phrases like 'how do I test this component/screen', 'verify this feature', 'make sure this works on web/mobile', 'is this covered by tests', 'my tests pass but the feature is broken', 'no tests yet', or before shipping any FE change. Guides the full loop: define verifiable success criteria before coding, write tests at the correct layer (logic/component/E2E/visual), and confirm behavior by running the real app."
allowed-tools:
  - Bash
  - Read
  - Glob
  - Grep
  - Edit
  - Write
license: MIT
compatibility: "Claude Code >= 1.0; Node.js project (web: React/Vite; mobile: React Native/Expo)"
metadata:
  author: hoangsa
  version: 0.0.1
  category: testing
  spec: agentskills.io/1.0
---

<objective>
Make a frontend feature — on web (React/Vite) or React Native/Expo — provably correct
instead of "looks done". A feature is verified only when BOTH legs exist:

1. **A test that turns red if the behavior breaks** — automated, at the cheapest layer that
   still exercises the real behavior.
2. **An observation of the real app doing the thing** — a run, a screenshot, or a driven E2E
   session — not just green unit output.

Green tests that stay green when you break the code verify nothing. This skill drives the
loop: define verifiable criteria BEFORE coding → pick the right test layer → write the test →
run and watch the real app → prove the test is honest with a mutation check.
</objective>

<triggers>
- "How do I test this component / screen / hook?"
- "Verify this feature works" / "make sure it works on web/mobile"
- "Is this covered by tests?" / "what should I test here?"
- "My tests pass but the feature is broken" (false-green)
- "There are no tests yet" / setting up FE testing from scratch
- Building a new feature, form, list, navigation flow, or data-fetching screen
- Before shipping / opening a PR on any FE change
</triggers>

<principles>
This skill is an application of CLAUDE.md §4 (Goal-Driven Execution) to frontend. The core
rules:

- **Criteria before code.** You cannot verify what you did not define. Write the pass/fail
  checks first, in user-observable terms.
- **Test behavior, not implementation.** Assert what the user sees and can do, not internal
  state, prop names, or CSS classes. Tests coupled to implementation break on refactor and
  pass on real bugs.
- **Cheapest honest layer wins.** Push each check to the lowest layer of the pyramid that
  still exercises the real behavior. Don't spin up a browser to test a pure function.
- **Two legs or it isn't verified.** Automated red-on-break test AND a real-app observation.
- **A test you haven't seen fail is a hypothesis, not a test.** Prove it (mutation check).
</principles>

<flows>

<flow name="1-detect-stack">
Before anything, detect what you're working in. Do not assume.

```bash
# web vs mobile, and existing test tooling
cat package.json 2>/dev/null | grep -A40 '"\(dependencies\|devDependencies\|scripts\)"'
ls vite.config.* app.json app.config.* metro.config.* playwright.config.* vitest.config.* jest.config.* 2>/dev/null
ls .maestro/ e2e/ 2>/dev/null
```

Classify:
- **Web** — `vite`/`react-dom` present. Expect Vitest + Testing Library + Playwright.
- **React Native / Expo** — `react-native`/`expo` present. Expect Jest (`jest-expo`) +
  `@testing-library/react-native` + Maestro (or Detox).

If no test tooling exists, see the setup sections in the reference files before writing tests —
do not hand-roll a runner.
</flow>

<flow name="2-define-criteria">
Turn the feature request into concrete, observable pass/fail checks BEFORE writing code.
Write them down (in the plan, a comment, or the test file's `describe` block).

Format each as Given / When / Then in **user-observable** terms:

```
Feature: "Add a save button to the profile form"
- Given a filled form, When I click Save, Then a success toast appears and the button is disabled while saving.
- Given a network error, When I click Save, Then an error message appears and the form stays editable.
- Given an empty required field, When I click Save, Then a field error shows and no request is sent.
```

Rules:
- Each criterion names something a **user or the API can observe** — a visible element,
  a navigation, a network call. Never "state.x === true".
- If you can't phrase it observably, you don't understand the feature yet — ask.
- These criteria become your test names and your run-and-observe checklist. One artifact,
  used twice.

See `references/verification-playbook.md` for the criteria template and how to map each
criterion to a layer.
</flow>

<flow name="3-pick-layer">
For each criterion, pick the cheapest layer that still exercises the real behavior:

| Behavior under test | Layer | Web tool | Mobile tool |
|---|---|---|---|
| Pure function / hook logic, formatting, reducers | **Unit** | Vitest | Jest (`jest-expo`) |
| Component renders + responds to user input (types, clicks, shows error) | **Component** | `@testing-library/react` + `user-event` | `@testing-library/react-native` |
| Network-dependent component behavior | **Component + mocked network** | MSW | MSW / `jest.mock` fetch |
| Multi-screen flow, navigation, real routing/persistence | **E2E** | Playwright | Maestro (preferred) or Detox |
| Exact pixels / layout / visual regression | **Visual** | Playwright screenshot | Maestro screenshot / device screenshot |

Guidance:
- Most FE criteria land at the **component** layer — that's where "user does X, sees Y" lives.
  Favor it over E2E when routing/persistence aren't part of the behavior.
- Reserve **E2E** for flows that genuinely cross screens or need the real backend/router.
  E2E is slow and flaky; a few high-value flows beat dozens.
- **Visual** verifies pixels, not logic — pair it with a behavioral test, never alone.
- Don't test framework internals (React state, navigation library internals) or third-party
  components you didn't write.
</flow>

<flow name="4-write-test">
Write the test at the chosen layer. Full setup + idiomatic examples are in the references —
don't inline a framework from memory:

- **Web** → `references/web-testing.md` (Vitest, Testing Library queries, `user-event`, MSW,
  Playwright).
- **Mobile** → `references/mobile-testing.md` (Jest/`jest-expo`, RNTL, Maestro flows, Detox).

Non-negotiables regardless of stack:
- **Query the way a user finds things**: by role, label, text, accessibility. Not by CSS
  selector, `data-testid`, or component internals. (`testID`/`data-testid` only as a last
  resort for elements with no accessible handle.)
- **Drive with real user events** (`user-event` on web, `fireEvent`/RNTL on mobile), not by
  calling handlers directly.
- **Assert the observable outcome** from your criteria — the visible text, the disabled
  button, the fired request — not intermediate state.
- **Mock at the network boundary** (MSW), not by stubbing your own modules. Stubbing your own
  code is how tests pass while the feature is broken.
</flow>

<flow name="5-run-and-observe">
Green unit output is leg one. Now watch the real app do the thing (leg two). This is where
most "verified" features are actually caught broken.

**Web:**
```bash
npm run dev &            # or the project's dev script; note the URL/port
# Then drive it with Playwright (headed or screenshot) and read the screenshot back:
npx playwright test --headed          # if E2E specs exist
# or a one-off screenshot script — see references/web-testing.md "observe" section
```
Prefer the built-in `verify` and `run` skills to boot and drive the app, and `visual-debug`
to turn screenshots/recordings into an annotated montage for inspection.

**Mobile (Expo):**
```bash
npx expo start &                       # boot Metro
# Maestro drives the simulator and can screenshot each step:
maestro test .maestro/<flow>.yaml
```
Maestro's `takeScreenshot` steps + the `visual-debug` skill give you frame-by-frame evidence.

Walk your criteria list from flow 2 and tick each one off against what you actually see.
If a criterion can't be observed, it isn't done.
</flow>

<flow name="6-mutation-check">
Prove the test is honest before you trust it. For each new test:

1. Break the code it covers (flip a condition, delete the handler, hardcode the wrong value).
2. Run the test. **It MUST go red.** If it stays green, the test asserts the wrong thing (or
   asserts a mock) — fix the test, not the code.
3. Restore the code. Confirm green again.

A test you've only ever seen pass is a hypothesis. This 30-second step is the difference
between "tests pass" and "feature verified".
</flow>

</flows>

<rules>
| Rule | Detail |
|------|--------|
| **Criteria before code** | Write observable Given/When/Then checks before implementing. No criteria → nothing to verify. |
| **Two legs** | Every feature needs a red-on-break automated test AND a real-app observation. One alone is not "verified". |
| **Behavior over implementation** | Assert what the user/API observes. Never assert internal state, prop names, or CSS classes. |
| **Query like a user** | Role / label / text / a11y. `testID`/`data-testid` is a last resort, not a default. |
| **Cheapest honest layer** | Push each check to the lowest pyramid layer that still exercises real behavior. |
| **Mock only the network** | Mock at the fetch/HTTP boundary (MSW). Never stub your own modules to make a test pass. |
| **Mutation check** | A new test must be seen failing (break the code) before it's trusted. |
| **Detect, don't assume** | Detect web vs mobile and existing tooling from the repo before writing tests. |
</rules>

<references>
- **`references/web-testing.md`** — Web setup (Vitest + Testing Library + `user-event` + MSW + Playwright), idiomatic component/E2E examples, and the "observe the real app" recipe.
- **`references/mobile-testing.md`** — React Native/Expo setup (`jest-expo` + `@testing-library/react-native`, Maestro flows, Detox), device/simulator observation, and screenshot evidence.
- **`references/verification-playbook.md`** — The pre-code criteria template, criterion→layer mapping worksheet, and the false-green anti-pattern catalog (what makes tests lie).
</references>
