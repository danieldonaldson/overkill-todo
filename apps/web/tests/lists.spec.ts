import { expect, test, type Page } from '@playwright/test'
import { seedSession, TEST_USER } from './helpers/session'

// Lists flows with the API mocked (OT-013 slice 1): create a list, open it at
// its own address, add and tick a todo, and one save that fails. The failed
// save keeps the title and says so; a later load shows the server's truth.
const LIST_A = '3f2b8c1e-9d4a-4f6b-8c2d-1e0a9b8c7d6e'
const LIST_B = '6c1e2d3f-4a5b-6c7d-8e9f-0a1b2c3d4e5f'

interface Todo {
  id: string
  title: string
  done: boolean
  version: number
  created_at: string
  completed_at: string | null
}
interface List {
  id: string
  name: string
  version: number
}
interface State {
  lists: List[]
  todos: Record<string, Todo[]>
  failAdds: number
  delay: Record<string, number>
  /** Hold each mark until this resolves. */
  holdMarks?: Promise<void>
  /** Answer marks with this status instead of saving. */
  failMarks?: number
}

function mockLists(page: Page, state: State) {
  const lists = () =>
    state.lists.map((list) => ({
      ...list,
      open_count: state.todos[list.id]?.filter((todo) => !todo.done).length ?? 0,
      todo_count: state.todos[list.id]?.length ?? 0,
    }))

  page.route('**/api/v1/**', async (route) => {
    const request = route.request()
    const path = new URL(request.url()).pathname.replace(/^\/api\/v1/, '')
    const method = request.method()
    if (path === '/auth/me') return route.fulfill({ json: TEST_USER })
    if (path === '/auth/refresh') {
      if (state.failMarks === 401) return route.fulfill({ status: 401, json: {} })
      return route.fulfill({ json: { expires_at: Math.floor(Date.now() / 1000) + 3600 } })
    }
    if (path === '/lists' && method === 'GET') return route.fulfill({ json: lists() })
    if (path === '/lists' && method === 'POST') {
      const body = request.postDataJSON() as { name: string }
      const list = { id: LIST_A, name: body.name, version: 1 }
      state.lists.push(list)
      state.todos[list.id] = []
      return route.fulfill({ json: { ...list, open_count: 0, todo_count: 0 } })
    }

    const detail = path.match(/^\/lists\/([^/]+)$/)
    if (detail && method === 'GET') {
      const id = detail[1]
      await new Promise((resolve) => setTimeout(resolve, state.delay[id] ?? 0))
      const list = lists().find((candidate) => candidate.id === id)
      if (!list) {
        return route.fulfill({
          status: 404,
          json: { code: 'list_not_found', message: 'Not found' },
        })
      }
      return route.fulfill({ json: { list, todos: state.todos[id] ?? [] } })
    }

    const add = path.match(/^\/lists\/([^/]+)\/todos$/)
    if (add && method === 'POST') {
      if (state.failAdds > 0) {
        state.failAdds -= 1
        return route.fulfill({
          status: 500,
          json: { code: 'internal_error', message: 'Something went wrong' },
        })
      }
      const body = request.postDataJSON() as { title: string }
      const todo: Todo = {
        id: '8a1c2d3e-4f5a-6b7c-8d9e-0f1a2b3c4d5e',
        title: body.title,
        done: false,
        version: 1,
        created_at: '2026-11-25T10:00:00Z',
        completed_at: null,
      }
      state.todos[add[1]]!.push(todo)
      return route.fulfill({ json: todo })
    }

    const mark = path.match(/^\/lists\/([^/]+)\/todos\/([^/]+)$/)
    if (mark && method === 'PATCH') {
      const body = request.postDataJSON() as { version: number; done: boolean }
      await state.holdMarks
      if (state.failMarks) {
        const status = state.failMarks
        return route.fulfill({ status, json: { code: 'failed', message: 'Failed' } })
      }
      const todo = state.todos[mark[1]]!.find((candidate) => candidate.id === mark[2])!
      todo.done = body.done
      todo.version += 1
      todo.completed_at = body.done ? '2026-11-25T10:05:00Z' : null
      return route.fulfill({ json: todo })
    }

    return route.fulfill({ status: 404, json: { code: 'not_found', message: 'Not found' } })
  })
}

function freshState(): State {
  return { lists: [], todos: { [LIST_A]: [] }, failAdds: 0, delay: {} }
}

