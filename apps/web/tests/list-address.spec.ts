import { expect, test, type Page } from '@playwright/test'
import { seedSession, TEST_USER } from './helpers/session'

// A list's own address (OT-013 D8): the shell is served for every
// /dashboard/lists/<id>, the island reads the id, and the address survives a
// reload and in-site moves. A malformed id is the same shell, no request. An
// unauthenticated visitor never sees dashboard content.
const ID = '3f2b8c1e-9d4a-4f6b-8c2d-1e0a9b8c7d6e'
const list = { id: ID, name: 'Weekend', open_count: 1, todo_count: 1 }
const todo = {
  id: '8a1c2d3e-4f5a-6b7c-8d9e-0f1a2b3c4d5e',
  title: 'Buy milk',
  done: false,
  version: 1,
  created_at: '2026-11-25T10:00:00Z',
  completed_at: null,
}

async function mockApi(page: Page) {
  const detailRequests: string[] = []
  await page.route('**/api/v1/auth/me', (route) => route.fulfill({ json: TEST_USER }))
  await page.route('**/api/v1/lists', (route) => route.fulfill({ json: [list] }))
  await page.route('**/api/v1/lists/*', (route) => {
    if (route.request().method() !== 'GET') return route.fallback()
    detailRequests.push(new URL(route.request().url()).pathname)
    return route.fulfill({ json: { list, todos: [todo] } })
  })
  return { detailRequests }
}

test.describe('a list address', () => {
  test('survives a reload and an in-site move away and back', async ({ page }) => {
    await seedSession(page)
    await mockApi(page)

    await page.goto(`/dashboard/lists/${ID}`)
    await expect(page.getByRole('heading', { name: 'Weekend' })).toBeVisible()

    await page.reload()
    await expect(page.getByRole('heading', { name: 'Weekend' })).toBeVisible()
    expect(new URL(page.url()).pathname).toBe(`/dashboard/lists/${ID}`)

    await page.getByRole('link', { name: 'Home' }).click()
    await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
    await page.goBack()
    await expect(page.getByRole('heading', { name: 'Weekend' })).toBeVisible()
    expect(new URL(page.url()).pathname).toBe(`/dashboard/lists/${ID}`)
  })

  test('with a malformed id shows the shell and asks for nothing', async ({ page }) => {
    await seedSession(page)
    const { detailRequests } = await mockApi(page)

    await page.goto('/dashboard/lists/not-a-uuid')
    await expect(page.getByRole('heading', { name: 'List not found' })).toBeVisible()
    expect(detailRequests).toEqual([])
  })

  test('a visitor is sent to sign in before dashboard content', async ({ page }) => {
    await mockApi(page)

    await page.goto(`/dashboard/lists/${ID}`)
    await page.waitForURL('**/login')
    await expect(page.getByRole('heading', { name: 'Lists' })).toHaveCount(0)
    await expect(page.getByRole('heading', { name: 'Weekend' })).toHaveCount(0)
  })
})
