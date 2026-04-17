---
name: expressjs
frameworks: ["express", "koa", "fastify", "hono"]
test_frameworks: ["jest", "vitest", "supertest", "mocha"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: Express / Koa / Fastify / Hono

## MUST
- Mount only the router or middleware under test; do not start a full server for unit tests.
- Use `supertest(app)` (or framework equivalent) to make in-process HTTP requests without binding a port.
- Test error-handling middleware by passing an `Error` object as the first argument to `next`.
- Assert on `res.status`, `res.body`, and `res.headers` in every route test.
- Mock external service calls (DB, HTTP clients) before the middleware under test runs.
- Test middleware in isolation by creating a minimal app with only that middleware attached.
- For Fastify: call `app.ready()` before making requests and `app.close()` in `afterAll`.
- For Koa: use `supertest(app.callback())` to avoid port conflicts.

## MUST NOT
- Do not call `app.listen()` inside test files — use in-process request helpers instead.
- Do not rely on global state between tests; reset shared variables in `beforeEach`.
- Do not skip asserting on error responses — test 4xx and 5xx paths explicitly.
- Do not mock `req`/`res` with plain objects unless you mock every property accessed by the middleware.
- Do not leave open server handles; always close/destroy in `afterAll`.

## Edge Case Checklist
- Middleware calls `next()` exactly once on the happy path.
- Error-handling middleware receives the correct `err.status` and `err.message`.
- Missing required query/body params return 400 before hitting business logic.
- Unknown routes return 404 (not a silent hang or 200).
- Malformed JSON body returns 400 with a readable message.
- Async middleware errors propagate to the error handler (not swallowed).
