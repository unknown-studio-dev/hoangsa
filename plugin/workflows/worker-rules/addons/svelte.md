---
name: svelte
frameworks: ["svelte", "sveltekit"]
test_frameworks: ["vitest", "jest", "testing-library"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: Svelte

## MUST
- Use `@testing-library/svelte` for component rendering — it handles lifecycle mounting and cleanup
- Await `tick()` from `svelte` after setting writable store values or updating props before asserting
- Destroy the component with `component.$destroy()` in `afterEach` if not using testing-library cleanup
- Test Svelte stores in isolation: import the store, call `.set()` / `.update()`, then `get(store)` to assert
- Use `fireEvent` or `userEvent` and await the result before checking DOM changes driven by reactive declarations
- For SvelteKit: mock `$app/navigation`, `$app/stores`, and `$env/*` modules in vitest config or per-test

## MUST NOT
- Do not access `$store` shorthand syntax inside test files (only valid in `.svelte` components) — use `get(store)` instead
- Do not mutate reactive declarations (`$:`) indirectly and assert without awaiting `tick()`
- Do not skip component destroy — Svelte lifecycle hooks (`onDestroy`) must run in tests to catch cleanup bugs
- Do not test internal component variables directly — test through rendered output or exported props
- Do not import SvelteKit runtime modules (`$app/*`) without mocking them; they throw outside the SvelteKit runtime

## Edge Case Checklist
- Reactive declaration (`$:`) updates when its dependency changes after `tick()`
- `onMount` side-effects execute and are cleaned up by `onDestroy`
- Writable store value reflected in multiple subscribers after one `.set()` call
- `{#if}` / `{#each}` blocks re-render correctly after store or prop change
- Slot content renders in the correct position in the component output
- SvelteKit: `load` function returns correct data shape for the page component
- SvelteKit: form actions handle valid and invalid submissions correctly
