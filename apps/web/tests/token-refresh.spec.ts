import { expect, test } from '@playwright/test'

const TEST_USER = {
  id: 'test-user',
  email: 'test@example.com',
  first_name: '',
  last_name: '',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
}

// A 401 on an authenticated request must trigger a single silent token
// refresh and a transparent retry — the user never sees the failure and is not
// bounced to /login. This exercises the client's `coordinateRefresh` 401 path
// end-to-end in the browser, which previously had no E2E coverage.
test.describe('silent token refresh', () => {
  test('a 401 is recovered by one silent refresh + retry, no redirect', async ({ page }) => {
    let refreshCount = 0
    await page.route('**/api/v1/auth/refresh', async (route) => {
      refreshCount += 1
      await route.fulfill({
        json: { expires_at: Math.floor(Date.now() / 1000) + 3600 },
      })
    })

    // The first GET /auth/me returns 401 (expired access token); after the
    // refresh, the retry and every later call succeed.
    let meCount = 0
    await page.route('**/api/v1/auth/me', async (route) => {
      meCount += 1
      if (meCount === 1) {
        await route.fulfill({ status: 401, json: { message: 'token expired' } })
        return
      }
      await route.fulfill({ json: TEST_USER })
    })

    await page.goto('/dashboard')

    // The protected shell still renders — the 401 was recovered transparently.
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()

    // The original request is retried after exactly one silent refresh.
    await expect.poll(() => meCount).toBeGreaterThanOrEqual(2)
    expect(refreshCount).toBe(1)

    // The user was never bounced to /login.
    await expect(page).toHaveURL(/\/dashboard$/)
  })

  test('a failed refresh after a 401 redirects to the login page', async ({ page }) => {
    // The refresh cookie is also dead → refresh fails, session is truly expired.
    await page.route('**/api/v1/auth/refresh', async (route) => {
      await route.fulfill({ status: 401, json: { message: 'refresh expired' } })
    })
    await page.route('**/api/v1/auth/me', async (route) => {
      await route.fulfill({ status: 401, json: { message: 'token expired' } })
    })

    await page.goto('/dashboard')

    // A truly-expired session lands on /login with the session=expired marker.
    await expect(page).toHaveURL(/\/login/)
  })
})
