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
