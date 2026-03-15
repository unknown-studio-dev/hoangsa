---
name: angular
frameworks: ["angular"]
test_frameworks: ["jasmine", "karma", "jest"]
---

# Testing Rules: Angular

## MUST
- Configure `TestBed` with the exact providers, imports, and declarations the component under test needs — no more
- Call `fixture.detectChanges()` after setup and after any state mutation before asserting the DOM
- Inject dependencies through `TestBed.inject()` — never instantiate services manually
- Use `fakeAsync` + `tick()` / `flush()` for code that relies on `setTimeout`, `setInterval`, or resolved promises
- Spy on service methods with `jasmine.createSpyObj` or `jest.spyOn` and provide the spy via `TestBed`
- Test Observables with `fakeAsync` + `tick()` or use `marbles` / `TestScheduler` for complex streams
- Destroy the fixture in `afterEach` with `fixture.destroy()` to clean up subscriptions

## MUST NOT
- Do not call `fixture.detectChanges()` only once at the top and skip subsequent calls after state changes
- Do not bypass dependency injection — no `new MyService()` inside tests
- Do not import `AppModule` into `TestBed` — use isolated module declarations per test suite
- Do not leave Observables unsubscribed in tests — this causes false positives from lingering async work
- Do not use `async/await` for Angular's `fakeAsync` zones — use `fakeAsync` + `tick()`
- Do not assert on private component properties — test through the rendered template or public API

## Edge Case Checklist
- Change detection runs after each input property change
- Unsubscription in `ngOnDestroy` stops all active subscriptions
- Form validation errors appear in the DOM after invalid submission
- HTTP calls use `HttpClientTestingModule` and `HttpTestingController.expectOne()`
- Router navigation triggers correct component activation (use `RouterTestingModule`)
- Async pipe unsubscribes automatically — verify no memory leak on component destroy
- Error thrown inside an Observable is caught and handled in the component
