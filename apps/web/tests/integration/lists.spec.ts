import { expect, test, type Page } from '@playwright/test'

// Two browser tabs against the real backend (OT-013 slice 1): both open the
// same list at its shared address, both tick the same todo, and the second
// save — based on the version it loaded — loses. The server's current version
// is what both tabs end with.
const FIXED_DEV_CODE = '123456'

function uniqueEmail(): string {
  return `e2e-lists-${Date.now()}-${Math.floor(Math.random() * 1e6)}@example.com`
}

async function advance(page: Page, fromPath: RegExp, toPath: RegExp, act: () => Promise<void>) {
  await expect(async () => {
    if (fromPath.test(new URL(page.url()).pathname)) {
      await act()
    }
    await expect(page).toHaveURL(toPath, { timeout: 2000 })
  }).toPass({ timeout: 20_000 })
}

async function register(page: Page) {
  const email = uniqueEmail()
  await page.goto('/register')
  await advance(page, /\/register$/, /\/verify-email/, async () => {
    await page.getByLabel(/email/i).fill(email)
    await page.getByRole('button', { name: /continue/i }).click()
  })
  await advance(page, /\/verify-email/, /\/set-password/, async () => {
    await page.getByLabel(/verification code/i).fill(FIXED_DEV_CODE)
  })
  await advance(page, /\/set-password/, /\/dashboard/, async () => {
    await page
      .getByLabel(/^new password|^password/i)
      .first()
      .fill('e2e-Password123')
    await page.getByLabel(/confirm/i).fill('e2e-Password123')
    await page.getByRole('button', { name: /create account/i }).click()
  })
  await expect(page.getByRole('heading', { name: 'Home' })).toBeVisible()
}

test('two tabs tick the same todo; the stale answer loses and the current version wins', async ({
  page,
}) => {
  await register(page)

  await page.goto('/dashboard/lists')
  await page.getByLabel('New list name').fill('Shared list')
  await page.getByRole('button', { name: 'Create', exact: true }).click()
  await expect(page).toHaveURL(/\/dashboard\/lists\/[0-9a-f-]{36}$/)
  await expect(page.getByRole('heading', { name: 'Shared list' })).toBeVisible()
  const address = page.url()

  await page.getByLabel('New todo title').fill('Same todo')
  await page.getByRole('button', { name: 'Add', exact: true }).click()
  const openTodos = page.getByRole('list', { name: 'Todos' })
  await expect(openTodos.getByText('Same todo')).toBeVisible()

  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByRole('heading', { name: 'Shared list' })).toBeVisible()
    await expect(second.getByRole('checkbox', { name: 'Mark "Same todo" done' })).toBeVisible()

    // First tab ticks it and unticks it: the server is now at version 3 while
    // the second tab still holds version 1.
    await page.getByRole('checkbox', { name: 'Mark "Same todo" done' }).click()
    await expect(page.getByRole('button', { name: /Completed \(1\)/ })).toBeVisible()
    await page.getByRole('button', { name: /Completed \(1\)/ }).click()
    await page.getByRole('checkbox', { name: 'Mark "Same todo" not done' }).click()
    await expect(page.getByRole('checkbox', { name: 'Mark "Same todo" done' })).toBeVisible()

    // The second tab's tick is based on version 1 and asks for a state the
    // server no longer has: the save loses with a stale conflict.
    const stale = second.waitForResponse(
      (response) =>
        response.request().method() === 'PATCH' &&
        response.url().includes('/todos/') &&
        response.status() === 409
    )
    await second.getByRole('checkbox', { name: 'Mark "Same todo" done' }).click()
    await stale
    await expect(second.getByRole('checkbox', { name: 'Mark "Same todo" done' })).toBeVisible()
  } finally {
    await second.close()
  }
})

test('an add whose answer is lost is saved once', async ({ page }) => {
  await register(page)
  await page.goto('/dashboard/lists')
  await page.getByLabel('New list name').fill('Lost answer')
  await page.getByRole('button', { name: 'Create', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Lost answer' })).toBeVisible()

  // The backend saves the first attempt; the browser never hears about it.
  let sent = 0
  await page.route('**/api/v1/lists/*/todos', async (route) => {
    sent += 1
    if (sent > 1) return route.continue()
    await route.fetch()
    await route.abort('connectionreset')
  })
  await page.getByLabel('New todo title').fill('Only once')
  await page.getByRole('button', { name: 'Add', exact: true }).click()
  await expect.poll(() => sent).toBe(2)

  await page.reload()
  await expect(page.getByRole('list', { name: 'Todos' }).getByText('Only once')).toHaveCount(1)
})
