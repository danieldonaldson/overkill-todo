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

test('a save held while another Member signs in is refused', async ({ page }) => {
  await register(page)
  await page.goto('/dashboard/lists')
  await expect(page.getByText('No lists yet.', { exact: false })).toBeVisible()

  // Hold the first attempt before the server sees it.
  const held = Promise.withResolvers<void>()
  const release = Promise.withResolvers<void>()
  let first = true
  await page.route('**/api/v1/lists', async (route) => {
    if (route.request().method() !== 'POST' || !first) return route.continue()
    first = false
    held.resolve()
    await release.promise
    // As when the session ran out meanwhile: the client refreshes and resends,
    // now with the other Member's cookies.
    await route.fulfill({ status: 401, json: { code: 'token_expired', message: 'Expired' } })
  })
  await page.getByLabel('New list name').fill('Only mine')
  await page.getByRole('button', { name: 'Create', exact: true }).click()
  await held.promise

  // Member B signs up from a second tab of the same browser.
  const second = await page.context().newPage()
  await second.goto('/dashboard/settings')
  await second.getByRole('button', { name: /log out/i }).click()
  await expect(second).toHaveURL(/\/login/)
  await register(second)

  const refused = page.waitForResponse((response) => response.status() === 412)
  release.resolve()
  await refused
  await expect(page.getByText(/signed in as another account now/)).toBeVisible()
  await expect(page.getByText('Only mine')).toHaveCount(0)
  await second.goto('/dashboard/lists')
  await expect(second.getByText('No lists yet.', { exact: false })).toBeVisible()

  // The second tab showed A until B signed up in it: B's own change is accepted.
  await second.getByLabel('New list name').fill("B's list")
  await second.getByRole('button', { name: 'Create', exact: true }).click()
  await expect(second.getByRole('heading', { name: "B's list" })).toBeVisible()
})

// --- Rename and delete conflicts (OT-013 slice 3) ---------------------------

async function newList(page: Page, name: string): Promise<string> {
  await page.goto('/dashboard/lists')
  await page.getByLabel('New list name').fill(name)
  await page.getByRole('button', { name: 'Create', exact: true }).click()
  await expect(page.getByRole('heading', { name })).toBeVisible()
  return page.url()
}

async function addTodo(page: Page, title: string) {
  await page.getByLabel('New todo title').fill(title)
  await page.getByRole('button', { name: 'Add', exact: true }).click()
  await expect(page.getByRole('list', { name: 'Todos' }).getByText(title)).toBeVisible()
}

function todoRow(page: Page, title: string) {
  return page.getByRole('listitem').filter({ hasText: title }).first()
}

async function renameTodo(page: Page, from: string, to: string) {
  await todoRow(page, from).getByRole('button', { name: 'Rename' }).click()
  await page.getByLabel('Todo title', { exact: true }).fill(to)
  await page.getByRole('button', { name: 'Save' }).click()
}

async function deleteTodo(page: Page, title: string) {
  await todoRow(page, title).getByRole('button', { name: 'Delete' }).click()
}

async function renameList(page: Page, to: string) {
  await page.getByRole('button', { name: 'Rename' }).first().click()
  await page.getByLabel('List name', { exact: true }).fill(to)
  await page.getByRole('button', { name: 'Save' }).click()
}

async function deleteList(page: Page) {
  await page.getByRole('button', { name: 'Delete list' }).click()
  await page.getByRole('dialog').getByRole('button', { name: 'Delete list' }).click()
  await expect(page).toHaveURL(/\/dashboard\/lists$/)
}

test('two tabs rename the same todo; the first name wins and the text stays', async ({ page }) => {
  await register(page)
  const address = await newList(page, 'Shared list')
  await addTodo(page, 'Same todo')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    const secondTodos = second.getByRole('list', { name: 'Todos' })
    await expect(secondTodos.getByText('Same todo')).toBeVisible()

    await renameTodo(page, 'Same todo', 'First name')
    await expect(page.getByRole('list', { name: 'Todos' }).getByText('First name')).toBeVisible()

    const stale = second.waitForResponse(
      (response) =>
        response.request().method() === 'PATCH' &&
        response.url().includes('/todos/') &&
        response.status() === 409
    )
    await renameTodo(second, 'Same todo', 'Second name')
    await stale

    await expect(secondTodos.getByText('First name')).toBeVisible()
    await expect(second.getByText(/changed elsewhere/)).toBeVisible()
    await expect(
      second.getByRole('list', { name: 'Not saved' }).getByText('Second name')
    ).toBeVisible()
  } finally {
    await second.close()
  }

  await page.reload()
  await expect(page.getByRole('list', { name: 'Todos' }).getByText('First name')).toBeVisible()
})

test('a rename and a tick in two tabs keep the new title and leave the todo open', async ({
  page,
}) => {
  await register(page)
  const address = await newList(page, 'Mixed list')
  await addTodo(page, 'Same todo')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByText('Same todo')).toBeVisible()

    await renameTodo(page, 'Same todo', 'Renamed')
    const stale = second.waitForResponse((response) => response.status() === 409)
    await second.getByRole('checkbox', { name: 'Mark "Same todo" done' }).click()
    await stale

    await expect(second.getByRole('checkbox', { name: 'Mark "Renamed" done' })).toBeVisible()
    await expect(second.getByText(/changed elsewhere/)).toBeVisible()
    await expect(second.getByRole('button', { name: /Completed/ })).toHaveCount(0)
  } finally {
    await second.close()
  }
})

