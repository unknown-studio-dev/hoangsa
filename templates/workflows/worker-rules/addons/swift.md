---
name: swift
frameworks: ["swift", "swiftui", "uikit", "combine"]
test_frameworks: ["xctest", "quick", "nimble"]
---

# Testing Rules: Swift

## MUST
- Use `XCTestExpectation` (or `async/await` with `XCTest` in Xcode 13+) for all asynchronous tests; always call `wait(for:timeout:)` with a realistic but bounded timeout
- Test SwiftUI views with `ViewInspector`; call `view.inspect()` after triggering state changes and assert on the resulting view tree
- Test Combine publishers by collecting emitted values with `XCTestExpectation` + `sink`, or use the `eraseToAnyPublisher()` pattern with a test scheduler
- Create a `TestScheduler` (or use `CombineSchedulers`) to control time in Combine pipelines and avoid real `Timer` usage in tests
- Use `Quick`/`Nimble` for BDD-style tests when the subject has many related behaviours that benefit from `describe`/`context`/`it` grouping

## MUST NOT
- Do not use `sleep()` or `usleep()` in tests to wait for async work; always use expectations or structured concurrency
- Do not test private methods directly; test through the public API and expose testability via `@testable import`
- Do not access UIKit UI elements from a background thread in test assertions; dispatch to `DispatchQueue.main` or use `MainActor`
- Do not leave unfulfilled expectations — always pair every `expectation` creation with `wait(for:timeout:)`
- Do not rely on `viewDidAppear` being called automatically in `XCTestCase`; call lifecycle methods manually or use a hosting controller

## Edge Case Checklist
- Actor isolation: `@MainActor` functions must be awaited in async test contexts
- SwiftUI `@State` is not accessible from outside the view — test via `ViewInspector` bindings or by observing published output
- Combine `@Published` fires on assignment, not on change — assert after `objectWillChange` settles
- Memory leaks: add `addTeardownBlock { [weak sut] in XCTAssertNil(sut) }` to catch retain cycles
- Simulator vs device: test push notification permission and CoreLocation flows with mocked delegates, not real hardware APIs
