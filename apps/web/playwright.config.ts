import { defineConfig, devices } from '@playwright/test'

const externalBaseURL = process.env.DASHBOARD_BASE_URL

export default defineConfig({
  testDir: './tests',
  testMatch: '**/*.spec.ts',
  // Integration specs hit a real backend and run via playwright.integration.config.ts.
  // integration/ needs the full stack; smoke/ targets a live deployment (own configs).
  testIgnore: ['**/integration/**', '**/smoke/**'],
  fullyParallel: true,
  retries: 0,
  use: {
    baseURL: externalBaseURL ?? 'http://localhost:4323',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: externalBaseURL
    ? undefined
    : {
        command: 'pnpm preview --port 4323',
        port: 4323,
        reuseExistingServer: !process.env.CI,
        timeout: 30000,
      },
})
