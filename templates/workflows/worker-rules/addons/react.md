---
name: react
frameworks: ["react", "react-native", "expo"]
test_frameworks: ["jest", "vitest", "testing-library"]
---

# Testing Rules: React

## MUST
- Wrap state-updating code in `act()` when testing outside testing-library's built-in act wrapping
- Use `@testing-library/react` queries (`getByRole`, `getByLabelText`, `getByText`) over `getByTestId`
- Call `cleanup()` after each test (or rely on `afterEach` auto-cleanup from `@testing-library/react`)
- Test hooks with `renderHook()` from `@testing-library/react`, not by embedding them in dummy components
- Assert on user-visible output — text, roles, aria attributes — not on internal state
- Use `userEvent` over `fireEvent` for simulating interactions (closer to real browser behavior)
- Mock only the boundary (fetch, modules) — keep component logic unmocked
- Test each loading/error/success state branch explicitly for async components

## MUST NOT
- Do not query by class name or element tag — use semantic queries
- Do not test implementation details (state variable names, internal refs)
- Do not share rendered component instances across tests — each test must call `render()` independently
- Do not assert on snapshot diffs for logic-heavy components — write explicit assertions
- Do not call hooks outside `renderHook` or a React component body in tests
- Do not forget to `await` when using `findBy*` queries or `waitFor`

## Edge Case Checklist
- Component re-renders on prop change without losing internal state
- Async data fetch — loading state shown, error state shown, success state shown
- Event handler fires exactly once per user action (no double-trigger)
- Controlled inputs reflect new value after `userEvent.type`
- Custom hooks clean up subscriptions/timers on unmount
- Context consumers receive updated value after provider value change
- React Native: test on both iOS and Android render paths when layout differs
