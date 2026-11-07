import { expect, test, type Page } from '@playwright/test'

// Real end-to-end signup against the live backend + Postgres (no mocking).
// Exercises the actual wire contract: register → verify (fixed dev code) →
// complete-signup, with real CSRF cookies and the session cookie that lets the
// dashboard's AuthGuard load /auth/me afterwards.
//
// Requires the dev stack (`just dev`). The backend's dev mode issues the
// fixed code 123456 to e2e-* emails and skips real email + Turnstile.
const FIXED_DEV_CODE = '123456'

function uniqueEmail(): string {
  // e2e- prefix → backend issues the fixed dev code; suffix keeps each run's
  // user unique so reruns don't collide on "already registered".
  return `e2e-${Date.now()}-${Math.floor(Math.random() * 1e6)}@example.com`
}

// The dashboard is served by the Astro dev server, which hydrates islands after
// first paint — an interaction can land before the Svelte handler is attached
// and silently no-op. Retry the action until the expected navigation happens
// (idempotent: the guard skips re-running once we've moved on). This is the
// real-app analogue of waiting for hydration, without a fixed sleep.
async function advance(page: Page, fromPath: RegExp, toPath: RegExp, act: () => Promise<void>) {
  await expect(async () => {
    if (fromPath.test(new URL(page.url()).pathname)) {
      await act()
    }
    await expect(page).toHaveURL(toPath, { timeout: 2000 })
  }).toPass({ timeout: 20_000 })
}

test('a new user can register, verify, set a password, and land on the dashboard', async ({
  page,
}) => {
  const email = uniqueEmail()

  // 1. Register (email-first). Turnstile auto-skips on localhost.
  await page.goto('/register')
  await advance(page, /\/register$/, /\/verify-email/, async () => {
    await page.getByLabel(/email/i).fill(email)
    await page.getByRole('button', { name: /continue/i }).click()
  })
  await expect(page.getByText(email)).toBeVisible()

  // 2. Enter the fixed dev code; a 6-digit entry auto-submits. New users (no
  //    password yet) are routed to set their password.
  await advance(page, /\/verify-email/, /\/set-password/, async () => {
    await page.getByLabel(/verification code/i).fill(FIXED_DEV_CODE)
  })

  // 3. Set a password to complete signup.
  await advance(page, /\/set-password/, /\/dashboard/, async () => {
    await page.getByLabel(/^new password|^password/i).first().fill('e2e-Password123')
    await page.getByLabel(/confirm/i).fill('e2e-Password123')
    await page.getByRole('button', { name: /create account/i }).click()
  })

  // 4. Completing signup logs the user in (real session cookie) and the
  //    dashboard's AuthGuard successfully loads the protected shell.
  await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
})
