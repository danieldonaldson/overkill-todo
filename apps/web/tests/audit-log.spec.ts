import { expect, test, type Page } from '@playwright/test'
import { seedSession, TEST_USER } from './helpers/session'

// OT-008, with the API mocked: a Super Admin changes a role on the Admin page,
// then finds that change in the audit log.

const SUPER_ADMIN = {
  ...TEST_USER,
  id: '0190a0b0-0000-7000-8000-00000000000a',
  role: 'super_admin',
}
const ALICE = {
  id: '0190a0b0-0000-7000-8000-000000000001',
  email: 'alice.with.a.rather.long.address@example.com',
  role: 'user',
  email_verified: true,
  created_at: '2026-01-15T12:00:00Z',
}

/** Mocks the backend: the audit log shows a role change once one was made. */
async function mockBackend(page: Page) {
  const events: object[] = []
  await page.route('**/api/v1/auth/me', (route) => route.fulfill({ json: SUPER_ADMIN }))
  await page.route('**/api/v1/admin/users?*', (route) =>
    route.fulfill({ json: { users: [ALICE], total: 1, limit: 50, offset: 0 } })
  )
  await page.route('**/api/v1/admin/users/*/role', async (route) => {
    const { role } = route.request().postDataJSON() as { role: string }
    events.unshift({
      id: events.length + 1,
      occurred_at: '2026-11-20T10:00:00Z',
      kind: 'role_changed',
      subject: { id: ALICE.id, email: ALICE.email, deleted: false },
      actor: {
        kind: 'user',
        account: { id: SUPER_ADMIN.id, email: SUPER_ADMIN.email, deleted: false },
      },
      details: { from_role: ALICE.role, to_role: role },
    })
    await route.fulfill({ json: { ...ALICE, role } })
  })
  await page.route('**/api/v1/admin/audit-events*', (route) => {
    const account = new URL(route.request().url()).searchParams.get('account_id')
    const matching = events.filter(
      (e) => !account || (e as { subject: { id: string } }).subject.id === account
    )
    return route.fulfill({
      json: { events: matching, total: matching.length, limit: 50, offset: 0 },
    })
  })
}

test.describe('audit log', () => {
  test.beforeEach(async ({ page }) => {
    await seedSession(page, { cachedUser: SUPER_ADMIN })
    await mockBackend(page)
  })

  test('a Super Admin changes a role, then sees it in the audit log', async ({ page }) => {
    await page.goto('/dashboard/admin')
    const row = page.getByRole('row').filter({ hasText: ALICE.email })
    await row.getByRole('combobox').selectOption('admin')
    await expect(page.getByText(`Updated ${ALICE.email} to admin.`)).toBeVisible()

    await page.getByRole('link', { name: 'Audit log', exact: true }).click()

    await expect(page).toHaveURL('/dashboard/audit')
    await expect(page.getByRole('heading', { name: 'Audit log' })).toBeVisible()
    const event = page.getByTestId('audit-event').first()
    await expect(event).toContainText('Role changed')
    await expect(event).toContainText(ALICE.email)
    await expect(event).toContainText(SUPER_ADMIN.email)
    await expect(event).toContainText('Member → Admin')
    await expect(page.getByText('Showing 1–1 of 1')).toBeVisible()
  })

  test("the user list links to that account's events", async ({ page }) => {
    await page.goto('/dashboard/admin')
    await page
      .getByRole('row')
      .filter({ hasText: ALICE.email })
      .getByRole('combobox')
      .selectOption('admin')
    await expect(page.getByText(`Updated ${ALICE.email} to admin.`)).toBeVisible()

    await page.getByRole('link', { name: `Audit log for ${ALICE.email}` }).click()

    await expect(page).toHaveURL(`/dashboard/audit?account=${ALICE.id}`)
    await expect(page.getByLabel('Account ID')).toHaveValue(ALICE.id)
    await expect(page.getByTestId('audit-event')).toHaveCount(1)
  })

  test('is readable at 360 px without scrolling the page sideways', async ({ page }) => {
    await page.setViewportSize({ width: 360, height: 740 })
    await page.goto('/dashboard/admin')
    await page
      .getByRole('row')
      .filter({ hasText: ALICE.email })
      .getByRole('combobox')
      .selectOption('admin')
    await expect(page.getByText(`Updated ${ALICE.email} to admin.`)).toBeVisible()

    await page.goto('/dashboard/audit')
    const event = page.getByTestId('audit-event').first()
    await expect(event).toContainText('Member → Admin')
    for (const text of ['Role changed', ALICE.email, SUPER_ADMIN.email, 'Member → Admin']) {
      await expect(event.getByText(text, { exact: true })).toBeInViewport()
    }
    const overflow = await page.evaluate(
      () => document.documentElement.scrollWidth - document.documentElement.clientWidth
    )
    expect(overflow).toBeLessThanOrEqual(0)
  })
})
