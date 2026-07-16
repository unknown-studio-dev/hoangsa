---
name: go
frameworks: ["go", "gin", "echo", "fiber", "chi", "gorilla", "grpc-go"]
test_frameworks: ["testing", "testify", "gomock"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: Go

## MUST
- Structure HTTP handler tests as table-driven tests: define a `[]struct{ name, method, path string; body io.Reader; wantCode int }` slice and range over it with `t.Run`
- Use `net/http/httptest.NewRecorder()` and `httptest.NewServer()` for handler and integration tests respectively; never spin up a real listener in unit tests
- Generate mocks with `mockgen` and check generated files into the repo under `internal/mocks/`; regenerate with `go generate ./...`
- Use `t.Helper()` in every test helper function so failure lines point to the caller, not the helper
- Use `goleak.VerifyTestMain(m)` in `TestMain` for packages that spawn goroutines, to catch goroutine leaks
- Assert with `testify/assert` (non-fatal) or `testify/require` (fatal); use `require` when subsequent assertions depend on the previous one

## MUST NOT
- Do not use `time.Sleep` to wait for goroutines; use channels, `sync.WaitGroup`, or `goleak`
- Do not share `*testing.T` across goroutines — capture it in a local variable before launching goroutines
- Do not construct real gRPC clients in unit tests; use `bufconn` for in-process transport
- Do not call `os.Exit` inside tests or test helpers
- Do not ignore errors returned by `resp.Body.Close()` — always defer close and check the error

## Edge Case Checklist
- Race conditions: run tests with `-race` flag in CI
- Context cancellation: verify handlers return promptly when `ctx.Done()` is closed
- gRPC status codes: assert `status.Code(err)` not just `err != nil`
- Fiber/Echo path parameter encoding: test with URL-encoded characters in params
- `httptest.Server` cleanup: always call `defer ts.Close()` to release the port
