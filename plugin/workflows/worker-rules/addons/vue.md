---
name: vue
frameworks: ["vue", "nuxt"]
test_frameworks: ["vitest", "jest", "vue-test-utils"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: Vue

## MUST
- Mount components with `mount()` or `shallowMount()` from `@vue/test-utils` — choose based on whether child components are under test
- Await `nextTick()` after any reactive state mutation before asserting DOM output
- Use `wrapper.find()` with semantic selectors (`[role]`, `aria-label`) over CSS class selectors
- Test Pinia stores in isolation by calling `setActivePinia(createPinia())` before each test
- Use `wrapper.trigger('event')` and await the result before asserting
- Unmount the component with `wrapper.unmount()` in `afterEach` to prevent memory leaks
- For Nuxt: mock `useRoute`, `useRouter`, and Nuxt auto-imports explicitly in test setup

## MUST NOT
- Do not mutate `wrapper.vm` internal state directly to drive tests — go through props or user actions
- Do not skip `await nextTick()` after emitting events or setting reactive data
- Do not import `pinia` store instances without `setActivePinia` — tests will bleed state
- Do not use `shallowMount` when the test depends on child component behavior
- Do not assert on raw HTML strings — use `wrapper.find()` and `.text()` / `.attributes()`
- Do not test computed properties in isolation — test them through rendered output

## Edge Case Checklist
- Reactive `ref`/`reactive` values update the DOM after mutation + `nextTick`
- `watch` and `watchEffect` fire with correct old/new values
- Component emits the correct event with correct payload on user interaction
- Pinia actions mutate state and side-effects run as expected
- `v-if`/`v-show` toggles render or hide correct elements
- Nuxt: page component receives correct data from `useAsyncData` / `useFetch` mock
- Slots render provided content in the correct location
