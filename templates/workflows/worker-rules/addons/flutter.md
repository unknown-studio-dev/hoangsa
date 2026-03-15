---
name: flutter
frameworks: ["flutter", "dart"]
test_frameworks: ["flutter_test", "mockito", "bloc_test"]
---

# Testing Rules: Flutter

## MUST
- Use `pumpWidget` with a `MaterialApp` (or `WidgetsApp`) wrapper for every widget test; bare widget pumping causes theme and localisation lookup failures
- Call `tester.pump()` after triggering actions to flush the frame; use `tester.pumpAndSettle()` only when animations must complete — it can time out on infinite animations
- Test BLoC/Cubit logic with `bloc_test`'s `blocTest<B, S>()` helper; assert on emitted state sequences, not on bloc internals
- Generate mocks with `build_runner` and `@GenerateMocks`; commit the `.mocks.dart` files and regenerate with `dart run build_runner build --delete-conflicting-outputs`
- Gate golden tests behind a separate test profile (e.g., `--tags golden`) and regenerate baselines explicitly with `--update-goldens`; never regenerate goldens silently on CI

## MUST NOT
- Do not use `find.byType(Widget)` when a more specific finder (`find.text`, `find.byKey`, `find.byTooltip`) is available — it makes tests fragile to widget tree changes
- Do not test platform channel calls with real plugins in unit/widget tests; mock `MethodChannel` with `TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler`
- Do not call `pumpAndSettle` when testing widgets with `StreamBuilder` or looping animations — it will time out; use `pump(duration)` instead
- Do not assert on pixel-perfect golden baselines across different Flutter versions without pinning the engine version in CI
- Do not instantiate BLoCs directly in widget tests without providing them via `BlocProvider`; test the wired widget, not a detached one

## Edge Case Checklist
- Platform channel mocking: reset handlers in `tearDown` to avoid cross-test contamination
- `MediaQuery` and `SafeArea`: wrap widgets in a `MediaQuery` with known constraints to prevent layout overflow errors in tests
- Async `initState`: pump a frame after widget creation to allow futures started in `initState` to complete
- BLoC state equality: implement `Equatable` on all state classes or golden comparisons and `blocTest` sequence assertions will fail
- Golden test fonts: load test fonts with `FontLoader` in `flutter_test_config.dart` to avoid `.notdef` glyph rendering in baselines
