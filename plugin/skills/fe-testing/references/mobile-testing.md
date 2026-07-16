# Mobile testing — React Native / Expo

Reference for the `fe-testing` skill. Stack: React Native + Expo, Jest (`jest-expo`) for
unit/component, `@testing-library/react-native` (RNTL) for behavior, Maestro for E2E and
screenshots (Detox as the heavier alternative).

## Setup (only if no test tooling exists)

```bash
# Component + unit (Expo-managed)
npx expo install jest-expo jest react-test-renderer
npm i -D @testing-library/react-native @testing-library/jest-native
# E2E — Maestro (recommended: simple YAML flows, no native build changes)
curl -fsSL "https://get.maestro.mobile.dev" | bash
```

`package.json`:

```jsonc
"scripts": {
  "test": "jest",
  "test:watch": "jest --watch"
},
"jest": {
  "preset": "jest-expo",
  "setupFilesAfterEnv": ["@testing-library/jest-native/extend-expect"]
}
```

## Component tests — the 80% case

Same discipline as web: query by accessible role/text, drive with events, assert the
observable result. RNTL exposes queries by role, text, label text, and `testID` (last resort).

```tsx
import { render, screen, fireEvent, waitFor } from "@testing-library/react-native";
import { ProfileForm } from "./ProfileForm";

test("shows a field error and sends no request on empty required field", async () => {
  const onSubmit = jest.fn();
  render(<ProfileForm onSubmit={onSubmit} />);

  fireEvent.press(screen.getByRole("button", { name: /save/i }));

  expect(await screen.findByText(/name is required/i)).toBeTruthy();
  expect(onSubmit).not.toHaveBeenCalled();
});

test("disables the button while saving and shows success", async () => {
  render(<ProfileForm />);

  fireEvent.changeText(screen.getByLabelText(/name/i), "Ada");
  fireEvent.press(screen.getByRole("button", { name: /save/i }));

  expect(screen.getByRole("button", { name: /saving/i })).toBeDisabled();
  expect(await screen.findByText(/saved/i)).toBeTruthy();
});
```

Accessibility handles that make queries robust (add them to the component, they help users too):
- `accessibilityLabel` → `getByLabelText`
- `accessibilityRole="button"` + text → `getByRole("button", { name })`
- Reach for `testID` only when there's genuinely no accessible handle.

## Mocking the network

Prefer MSW (`msw/native`) so the real fetch/parse/error path runs. If MSW isn't set up,
`jest.mock` the HTTP client module — but never mock your own feature logic, only the transport.

```ts
server.use(http.post("*/api/profile", () => new HttpResponse(null, { status: 500 })));
```

## E2E — Maestro (preferred)

Flows are YAML, run against a simulator/emulator or device, and can screenshot each step.

```yaml
# .maestro/save-profile.yaml
appId: com.yourco.app
---
- launchApp
- tapOn: "Name"
- inputText: "Ada"
- tapOn: "Save"
- assertVisible: "Saved"
- takeScreenshot: saved-confirmation
```

```bash
maestro test .maestro/save-profile.yaml     # runs on the booted simulator
```

Reserve E2E for flows that cross screens or need navigation/persistence — a few high-value
flows, not a suite mirroring every component test.

### Detox (heavier alternative)

Use only if you need tighter native synchronization than Maestro provides. Detox requires a
native debug build and `detox.config.js`; its tests are JS (`element(by.id(...))`,
`.tap()`, `expect(...).toBeVisible()`). More setup and flakier to maintain — default to
Maestro unless there's a concrete reason.

## Observe the real app (leg two)

```bash
npx expo start &        # boot Metro; open on iOS simulator / Android emulator
```

- Maestro `takeScreenshot` steps write PNGs you can `Read` (Claude vision) or feed to the
  `visual-debug` skill for an annotated montage.
- Manual device/simulator screenshot also works: `xcrun simctl io booted screenshot /tmp/x.png`
  (iOS) or `adb exec-out screencap -p > /tmp/x.png` (Android).

Walk the criteria list from the feature and tick each one against what the simulator shows.

## False-green traps (mobile)

- `react-test-renderer` snapshots of a whole screen — brittle and bug-blind; assert behavior
  instead.
- Testing against a mocked navigation prop instead of the real navigator — passes while the
  actual screen transition is broken. Cover cross-screen flows with Maestro.
- Querying only by `testID` — regressions in the user-visible label go undetected.
- Forgetting `await`/`waitFor` around async state — assertions run before the re-render.
- Mocking your own data layer to return the success shape — the parse/error handling never
  executes, so its bugs ship.
