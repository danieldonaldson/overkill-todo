import { expect, test } from '@playwright/test'

test.describe('dashboard dark mode', () => {
  test('saved dark preference is applied before paint', async ({ page }) => {
    // Not '/': that route is now a meta-refresh redirect, which would navigate
    // out from under the localStorage write below.
    await page.goto('/login')
    await page.evaluate(() => localStorage.setItem('mode-watcher-mode', 'dark'))
    await page.reload()

    const htmlClass = await page.evaluate(() => document.documentElement.className)
    expect(htmlClass).toContain('dark')
  })

  test('theme toggle exists in dashboard chrome', async ({ page }) => {
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

    await page.goto('/dashboard')

    await expect(page.getByRole('button', { name: /toggle theme/i })).toBeVisible()
  })

  test('theme toggle switches to dark mode and persists the preference', async ({ page }) => {
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

    await page.goto('/dashboard')
    await page.evaluate(() => localStorage.setItem('mode-watcher-mode', 'light'))
    await page.reload()

    const toggle = page.getByRole('button', { name: /toggle theme/i })
    await toggle.click()

    await expect(page.locator('html')).toHaveClass(/dark/)
    await expect
      .poll(() => page.evaluate(() => localStorage.getItem('mode-watcher-mode')))
      .toBe('dark')
  })
})
