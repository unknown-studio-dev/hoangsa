---
name: typescript
frameworks: ["typescript"]
test_frameworks: ["jest", "vitest", "mocha"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: TypeScript

## MUST
- Type all mock objects explicitly; use the actual interface or class type, not `any`.
- Use `satisfies` or `as const` to preserve literal types in test fixtures.
- Use `jest.mocked()` / `vi.mocked()` to get typed access to mocked functions.
- Test generic functions with multiple concrete type arguments to cover type-variable behaviour.
- Assert on discriminated union branches by narrowing before asserting (use `if`/`switch`, not casts).
- Keep `tsconfig` for tests strict (`strict: true`) — separate `tsconfig.test.json` if needed.
- Use `expectTypeOf` (vitest) or `@ts-expect-error` to assert compile-time type errors in tests.

## MUST NOT
- Do not use `as any` to bypass TypeScript errors in mocks — fix the type instead.
- Do not cast result to expected type before asserting — assert on the raw return value.
- Do not disable `strict` or `noImplicitAny` for test files.
- Do not import types with `import` when `import type` is sufficient; keeps test boundaries clear.
- Do not duplicate type definitions in test files — import from source.

## Edge Case Checklist
- Narrowing: `unknown` return values are narrowed before use without unsafe casts.
- Optional fields: functions handle `undefined` properties without throwing.
- Generics: functions with constrained type params reject invalid types at compile time.
- Enum exhaustiveness: `switch` over an enum has a `default` that `assertNever`s unreachable arms.
- Overloads: each overload signature is exercised with a matching call signature in tests.
- `strictNullChecks`: null/undefined paths are explicitly tested, not left as implicit branches.
