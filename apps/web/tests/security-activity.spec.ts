import { expect, test, type Page } from '@playwright/test'
import { seedSession, TEST_USER } from './helpers/session'

// OT-008 slice 3, with the API mocked: a Member changes their password and
// sees it at the top of Security activity on Settings.

const MEMBER = { ...TEST_USER, role: 'user' }

async function mockBackend(page: Page) {
  const events: object[] = [
    {
      id: 1,
      occurred_at: '2026-11-20T09:00:00Z',
      kind: 'role_changed',
      actor: 'administrator',
      details: { from_role: 'user', to_role: 'admin' },
    },
  ]
  await page.route('**/api/v1/auth/me', (route) => route.fulfill({ json: MEMBER }))
  await page.route('**/api/v1/users/password/change', async (route) => {
    events.unshift({
      id: events.length + 1,
      occurred_at: '2026-11-21T10:00:00Z',
      kind: 'password_changed',
      actor: 'you',
      details: {},
    })
    await route.fulfill({ status: 204 })
  })
  await page.route('**/api/v1/users/me/security-activity*', (route) =>
    route.fulfill({ json: { events } })
  )
}

test.describe('security activity', () => {
  test.beforeEach(async ({ page }) => {
    await seedSession(page, { cachedUser: MEMBER })
    await mockBackend(page)
  })

  test('a Member changes their password and sees it at the top', async ({ page }) => {
    await page.goto('/dashboard/settings')
    const activity = page.getByTestId('security-activity')
    await expect(activity.getByText('Role changed from Member to Admin')).toBeVisible()
    await expect(activity.getByText(/By an administrator/)).toBeVisible()

    await page.getByLabel(/current password/i).fill('oldpassword123')
    await page.getByLabel(/^new password/i).fill('newpassword123')
    await page.getByRole('button', { name: /change password/i }).click()

    const first = activity.getByTestId('activity-event').first()
    await expect(first).toContainText('Password changed')
    await expect(first).toContainText('By you')
  })

  test('is readable at 360 px without scrolling the page sideways', async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 740 })
    await page.goto('/dashboard/settings')
    const activity = page.getByTestId('security-activity')
    await activity.scrollIntoViewIfNeeded()
    await expect(activity.getByText('Role changed from Member to Admin')).toBeInViewport()
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth
    )
    expect(overflow).toBeLessThanOrEqual(0)
  })
})