test('create a list, add and tick a todo, and keep the title of a failed save', async ({
  page,
}) => {
  await seedSession(page)
  const state = freshState()
  mockLists(page, state)

  await page.goto('/dashboard/lists')
  await page.getByLabel('New list name').fill('Weekend')
  await page.getByRole('button', { name: 'Create', exact: true }).click()
  await expect(page).toHaveURL(new RegExp(`/dashboard/lists/${LIST_A}$`))
  await expect(page.getByRole('heading', { name: 'Weekend' })).toBeVisible()

  await page.getByLabel('New todo title').fill('Buy milk')
  await page.getByRole('button', { name: 'Add', exact: true }).click()
  const todos = page.getByRole('list', { name: 'Todos' })
  await expect(todos.getByText('Buy milk')).toBeVisible()
  await page.getByRole('checkbox', { name: 'Mark "Buy milk" done' }).click()
  await expect(page.getByRole('button', { name: /Completed \(1\)/ })).toBeVisible()
  await expect(todos.getByText('Buy milk')).toHaveCount(0)

  // A failed save: the title comes back to the input, and the optimistic row
  // goes when the refetch returns the server's truth.
  state.failAdds = 2
  await page.getByLabel('New todo title').fill('Write report')
  await page.getByRole('button', { name: 'Add', exact: true }).click()
  await expect(page.getByText(/Couldn't save your change/)).toBeVisible()
  await expect(page.getByLabel('New todo title')).toHaveValue('Write report')
  await expect(todos.getByText('Write report')).toHaveCount(0)

  // The address is still the list's own; a reload opens it again.
  await page.reload()
  await expect(page.getByRole('heading', { name: 'Weekend' })).toBeVisible()
  expect(new URL(page.url()).pathname).toBe(`/dashboard/lists/${LIST_A}`)
})

test('a slow answer for the previous list cannot replace the one now open', async ({ page }) => {
  await seedSession(page)
  const state = freshState()
  state.lists = [
    { id: LIST_A, name: 'Alpha', version: 1 },
    { id: LIST_B, name: 'Beta', version: 1 },
  ]
  state.todos = { [LIST_A]: [], [LIST_B]: [] }
  state.delay = { [LIST_A]: 500 }
  mockLists(page, state)

  await page.goto(`/dashboard/lists/${LIST_A}`)
  await expect(page.getByRole('button', { name: /^Beta/ })).toBeVisible()
  await page.getByRole('button', { name: /^Beta/ }).click()

  await expect(page.getByRole('heading', { name: 'Beta' })).toBeVisible()
  await page.waitForTimeout(800)
  await expect(page.getByRole('heading', { name: 'Beta' })).toBeVisible()
  expect(new URL(page.url()).pathname).toBe(`/dashboard/lists/${LIST_B}`)
})

function oneTodo(): State {
  const state = freshState()
  state.lists = [{ id: LIST_A, name: 'Alpha', version: 1 }]
  state.todos[LIST_A] = [
    {
      id: '8a1c2d3e-4f5a-6b7c-8d9e-0f1a2b3c4d5e',
      title: 'Buy milk',
      done: false,
      version: 1,
      created_at: '2026-11-25T10:00:00Z',
      completed_at: null,
    },
  ]
  return state
}

const tick = (page: Page) =>
  page.getByRole('checkbox', { name: 'Mark "Buy milk" done', exact: true }).click()

test('Lists opens from the navigation without rebuilding the header', async ({ page }) => {
  await seedSession(page)
  mockLists(page, oneTodo())
  await page.goto('/dashboard')
  const header = page.locator('header')
  await header.evaluate((element) => element.setAttribute('data-same', 'yes'))
  await page.getByRole('link', { name: 'Lists' }).click()
  await expect(page).toHaveURL(/\/dashboard\/lists\/?$/)
  await expect(page.getByRole('button', { name: /^Alpha/ })).toBeVisible()
  await expect(page.getByRole('link', { name: 'Lists' })).toHaveAttribute('aria-current', 'page')
  await expect(header).toHaveAttribute('data-same', 'yes')

  await page.getByRole('button', { name: /^Alpha/ }).click()
  await expect(page.getByText('Buy milk')).toBeVisible()
  await page.goBack()
  await expect(page.getByText('Select a list to see its todos.')).toBeVisible()
  await expect(header).toHaveAttribute('data-same', 'yes')
})

test('a save goes on after moving to Settings, and says so if it fails', async ({ page }) => {
  await seedSession(page)
  const state = oneTodo()
  const hold = Promise.withResolvers<void>()
  state.holdMarks = hold.promise
  mockLists(page, state)
  await page.goto(`/dashboard/lists/${LIST_A}`)
  await tick(page)
  await expect(page.getByRole('button', { name: /Completed \(1\)/ })).toBeVisible()

  let asked = false
  page.on('dialog', (dialog) => {
    asked = true
    void dialog.dismiss()
  })
  await page.getByRole('link', { name: 'Settings' }).click()
  await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
  expect(asked).toBe(false)

  state.failMarks = 400
  hold.resolve()
  await expect(page.getByText('Failed')).toBeVisible()
  await page.getByRole('link', { name: 'Lists' }).click()
  await page.getByRole('button', { name: /^Alpha/ }).click()
  await expect(page.getByRole('checkbox', { name: 'Mark "Buy milk" done' })).not.toBeChecked()
})

test('closing the tab asks only while a change is unsaved', async ({ page }) => {
  await seedSession(page)
  const state = oneTodo()
  const hold = Promise.withResolvers<void>()
  state.holdMarks = hold.promise
  mockLists(page, state)
  await page.goto(`/dashboard/lists/${LIST_A}`)
  await tick(page)
  await expect(page.getByRole('button', { name: /Completed \(1\)/ })).toBeVisible()

  const dialog = page.waitForEvent('dialog')
  await page.close({ runBeforeUnload: true })
  const asked = await dialog
  expect(asked.type()).toBe('beforeunload')
  await asked.dismiss()

  // Saved: leaving asks nothing.
  const saved = page.waitForResponse((response) => response.request().method() === 'PATCH')
  hold.resolve()
  await saved
  await expect
    .poll(() =>
      page.evaluate(() => {
        const event = new Event('beforeunload', { cancelable: true })
        window.dispatchEvent(event)
        return event.defaultPrevented
      })
    )
    .toBe(false)
})

test('a session that ends while saving goes to sign-in without asking', async ({ page }) => {
  await seedSession(page)
  const state = oneTodo()
  const hold = Promise.withResolvers<void>()
  state.holdMarks = hold.promise
  state.failMarks = 401
  mockLists(page, state)
  let asked = false
  page.on('dialog', (dialog) => {
    asked = true
    void dialog.accept()
  })
  await page.goto(`/dashboard/lists/${LIST_A}`)
  // A second change still waiting behind the first.
  await page.getByLabel('New todo title').fill('Waiting')
  await tick(page)
  await page.getByRole('button', { name: 'Add', exact: true }).click()
  hold.resolve()
  await expect(page).toHaveURL(/\/login\?session=expired/)
  expect(asked).toBe(false)
  expect(state.todos[LIST_A]![0]!.done).toBe(false)
  expect(state.todos[LIST_A]).toHaveLength(1)
})

test('a crashed list panel leaves the header and navigation working', async ({ page }) => {
  await seedSession(page)
  mockLists(page, oneTodo())
  await page.addInitScript(() => {
    ;(globalThis as { __otCrashIsland?: string }).__otCrashIsland = 'ListsPanel'
  })
  await page.goto(`/dashboard/lists/${LIST_A}`)
  await expect(page.getByRole('button', { name: 'Try again' })).toBeVisible()
  await page.getByRole('link', { name: 'Settings' }).click()
  await expect(page.getByRole('heading', { name: 'Settings', exact: true })).toBeVisible()
})

test('nothing scrolls sideways at 360 px', async ({ page }) => {
  await page.setViewportSize({ width: 360, height: 740 })
  await seedSession(page, { cachedUser: { ...TEST_USER, role: 'super_admin' } })
  const state = oneTodo()
  state.lists[0]!.name = 'x'.repeat(100)
  state.todos[LIST_A]![0]!.title = 'y'.repeat(500)
  mockLists(page, state)
  const overflow = () =>
    page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth)
  await page.goto('/dashboard/lists')
  await expect(page.getByText('Select a list to see its todos.')).toBeVisible()
  expect(await overflow()).toBeLessThanOrEqual(0)
  await page.goto(`/dashboard/lists/${LIST_A}`)
  await expect(page.getByRole('checkbox')).toBeVisible()
  expect(await overflow()).toBeLessThanOrEqual(0)
})
