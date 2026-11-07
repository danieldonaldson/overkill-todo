import { expect, test } from '@playwright/test'

test.describe('auth pages', () => {
  test('the site root sends visitors to sign in', async ({ page }) => {
    await page.goto('/')

    await page.waitForURL('**/login')
    await expect(page.getByRole('heading', { name: 'Welcome back' })).toBeVisible()
  })

  test('login page renders form and oauth', async ({ page }) => {
    await page.goto('/login')

    await expect(page.getByRole('heading', { name: 'Welcome back' })).toBeVisible()
    await expect(page.getByLabel(/email/i)).toBeVisible()
    await expect(page.getByRole('textbox', { name: /password/i })).toBeVisible()
    await expect(page.getByRole('button', { name: /google/i })).toBeVisible()
  })

  test('register page renders email-first signup', async ({ page }) => {
    await page.goto('/register')

    await expect(page.getByRole('heading', { name: 'Create your account' })).toBeVisible()
    await expect(page.getByLabel(/email/i)).toBeVisible()
    await expect(page.getByRole('button', { name: /continue/i })).toBeVisible()
    await expect(page.locator('input[name="password"]')).toHaveCount(0)
  })

  test('tabs swap the panel and the URL without a page load', async ({ page }) => {
    await page.goto('/login')

    await page.getByRole('tab', { name: 'Create account' }).click()

    await expect(page.getByRole('heading', { name: 'Create your account' })).toBeVisible()
    await expect(page).toHaveURL(/\/register$/)
    await expect(page.locator('input[name="password"]')).toHaveCount(0)

    await page.getByRole('tab', { name: 'Sign in' }).click()

    await expect(page.getByRole('heading', { name: 'Welcome back' })).toBeVisible()
    await expect(page).toHaveURL(/\/login$/)
  })

  test('forgot password links back to login', async ({ page }) => {
    await page.goto('/forgot-password')

    await expect(page.getByRole('button', { name: /send reset code/i })).toBeVisible()
    await expect(page.getByRole('link', { name: /back to sign in/i })).toHaveAttribute('href', '/login')
  })
})
