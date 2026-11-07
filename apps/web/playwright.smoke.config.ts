import { defineConfig, devices } from '@playwright/test'

// Production smoke suite.
//
// Runs against a REAL deployed environment (production by default) and is
// STRICTLY READ-ONLY: it creates no users, mutates nothing, and sends no
// request that mutates state. The two POSTs it does make are rejected at the
// CSRF layer before any handler runs — that rejection IS the assertion.
//
// Why a separate config from the other two suites:
//   - playwright.config.ts          → built dashboard + mocked API (CI gate)
//   - playwright.integration.config.ts → real backend from `just dev`
//   - this one                     → a real DEPLOYED environment, over the internet
//
// The distinction that matters: the first two prove the code and config are
// right, and both did so while production was serving no CSP header at all.
// nginx `add_header` inheritance is replace-not-merge, so a location block
// silently stripped the security headers from every HTML response — the file
// still read as though CSP were enforced, and csp.spec.ts, which greps that
// file, stayed green throughout. Only asserting against the bytes a live edge
// actually returns catches that class of bug, which is what this suite does.
//
// Retries are on: this crosses the public internet and Cloudflare, so a single
// transport blip should not read as a failed deploy.
const baseURL = process.env.SMOKE_BASE_URL ?? 'https://app.app.com'

export default defineConfig({
  testDir: './tests/smoke',
  testMatch: '**/*.spec.ts',
  fullyParallel: true,
  retries: 2,
  reporter: process.env.CI ? [['github'], ['list']] : [['list']],
  use: {
    baseURL,
    // A smoke run is diagnostic: when it fails against prod you want the
    // evidence in hand, not a rerun with tracing switched on.
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  // No webServer: the environment under test is deployed, not started by us.
})
