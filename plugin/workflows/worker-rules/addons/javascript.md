---
name: javascript
frameworks: ["javascript", "nodejs", "node", "bun"]
test_frameworks: ["jest", "vitest", "mocha", "node:test"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: JavaScript / Node.js / Bun

## MUST
- Always `await` async functions in tests; missing `await` silently passes on rejected promises.
- Use `assert.rejects()` / `expect(...).rejects.toThrow()` to test promise rejections.
- Use `jest.useFakeTimers()` / `vi.useFakeTimers()` for code that uses `setTimeout`/`setInterval`.
- Restore real timers and stubs in `afterEach` to avoid test-order dependencies.
- Test EventEmitter behaviour by listening for events before the action that triggers them.
- Use `stream.pipeline()` in tests when testing Node.js streams; assert on `finish`/`error` events.
- For callback-style APIs: wrap in `util.promisify` in the test or use `done` callback correctly.
- In Bun: use `Bun.serve()` test utilities or `fetch` against `localhost` for HTTP route tests.

## MUST NOT
- Do not use bare `Promise` in tests without `.catch()` or `await` — unhandled rejections hide bugs.
- Do not rely on `process.env` values set outside the test — set and restore them in `beforeEach`/`afterEach`.
- Do not share mutable module-level state between tests; reset in `beforeEach`.
- Do not use `done` callback with `async` test functions — pick one style per test.
- Do not call `process.exit()` inside code under test without mocking `process.exit` first.

## Edge Case Checklist
- Async: function rejects (not silently swallows) on upstream failure.
- Callbacks: error-first callback receives `Error` instance, not a string, on failure.
- EventEmitter: `error` event without a listener throws — ensure tests attach one.
- Timers: debounced/throttled functions fire correctly at boundary intervals.
- Streams: backpressure is handled; writable `drain` event fires after high-watermark pause.
- ESM/CJS: named exports resolve correctly in both module formats if dual-published.
