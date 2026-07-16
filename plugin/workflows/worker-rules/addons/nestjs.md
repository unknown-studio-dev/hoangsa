---
name: nestjs
frameworks: ["nestjs"]
test_frameworks: ["jest", "supertest"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: NestJS

## MUST
- Use `Test.createTestingModule()` to bootstrap modules under test with real DI
- Provide mock dependencies via `useValue` or `useFactory` in the test module providers array
- Test guards by calling `canActivate()` directly with a mock `ExecutionContext`
- Test pipes by calling `transform()` directly with sample input and metadata
- Test interceptors by calling `intercept()` with a mock `CallHandler` that returns an observable
- Use `supertest` with `app.getHttpServer()` for e2e tests that validate full request/response cycles
- Close the app with `app.close()` in `afterAll` to release connections
- Assert on HTTP status codes and response body shape in e2e tests

## MUST NOT
- Do not instantiate services manually with `new` — always resolve through the DI container
- Do not skip closing the test app; open handles cause Jest to hang
- Do not test the NestJS framework internals (routing, module resolution) — test your code only
- Do not use `any` to silence TypeScript errors in mock definitions
- Do not share app instances across test files without resetting module state

## Edge Case Checklist
- Guard blocks unauthenticated requests (missing/invalid token)
- Pipe throws `BadRequestException` on invalid or missing required fields
- Interceptor transforms response correctly for both success and error paths
- Exception filters return the expected JSON shape and HTTP status
- Circular dependency between providers is caught early (module throws on init)
- e2e: 404 on unknown routes, 405 on wrong HTTP method
