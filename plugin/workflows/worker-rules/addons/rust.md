---
name: rust
frameworks: ["rust", "axum", "actix-web", "rocket", "warp", "tokio", "leptos", "tauri"]
test_frameworks: ["cargo-test", "tokio-test"]
priority: 50
inject_position: after_base
allowed_tools: []
pre_invoke_gate: null
---

# Testing Rules: Rust

## MUST
- Annotate async tests with `#[tokio::test]` (or `#[actix_web::test]` for Actix); never use `#[test]` with async bodies.
- Use `tokio::test` with `flavor = "multi_thread"` only when the code under test requires it.
- Define a `MockRepo` / `MockService` trait impl in a `#[cfg(test)]` module to isolate business logic from I/O.
- Test `Result`-returning functions with both `Ok` and `Err` variants; use `assert!(result.is_err())` for error paths.
- Use `axum::test` helpers (`TestClient`) or `actix-web::test::init_service` for HTTP handler tests.
- Place unit tests in the same file as the code under test inside `#[cfg(test)] mod tests { ... }`.
- Use `proptest` or `quickcheck` for functions with large input spaces (parsers, serialisers).

## MUST NOT
- Do not call `unwrap()` in test assertions on `Result`/`Option` without a meaningful failure message — use `expect("context")`.
- Do not share mutable global state between tests; use per-test fixtures or `tokio::sync::Mutex` if unavoidable.
- Do not ignore `#[allow(dead_code)]` warnings in test helpers — they signal unused mocks.
- Do not test private functions through `pub(crate)` hacks; test public API surface instead.
- Do not use `std::thread::sleep` in async tests; use `tokio::time::sleep` with fake-time (`tokio::time::pause()`).

## Edge Case Checklist
- `Result`: every `?`-propagated error path has a corresponding test that triggers it.
- Panics: functions documented as panicking are tested with `#[should_panic(expected = "...")]`.
- Lifetimes: borrowed return values compile without lifetime annotation errors in test call sites.
- Axum/Actix: 404 on unknown routes, correct `Content-Type` header on JSON responses.
- Tokio: tasks spawned in the code under test are joined before assertions (no race conditions).
- Feature flags: `#[cfg(feature = "...")]` gated code is tested with `cargo test --features <flag>`.
