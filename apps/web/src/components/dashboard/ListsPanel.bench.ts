// A full list (OT-013 slice 5): opening a list of 1,000 todos, and each change
// a Member makes to it, from the store call until the panel shows it. The API
// is the in-memory server the client tests use, so versions and answers behave
// as the real one's. Run with `just bench-web`; tests/lists.spec.ts holds the
// 200 ms budget in a real browser on the CI runner.
import { test, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import { tick } from 'svelte'
import ListsPanel from './ListsPanel.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    lists: {
      list: vi.fn(),
      create: vi.fn(),
      rename: vi.fn(),
      deleteList: vi.fn(),
      get: vi.fn(),
      addTodo: vi.fn(),
      markTodo: vi.fn(),
      renameTodo: vi.fn(),
      deleteTodo: vi.fn(),
      moveTodo: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
  navigateReplace: vi.fn(),
  redirect: vi.fn(),
}))

import { api } from '../../lib/api'
import { lists } from '../../lib/lists/store.svelte'
import type { TodoView } from '../../lib/lists/engine'
import { MemoryListsServer } from '../../lib/lists/memory-server'

const COUNT = 1000
// The panel opens only a UUID address; the server names its lists `list-<n>`.
const LIST_ID = '3f2b8c1e-9d4a-4f6b-8c2d-1e0a9b8c7d6e'

const server = new MemoryListsServer()
const serverId = server.seededList(
  'Full',
  Array.from({ length: COUNT }, (_, index) => `Todo ${index + 1}`)
)
const inServer = (id: string) => (id === LIST_ID ? serverId : id)
const outward = <T extends { id: string }>(list: T): T =>
  list.id === serverId ? { ...list, id: LIST_ID } : list

// Configured at module scope: Vitest bench mode skips beforeAll hooks.
vi.mocked(api.lists.list).mockImplementation(async () => server.list().map(outward))
vi.mocked(api.lists.get).mockImplementation(async (id) => {
  const detail = server.getList(inServer(id))
  return { ...detail, list: outward(detail.list) }
})
vi.mocked(api.lists.addTodo).mockImplementation(async (id, { key, title }) =>
  server.addTodo(inServer(id), key, title)
)
vi.mocked(api.lists.markTodo).mockImplementation(async (id, todoId, { version, done }) =>
  server.markTodo(inServer(id), todoId, version, done)
)
vi.mocked(api.lists.renameTodo).mockImplementation(async (id, todoId, { version, title }) =>
  server.renameTodo(inServer(id), todoId, version, title)
)
vi.mocked(api.lists.deleteTodo).mockImplementation(async (id, todoId, version) =>
  server.deleteTodo(inServer(id), todoId, version)
)
vi.mocked(api.lists.moveTodo).mockImplementation(async (id, todoId, { above, below }) => ({
  open_todo_ids: server.moveTodo(inServer(id), todoId, above, below),
}))

const until = (done: () => boolean) =>
  vi.waitFor(
    () => {
      if (!done()) throw new Error('not yet')
    },
    { interval: 1, timeout: 5000 }
  )

const rows = () => document.querySelectorAll('[data-open-todo]')
const openTodos = (): TodoView[] =>
  (lists.snapshot.details[LIST_ID]?.todos ?? []).filter((todo) => !todo.done)
/** Every save answered, so each iteration starts from a settled list. */
const settled = () => until(() => lists.snapshot.queueLength === 0)

async function openList() {
  lists.reset()
  history.replaceState({}, '', `/dashboard/lists/${LIST_ID}`)
  const screen = await render(ListsPanel)
  await until(() => rows().length === COUNT - 1 || rows().length === COUNT)
  await screen.unmount()
}

test('ListsPanel open (1,000 todos)', async ({ bench }) => {
  await bench('open', openList).run()
})

test('ListsPanel changes (1,000 todos)', async ({ bench }) => {
  lists.reset()
  history.replaceState({}, '', `/dashboard/lists/${LIST_ID}`)
  await render(ListsPanel)
  // One slot is kept free for the add.
  const spare = openTodos().at(-1)!
  lists.deleteTodo(LIST_ID, spare)
  await until(() => rows().length === COUNT - 1)
  await settled()

  await bench.compare(
    // Each pairs a change with its undo, so every iteration sees the same list.
    bench('add and delete', async () => {
      lists.addTodo(LIST_ID, 'The last one')
      await tick()
      await until(() => rows().length === COUNT)
      await settled()
      lists.deleteTodo(LIST_ID, openTodos().at(-1)!)
      await tick()
      await settled()
    }),
    bench('mark done and back', async () => {
      const first = openTodos()[0]!
      lists.markTodo(LIST_ID, first, true)
      await tick()
      await settled()
      const done = lists.snapshot.details[LIST_ID]!.todos.find((todo) => todo.id === first.id)!
      lists.markTodo(LIST_ID, done, false)
      await tick()
      await settled()
    }),
    bench('rename and back', async () => {
      const first = openTodos()[0]!
      lists.submitRename(lists.startTodoRename(LIST_ID, first), `${first.title}!`)
      await tick()
      await settled()
      const renamed = openTodos()[0]!
      lists.submitRename(lists.startTodoRename(LIST_ID, renamed), first.title)
      await tick()
      await settled()
    }),
    bench('move to the top and back', async () => {
      const third = openTodos()[2]!
      lists.moveTodo(LIST_ID, third, 0)
      await tick()
      await settled()
      lists.moveTodo(LIST_ID, openTodos()[0]!, 2)
      await tick()
      await settled()
    }),
    { throws: true }
  )
})
