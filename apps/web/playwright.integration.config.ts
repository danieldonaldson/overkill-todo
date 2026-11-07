import { defineConfig, devices } from '@playwright/test'

// Integration E2E: runs the real browser against the REAL backend (no route
// mocking). Assumes the dev stack is already up — `just dev` serves it
// on :4321 (or set DASHBOARD_BASE_URL to the Compose edge, :8080). Because these hit a live API + Postgres, they live in
// tests/integration and are excluded from the default (mock/preview) suite.
//
// The backend's dev mode gives any `e2e-*` / `test-*` / `playwright*` email a
// fixed verification code (123456) and skips real email + Turnstile, which is
// what makes a real signup driveable from a test.
const baseURL = process.env.DASHBOARD_BASE_URL ?? 'http://localhost:4321'

export default defineConfig({
  testDir: './tests/integration',
  testMatch: '**/*.spec.ts',
  // Serial: these create real users; keep the live DB interactions predictable.
  fullyParallel: false,
  workers: 1,
  retries: 0,
  use: { baseURL },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  // No webServer: the stack is provided externally by `just dev`.
})
