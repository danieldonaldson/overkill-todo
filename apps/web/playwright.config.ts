import { defineConfig, devices } from '@playwright/test'

const externalBaseURL = process.env.DASHBOARD_BASE_URL

// Per-worktree (PREVIEW_PORT in .worktree.env). It must differ between checkouts:
// reuseExistingServer would otherwise run this suite against another worktree's build.
const previewPort = Number(process.env.PREVIEW_PORT ?? 4323)

export default defineConfig({
  testDir: './tests',
  testMatch: '**/*.spec.ts',
  // Integration specs hit a real backend and run via playwright.integration.config.ts.
  // integration/ needs the full stack; smoke/ targets a live deployment (own configs).
  testIgnore: ['**/integration/**', '**/smoke/**'],
  fullyParallel: true,
  retries: 0,
  use: {
    baseURL: externalBaseURL ?? `http://localhost:${previewPort}`,
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: externalBaseURL
    ? undefined
    : {
        // --ignore-lock keeps the server in the foreground: Astro 7.4+ otherwise
        // detaches `astro preview` when run under an AI agent, and Playwright
        // reads the early exit as a crash.
        command: `pnpm preview --port ${previewPort} --ignore-lock`,
        port: previewPort,
        reuseExistingServer: !process.env.CI,
        timeout: 30000,
      },
})
