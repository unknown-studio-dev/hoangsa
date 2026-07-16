# Web testing — React / Vite

Reference for the `fe-testing` skill. Stack: React + Vite, Vitest for unit/component,
Testing Library for behavior, MSW for network, Playwright for E2E and screenshots.

## Setup (only if no test tooling exists)

```bash
# Component + unit
npm i -D vitest @testing-library/react @testing-library/user-event @testing-library/jest-dom jsdom
# Network mocking
npm i -D msw
# E2E + screenshots
npm i -D @playwright/test && npx playwright install
```

`vitest.config.ts` (or add `test` to `vite.config.ts`):

```ts
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: true,
    setupFiles: ["./src/test/setup.ts"],
  },
});
```

`src/test/setup.ts`:

```ts
import "@testing-library/jest-dom/vitest";
// If using MSW: start the server here (see below).
```

`package.json` scripts:

```jsonc
"test": "vitest run",
"test:watch": "vitest",
"e2e": "playwright test"
```

## Component tests — the 80% case

Query the way a user finds things (role/label/text), drive with `user-event`, assert the
observable outcome.

```tsx
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ProfileForm } from "./ProfileForm";

test("shows a field error and sends no request on empty required field", async () => {
  const user = userEvent.setup();
  const onSubmit = vi.fn();
  render(<ProfileForm onSubmit={onSubmit} />);

  await user.click(screen.getByRole("button", { name: /save/i }));

  expect(await screen.findByText(/name is required/i)).toBeInTheDocument();
  expect(onSubmit).not.toHaveBeenCalled();
});

test("disables the button while saving and shows a success toast", async () => {
  const user = userEvent.setup();
  render(<ProfileForm />);

  await user.type(screen.getByLabelText(/name/i), "Ada");
  await user.click(screen.getByRole("button", { name: /save/i }));

  expect(screen.getByRole("button", { name: /saving/i })).toBeDisabled();
  expect(await screen.findByText(/saved/i)).toBeInTheDocument();
});
```

### Query priority (Testing Library) — highest first

1. `getByRole` (+ `name`) — how assistive tech and users perceive it. Default choice.
2. `getByLabelText` — form fields.
3. `getByPlaceholderText`, `getByText`, `getByDisplayValue`.
4. `getByTestId` — **last resort**, only when there's no accessible handle.

`get*` throws if missing (sync), `find*` awaits appearance (async, for post-fetch/toast),
`query*` returns null (for asserting absence). Use `find*` for anything that appears after an
`await`.

## Mocking the network — MSW, at the boundary

Never stub your own `useUser()` hook or fetch wrapper. Intercept HTTP so the real code path
runs.

```ts
// src/test/server.ts
import { setupServer } from "msw/node";
import { http, HttpResponse } from "msw";

export const server = setupServer(
  http.post("/api/profile", () => HttpResponse.json({ ok: true }))
);
```

```ts
// src/test/setup.ts
import { server } from "./server";
beforeAll(() => server.listen({ onUnhandledRequest: "error" }));
afterEach(() => server.resetHandlers());
afterAll(() => server.close());
```

Per-test error case:

```ts
server.use(http.post("/api/profile", () => new HttpResponse(null, { status: 500 })));
```

## E2E — Playwright (only for cross-screen flows)

```ts
// e2e/save-profile.spec.ts
import { test, expect } from "@playwright/test";

test("user saves profile and sees confirmation", async ({ page }) => {
  await page.goto("/profile");
  await page.getByLabel(/name/i).fill("Ada");
  await page.getByRole("button", { name: /save/i }).click();
  await expect(page.getByText(/saved/i)).toBeVisible();
});
```

`playwright.config.ts` can boot your dev server automatically:

```ts
webServer: { command: "npm run dev", url: "http://localhost:5173", reuseExistingServer: true }
```

## Observe the real app (leg two)

Run the dev server and capture what the app actually renders, then read the screenshot back:

```bash
npm run dev &     # note the URL, e.g. http://localhost:5173
```

One-off screenshot without a full spec:

```ts
// e2e/observe.spec.ts — run: npx playwright test e2e/observe.spec.ts
import { test } from "@playwright/test";
test("snapshot the feature", async ({ page }) => {
  await page.goto("http://localhost:5173/profile");
  await page.getByLabel(/name/i).fill("Ada");
  await page.screenshot({ path: "/tmp/profile-filled.png", fullPage: true });
});
```

Then `Read` the PNG (Claude vision) or pass it to the `visual-debug` skill for an annotated
montage. Prefer the built-in `verify` / `run` skills to boot and drive the app when available.

## False-green traps (web)

- Asserting `expect(mockOnClick).toHaveBeenCalled()` **and nothing else** — proves the button
  is wired, not that anything happened. Assert the visible result too.
- Snapshotting an entire component — passes on real bugs, fails on harmless markup churn.
  Snapshot only small, stable, semantically-meaningful output.
- Querying by CSS class / `data-testid` everywhere — tests survive when the accessible name
  (what users rely on) regresses.
- `await user.click(...)` missing its `await` — the assertion runs before React updates.
- Mocking your own data hook to return the happy value — the fetch/parse/error code never
  runs, so its bugs never surface.