test('deleting a todo renamed elsewhere keeps it with the new title', async ({ page }) => {
  await register(page)
  const address = await newList(page, 'Kept list')
  await addTodo(page, 'Same todo')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByText('Same todo')).toBeVisible()

    await renameTodo(page, 'Same todo', 'Renamed')
    const stale = second.waitForResponse((response) => response.status() === 409)
    await deleteTodo(second, 'Same todo')
    await stale

    const todos = second.getByRole('list', { name: 'Todos' })
    await expect(todos.getByText('Renamed')).toBeVisible()
    await expect(second.getByText(/changed elsewhere/)).toBeVisible()
  } finally {
    await second.close()
  }
  await page.reload()
  await expect(page.getByRole('list', { name: 'Todos' }).getByText('Renamed')).toBeVisible()
})

test('different todos renamed in two tabs both save', async ({ page }) => {
  await register(page)
  const address = await newList(page, 'Two todos')
  await addTodo(page, 'Alpha')
  await addTodo(page, 'Beta')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByText('Alpha')).toBeVisible()

    await renameTodo(page, 'Alpha', 'Alpha one')
    await renameTodo(second, 'Beta', 'Beta one')

    // Tabs aren't live: each loads the other tab's rename to check both saved.
    await page.reload()
    await second.reload()
    for (const tab of [page, second]) {
      const todos = tab.getByRole('list', { name: 'Todos' })
      await expect(todos.getByText('Alpha one')).toBeVisible()
      await expect(todos.getByText('Beta one')).toBeVisible()
      await expect(tab.getByText(/changed elsewhere/)).toHaveCount(0)
    }
  } finally {
    await second.close()
  }
})

test('two tabs rename the same list; the first name wins', async ({ page }) => {
  await register(page)
  const address = await newList(page, 'First list')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByRole('heading', { name: 'First list' })).toBeVisible()

    await renameList(page, 'Kept name')
    await expect(page.getByRole('heading', { name: 'Kept name' })).toBeVisible()

    const stale = second.waitForResponse(
      (response) => response.request().method() === 'PATCH' && response.status() === 409
    )
    await renameList(second, 'Other name')
    await stale

    await expect(second.getByRole('heading', { name: 'Kept name' })).toBeVisible()
    await expect(second.getByText(/list changed elsewhere/)).toBeVisible()
  } finally {
    await second.close()
  }
})

test('a todo deleted in both tabs ends gone with no error', async ({ page }) => {
  await register(page)
  const address = await newList(page, 'Both delete')
  await addTodo(page, 'Same todo')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByText('Same todo')).toBeVisible()

    await deleteTodo(page, 'Same todo')
    await expect(page.getByText('No todos yet. Add your first one.')).toBeVisible()

    await deleteTodo(second, 'Same todo')
    await expect(second.getByText('No todos yet. Add your first one.')).toBeVisible()
    await expect(second.getByText(/deleted elsewhere|Couldn't save/)).toHaveCount(0)
  } finally {
    await second.close()
  }
})

test('marking a todo deleted elsewhere removes it with a note', async ({ page }) => {
  await register(page)
  const address = await newList(page, 'Mark gone')
  await addTodo(page, 'Same todo')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByText('Same todo')).toBeVisible()

    await deleteTodo(page, 'Same todo')
    await expect(page.getByText('No todos yet. Add your first one.')).toBeVisible()

    const gone = second.waitForResponse(
      (response) => response.request().method() === 'PATCH' && response.status() === 404
    )
    await second.getByRole('checkbox', { name: 'Mark "Same todo" done' }).click()
    await gone

    await expect(second.getByText('No todos yet. Add your first one.')).toBeVisible()
    await expect(second.getByText(/todo was deleted elsewhere/)).toBeVisible()
  } finally {
    await second.close()
  }
})

test('a list deleted elsewhere shows list not found and leaves the picker', async ({ page }) => {
  await register(page)
  const address = await newList(page, 'Shared list')
  await addTodo(page, 'Same todo')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByText('Same todo')).toBeVisible()

    await deleteList(page)
    await expect(page.getByText('No lists yet.', { exact: false })).toBeVisible()

    const gone = second.waitForResponse(
      (response) => response.url().includes('/todos') && response.status() === 404
    )
    await second.getByLabel('New todo title').fill('After delete')
    await second.getByRole('button', { name: 'Add', exact: true }).click()
    await gone

    await expect(second.getByText('List not found')).toBeVisible()
    await expect(second.getByText(/list was deleted elsewhere/)).toBeVisible()
    await expect(second.getByRole('button', { name: /Shared list/ })).toHaveCount(0)
  } finally {
    await second.close()
  }
})

test('a todo added before confirming a list deletion asks again with the new count', async ({
  page,
}) => {
  await register(page)
  const address = await newList(page, 'Confirm twice')
  await addTodo(page, 'First todo')
  const second = await page.context().newPage()
  try {
    await second.goto(address)
    await expect(second.getByText('First todo')).toBeVisible()

    await page.getByRole('button', { name: 'Delete list' }).click()
    const dialog = page.getByRole('dialog')
    await expect(dialog.getByText(/It holds\s+1\s+todo/)).toBeVisible()

    await addTodo(second, 'Second todo')
    await dialog.getByRole('button', { name: 'Delete list' }).click()

    await expect(dialog.getByText(/It holds\s+2\s+todos/)).toBeVisible()
    await expect(dialog.getByText(/check the name and number of todos/i)).toBeVisible()
    await expect(page.getByRole('heading', { name: 'Confirm twice', exact: true })).toBeVisible()

    await dialog.getByRole('button', { name: 'Cancel' }).click()
    await page.reload()
    await expect(page.getByText('First todo')).toBeVisible()
    await expect(page.getByText('Second todo')).toBeVisible()
  } finally {
    await second.close()
  }
})
