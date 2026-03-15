---
name: nextjs
frameworks: ["nextjs", "next"]
test_frameworks: ["jest", "vitest", "testing-library", "playwright"]
---

# Testing Rules: Next.js

> Extends react.md rules. Apply all React testing rules plus the ones below.

## MUST
- Distinguish Server Components (no hooks, no browser APIs) from Client Components (`"use client"`) — test them differently
- Test Server Components as plain async functions: call the component function directly and assert on the returned JSX or rendered output
- Mock `next/navigation` (`useRouter`, `usePathname`, `useSearchParams`) for Client Components that use routing
- Test API route handlers by importing the handler function and calling it with a mock `Request` object — do not spin up a server
- Test middleware by calling the middleware function with a mock `NextRequest` and asserting on the returned `NextResponse`
- Use Playwright for end-to-end tests covering SSR hydration, navigation, and middleware redirects
- Mock `next/headers` (`cookies()`, `headers()`) in unit tests for Server Actions and server-only utilities

## MUST NOT
- Do not render Server Components with `@testing-library/react` render — they are not client-renderable in unit tests
- Do not call `useRouter()` in a test file directly — mock the module and provide return values
- Do not test `getServerSideProps` / `getStaticProps` through the page component — test them as standalone exported functions
- Do not rely on `next dev` being running for unit or integration tests — all network calls must be mocked
- Do not skip testing the error boundary and not-found paths for dynamic routes

## Edge Case Checklist
- Server Component fetches data and passes correct props to Client Component children
- Dynamic route (`[id]`) renders correct content for valid and invalid params
- Middleware redirects unauthenticated requests to login and passes authenticated requests through
- API route returns correct status codes for valid input, invalid input, and internal errors
- Client Component hydrates without mismatch between server-rendered and client-rendered HTML
- `loading.tsx` and `error.tsx` segments render under the correct conditions
- Server Action validates input server-side and returns structured error on failure
