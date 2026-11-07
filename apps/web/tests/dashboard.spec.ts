import { expect, test } from '@playwright/test'

test.describe('dashboard static shells', () => {
  test.beforeEach(async ({ page }) => {
    await page.route('**/api/v1/auth/me', async (route) => {
      await route.fulfill({
        json: {
          id: 'test-user',
          email: 'test@example.com',
          first_name: '',
          last_name: '',
          email_verified: true,
          has_password: true,
          oauth_providers: [],
        },
      })
    })
  })

  test('dashboard route serves protected shell', async ({ page }) => {
    await page.goto('/dashboard')

    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await expect(page.getByRole('navigation', { name: /dashboard/i })).toBeVisible()
  })

  test('dashboard navigation does not reload the document', async ({ page }) => {
    await page.goto('/dashboard')

    await page.evaluate(() => {
      ;(window as Window & { __dashboardNavigationMarker?: string }).__dashboardNavigationMarker =
        'preserved'
    })

    await page.getByRole('link', { name: 'Settings' }).click()

    await expect(page).toHaveURL('/dashboard/settings')
    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect
      .poll(() =>
        page.evaluate(
          () =>
            (window as Window & { __dashboardNavigationMarker?: string })
              .__dashboardNavigationMarker
        )
      )
      .toBe('preserved')
  })

  test('settings route serves protected shell', async ({ page }) => {
    await page.goto('/dashboard/settings')

    await expect(page.getByRole('heading', { name: 'Settings' })).toBeVisible()
    await expect(page.getByText('Manage your dashboard account.')).toBeVisible()
  })

})
