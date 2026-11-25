import { expect, test, type Page } from '@playwright/test'
import { seedSession, TEST_USER } from './helpers/session'

// Another Member signed in from a different tab (OT-015): the server refuses the
// change, and the page comes back as the signed-in account with one message.
const OTHER = { ...TEST_USER, id: 'other-user', email: 'other@example.com', role: 'user' }
const NOTICE = "You're signed in as another account now. Nothing was saved."
const REFUSED = { status: 412, json: { code: 'account_changed', message: 'Another account' } }

async function mockApi(page: Page, account: { current: object; down?: boolean }) {
  const refused: { path: string; account: string | undefined }[] = []
  await page.route('**/api/v1/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace(/^\/api\/v1/, '')
    if (path === '/auth/me') {
      if (account.down) return route.fulfill({ status: 503, json: { code: 'x', message: 'down' } })
      return route.fulfill({ json: account.current })
    }
    if (path === '/auth/refresh') {
      return route.fulfill({ json: { expires_at: Math.floor(Date.now() / 1000) + 3600 } })
    }
    if (request.method() !== 'GET') {
      refused.push({ path, account: request.headers()['x-account-id'] })
      account.current = OTHER
      return route.fulfill(REFUSED)
    }
    if (path === '/lists') {
      const name = account.current === OTHER ? "Other's list" : 'Private list'
      return route.fulfill({
        json: [
          {
            id: '3f2b8c1e-9d4a-4f6b-8c2d-1e0a9b8c7d6e',
            name,
            version: 1,
            open_count: 0,
            todo_count: 0,
          },
        ],
      })
    }
    return route.fulfill({ json: { events: [], users: [], total: 0 } })
  })
  return refused
}

test('a list change for the previous account is refused and its data goes', async ({ page }) => {
  await seedSession(page, { cachedUser: TEST_USER })
  const refused = await mockApi(page, { current: TEST_USER })
  await page.goto('/dashboard/lists')
  await expect(page.getByText('Private list')).toBeVisible()
  await page.getByLabel('New list name').fill('Secret plan')
  await page.getByRole('button', { name: 'Create', exact: true }).click()

  await expect(page.getByText(NOTICE)).toHaveCount(1)
  await expect(page.getByText("Other's list")).toBeVisible()
  await expect(page.getByText('Private list')).toHaveCount(0)
  await expect(page.getByText('Secret plan')).toHaveCount(0)
  await expect(page.getByLabel('New list name')).toHaveValue('')
  expect(refused).toEqual([{ path: '/lists', account: TEST_USER.id }])
})

test('a Settings change shows the signed-in account, also when it cannot load', async ({
  page,
}) => {
  await seedSession(page, { cachedUser: TEST_USER })
  const account: { current: object; down?: boolean } = { current: TEST_USER }
  const refused = await mockApi(page, account)
  await page.goto('/dashboard/settings')
  await expect(page.getByText(TEST_USER.email).first()).toBeVisible()
  await page.getByLabel(/current password/i).fill('old-password-1')
  await page.getByLabel(/^new password/i).fill('new-password-1')
  account.down = true
  await page.getByRole('button', { name: /(change|update) password/i }).click()

  await expect(page.getByText(NOTICE)).toHaveCount(1)
  await expect(page.getByText(/Couldn't reach the server/)).toBeVisible()
  await expect(page.getByText(TEST_USER.email)).toHaveCount(0)
  account.down = false
  await page.getByRole('button', { name: 'Retry' }).first().click()
  await expect(page.getByText(OTHER.email).first()).toBeVisible()
  expect(refused).toHaveLength(1)
})

test('an Admin page is left when the signed-in account is not an Admin', async ({ page }) => {
  const admin = { ...TEST_USER, role: 'super_admin' }
  await seedSession(page, { cachedUser: admin })
  const account: { current: object } = { current: admin }
  await mockApi(page, account)
  await page.route('**/api/v1/admin/users**', async (route) => {
    if (route.request().method() !== 'GET') {
      account.current = OTHER
      return route.fulfill(REFUSED)
    }
    if (account.current === OTHER) {
      return route.fulfill({ status: 403, json: { code: 'access_denied', message: 'No' } })
    }
    return route.fulfill({
      json: {
        users: [
          {
            id: 'row-1',
            email: 'row@example.com',
            first_name: '',
            last_name: '',
            role: 'user',
            email_verified: true,
            created_at: '2026-01-01T00:00:00Z',
          },
        ],
        total: 1,
      },
    })
  })
  await page.goto('/dashboard/admin')
  await page.getByRole('combobox').first().selectOption('admin')
  await expect(page).toHaveURL(/\/dashboard\/?$/)
  await expect(page.getByText('row@example.com')).toHaveCount(0)
})
