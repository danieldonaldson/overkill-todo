import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import { page, userEvent } from 'vitest/browser'
import ListsPanel from './ListsPanel.svelte'

const toastError = vi.hoisted(() => vi.fn())

vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }))

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

const markReady = vi.hoisted(() => vi.fn())
vi.mock('../../lib/telemetry/island-timing', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../lib/telemetry/island-timing')>()
  return {
    ...actual,
    createIslandTiming: () => Object.assign(new actual.IslandTimer(), { markReady }),
  }
})

import { ApiErrorException, SessionEndedError } from '../../lib/api/errors'
import { api } from '../../lib/api'
import { lists } from '../../lib/lists/store.svelte'
import { navigate } from '../../lib/navigation'
import type { List, ListDetail, Todo } from '../../lib/types'

const LIST_ID = '3f2b8c1e-9d4a-4f6b-8c2d-1e0a9b8c7d6e'
const OTHER_ID = '8a1c2d3e-4f5a-6b7c-8d9e-0f1a2b3c4d5e'

const list = (overrides: Partial<List> = {}): List => ({
  id: LIST_ID,
  name: 'Work',
  version: 1,
  open_count: 0,
  todo_count: 0,
  ...overrides,
})

const todo = (overrides: Partial<Todo> = {}): Todo => ({
  id: 'todo-1',
  title: 'Buy milk',
  version: 1,
  done: false,
  created_at: '2026-01-01T00:00:00Z',
  ...overrides,
})

const detail = (todos: Todo[], summary = list()): ListDetail => ({ list: summary, todos })

function setPath(path: string) {
  history.replaceState({}, '', path)
}

/** Type into an input the way the browser does (Playwright's fill re-renders). */
function typeInto(locator: { element(): HTMLElement | SVGElement }, value: string) {
  const input = locator.element() as HTMLInputElement
  input.focus()
  input.value = value
  input.dispatchEvent(new Event('input', { bubbles: true }))
}

function mockLoaded(todos: Todo[] = [], summary = list()) {
  vi.mocked(api.lists.list).mockResolvedValue([summary])
  vi.mocked(api.lists.get).mockResolvedValue(detail(todos, summary))
}

/** Whether closing or reloading the tab now would ask to confirm. */
function leavingAsks(): boolean {
  const event = new Event('beforeunload', { cancelable: true })
  window.dispatchEvent(event)
  return event.defaultPrevented
}

beforeEach(() => {
  lists.reset()
  vi.clearAllMocks()
  vi.mocked(api.lists.list).mockResolvedValue([])
  setPath(`/dashboard/lists/${LIST_ID}`)
})

afterEach(() => {
  setPath('/dashboard/lists')
})

describe('ListsPanel', () => {
  it('shows placeholders until the lists and todos load', async () => {
    const pendingLists = Promise.withResolvers<List[]>()
    const pendingDetail = Promise.withResolvers<ListDetail>()
    vi.mocked(api.lists.list).mockReturnValue(pendingLists.promise)
    vi.mocked(api.lists.get).mockReturnValue(pendingDetail.promise)

    const screen = await render(ListsPanel)
    await expect.element(screen.getByTestId('list-skeleton')).toBeInTheDocument()

    pendingLists.resolve([list()])
    pendingDetail.resolve(detail([todo()]))
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()
  })

  it('shows an empty list with the add field and no Completed group', async () => {
    mockLoaded([])
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('No todos yet. Add your first one.')).toBeInTheDocument()
    await expect.element(screen.getByLabelText('New todo title')).toBeInTheDocument()
    expect(screen.getByText(/Completed/).elements()).toHaveLength(0)
  })

  it('groups done todos in a collapsed Completed group', async () => {
    mockLoaded([
      todo({ id: 'a', title: 'open one' }),
      todo({
        id: 'b',
        title: 'done one',
        done: true,
        version: 2,
        completed_at: '2026-02-01T00:00:00Z',
      }),
    ])
    const screen = await render(ListsPanel)
    await expect
      .element(screen.getByRole('button', { name: /Completed \(1\)/ }))
      .toHaveAttribute('aria-expanded', 'false')
    expect(screen.getByText('done one').elements()).toHaveLength(0)
    await screen.getByRole('button', { name: /Completed \(1\)/ }).click()
    await expect.element(screen.getByText('done one')).toBeInTheDocument()
  })

  it('creates a list and replaces the address without reloading', async () => {
    setPath('/dashboard/lists')
    vi.mocked(api.lists.list).mockResolvedValue([])
    vi.mocked(api.lists.create).mockResolvedValue(list({ name: 'Groceries' }))
    vi.mocked(api.lists.get).mockResolvedValue(detail([], list({ name: 'Groceries' })))

    const screen = await render(ListsPanel)
    await screen.getByLabelText('New list name').fill('Groceries')
    await screen.getByRole('button', { name: 'Create' }).click()

    await vi.waitFor(() => expect(location.pathname).toBe(`/dashboard/lists/${LIST_ID}`))
    expect(api.lists.create).toHaveBeenCalledWith(expect.objectContaining({ name: 'Groceries' }))
    await expect.element(screen.getByRole('heading', { name: 'Groceries' })).toBeInTheDocument()
  })

  it('adds a todo and marks it done optimistically', async () => {
    mockLoaded([])
    vi.mocked(api.lists.addTodo).mockResolvedValue(todo({ id: 'todo-2', title: 'Write tests' }))
    vi.mocked(api.lists.markTodo).mockResolvedValue(
      todo({ id: 'todo-2', title: 'Write tests', done: true, version: 2 })
    )

    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    typeInto(screen.getByPlaceholder('Add a todo...'), 'Write tests')
    await screen.getByRole('button', { name: 'Add' }).click()
    await expect
      .element(screen.getByRole('checkbox', { name: 'Mark "Write tests" done' }))
      .toBeInTheDocument()

    await screen.getByRole('checkbox', { name: 'Mark "Write tests" done' }).click()
    await expect
      .element(screen.getByRole('button', { name: /Completed \(1\)/ }))
      .toBeInTheDocument()
    await screen.getByRole('button', { name: /Completed \(1\)/ }).click()
    await expect
      .element(screen.getByRole('checkbox', { name: 'Mark "Write tests" not done' }))
      .toBeChecked()
    expect(api.lists.markTodo).toHaveBeenCalled()
  })

  it('turns pasted line breaks into spaces', async () => {
    mockLoaded([])
    vi.mocked(api.lists.addTodo).mockResolvedValue(todo({ title: 'first line second line third' }))
    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    const input = screen.getByPlaceholder('Add a todo...').element() as HTMLInputElement
    expect(input.isConnected).toBe(true)
    const data = new DataTransfer()
    data.setData('text/plain', 'first line\nsecond line\r\nthird')
    data.setData('text', 'first line\nsecond line\r\nthird')
    const paste = new ClipboardEvent('paste', { bubbles: true, cancelable: true })
    Object.defineProperty(paste, 'clipboardData', { value: data })
    input.dispatchEvent(paste)
    await screen.getByRole('button', { name: 'Add' }).click()
    expect(api.lists.addTodo).toHaveBeenCalledWith(
      LIST_ID,
      expect.objectContaining({ title: 'first line second line third' })
    )
  })

  it('keeps its text and names the limit for 501 characters', async () => {
    mockLoaded([])
    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    const input = screen.getByPlaceholder('Add a todo...')
    const tooLong = '😀'.repeat(501)
    typeInto(input, tooLong)
    await screen.getByRole('button', { name: 'Add' }).click()

    await expect
      .element(screen.getByText('A todo can be at most 500 characters'))
      .toBeInTheDocument()
    expect(api.lists.addTodo).not.toHaveBeenCalled()
    expect((input.element() as HTMLInputElement).value).toBe(tooLong)
  })

  it('accepts 500 emoji and never uses maxlength', async () => {
    mockLoaded([])
    vi.mocked(api.lists.addTodo).mockResolvedValue(todo({ title: '😀'.repeat(500) }))
    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    const input = screen.getByPlaceholder('Add a todo...')
    expect(input.element().getAttribute('maxlength')).toBeNull()
    typeInto(input, '😀'.repeat(500))
    await screen.getByRole('button', { name: 'Add' }).click()
    expect(api.lists.addTodo).toHaveBeenCalledWith(
      LIST_ID,
      expect.objectContaining({ title: '😀'.repeat(500) })
    )
  })

  it('shows a failed save, announces it and reverts the change', async () => {
    mockLoaded([todo()])
    vi.mocked(api.lists.markTodo).mockRejectedValue(new TypeError('offline'))
    const screen = await render(ListsPanel)

    await screen.getByRole('checkbox', { name: 'Mark "Buy milk" done' }).click()
    await vi.waitFor(() => expect(toastError).toHaveBeenCalled())
    await expect
      .element(screen.getByRole('checkbox', { name: 'Mark "Buy milk" done' }))
      .not.toBeChecked()
  })

  it('shows markup in a title literally', async () => {
    mockLoaded([todo({ title: '<img src=x onerror=alert(1)>' })])
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument()
    expect(document.querySelector('img[src="x"]')).toBeNull()
  })

  it('shows list not found for a malformed address without a request', async () => {
    setPath('/dashboard/lists/not-a-uuid')
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('List not found')).toBeInTheDocument()
    expect(api.lists.get).not.toHaveBeenCalled()
  })

  it('shows list not found when the server says 404', async () => {
    vi.mocked(api.lists.list).mockResolvedValue([list()])
    vi.mocked(api.lists.get).mockRejectedValue(
      new ApiErrorException(
        { code: 'list_not_found', message: 'List not found' },
        { status: 404, message: 'List not found' }
      )
    )
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('List not found')).toBeInTheDocument()
  })

  it('shows a retryable failure when the lists cannot be loaded', async () => {
    let offline = true
    vi.mocked(api.lists.list).mockImplementation(async () => {
      if (offline) throw new TypeError('offline')
      return [list()]
    })
    vi.mocked(api.lists.get).mockImplementation(async () => {
      if (offline) throw new TypeError('offline')
      return detail([todo()])
    })
    const screen = await render(ListsPanel)

    await expect.element(screen.getByText("Couldn't load your lists.")).toBeInTheDocument()
    offline = false
    await screen.getByRole('button', { name: 'Retry' }).first().click()
    await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()
  })

  it('marks the open list in the picker and navigates to another', async () => {
    vi.mocked(api.lists.list).mockResolvedValue([list(), list({ id: OTHER_ID, name: 'Home' })])
    vi.mocked(api.lists.get).mockResolvedValue(detail([]))
    const screen = await render(ListsPanel)

    await expect
      .element(screen.getByRole('button', { name: /Work/ }))
      .toHaveAttribute('aria-current', 'page')
    await screen.getByRole('button', { name: /Home/ }).click()
    expect(navigate).toHaveBeenCalledWith(`/dashboard/lists/${OTHER_ID}`)
  })

  it('never says there are no lists before the server has answered', async () => {
    setPath('/dashboard/lists')
    const answer = Promise.withResolvers<List[]>()
    vi.mocked(api.lists.list).mockReturnValue(answer.promise)
    const screen = await render(ListsPanel)
    await expect.element(screen.getByTestId('lists-skeleton')).toBeInTheDocument()
    expect(screen.getByText('No lists yet.', { exact: false }).elements()).toHaveLength(0)
    expect(screen.getByText('Create your first list', { exact: false }).elements()).toHaveLength(0)

    answer.reject(new TypeError('offline'))
    await expect.element(screen.getByText("Couldn't load your lists.")).toBeInTheDocument()
    expect(screen.getByText('Create your first list', { exact: false }).elements()).toHaveLength(0)
  })

  it('opens a new list once, so the overview can be opened again', async () => {
    setPath('/dashboard/lists/')
    vi.mocked(api.lists.create).mockResolvedValue(list({ name: 'Groceries' }))
    vi.mocked(api.lists.get).mockResolvedValue(detail([], list({ name: 'Groceries' })))
    const first = await render(ListsPanel)
    await first.getByLabelText('New list name').fill('Groceries')
    await first.getByRole('button', { name: 'Create' }).click()
    await vi.waitFor(() => expect(location.pathname).toBe(`/dashboard/lists/${LIST_ID}`))
    await first.unmount()

    setPath('/dashboard/lists')
    vi.mocked(api.lists.list).mockResolvedValue([list({ name: 'Groceries' })])
    const second = await render(ListsPanel)
    await expect.element(second.getByText('Select a list to see its todos.')).toBeInTheDocument()
    expect(location.pathname).toBe('/dashboard/lists')
  })

  it('keeps every failed title and the one being typed', async () => {
    mockLoaded([])
    vi.mocked(api.lists.addTodo).mockRejectedValue(
      new ApiErrorException(null, { status: 400, message: 'refused' })
    )
    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    lists.addTodo(LIST_ID, 'A')
    lists.addTodo(LIST_ID, 'B')
    typeInto(screen.getByPlaceholder('Add a todo...'), 'D')

    const notSaved = screen.getByRole('list', { name: 'Not saved' })
    await expect.element(notSaved.getByText('A', { exact: true })).toBeInTheDocument()
    await expect.element(notSaved.getByText('B', { exact: true })).toBeInTheDocument()
    expect((screen.getByPlaceholder('Add a todo...').element() as HTMLInputElement).value).toBe('D')

    vi.mocked(api.lists.addTodo).mockResolvedValue(todo({ id: 'saved', title: 'A' }))
    await notSaved.getByRole('button', { name: 'Try again' }).first().click()
    await expect
      .element(screen.getByRole('checkbox', { name: 'Mark "A" done' }))
      .toBeInTheDocument()
    expect(notSaved.getByText('A', { exact: true }).elements()).toHaveLength(0)
  })

  it('keeps the input and its focus while the list refreshes, and says if that fails', async () => {
    mockLoaded([todo()])
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()
    const input = screen.getByPlaceholder('Add a todo...')
    const element = input.element()
    typeInto(input, 'half typed')
    const answer = Promise.withResolvers<ListDetail>()
    vi.mocked(api.lists.get).mockReturnValue(answer.promise)
    void lists.loadList(LIST_ID)
    expect(element.isConnected).toBe(true)
    expect(document.activeElement).toBe(element)
    answer.reject(new TypeError('offline'))
    await expect.element(screen.getByText(/Couldn't refresh this list/)).toBeInTheDocument()
    await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()
    expect(element.isConnected).toBe(true)
  })

  it("replaces one Member's lists and drafts with the next one's", async () => {
    mockLoaded([todo()])
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()
    typeInto(screen.getByPlaceholder('Add a todo...'), 'private draft')
    mockLoaded([todo({ id: 'theirs', title: 'Other todo' })], list({ name: 'Theirs' }))
    lists.switchAccount()
    await expect.element(screen.getByText('Other todo')).toBeInTheDocument()
    expect(screen.getByText('Buy milk').elements()).toHaveLength(0)
    expect((screen.getByPlaceholder('Add a todo...').element() as HTMLInputElement).value).toBe('')
  })

  it('shows list not found for a nested address without a request', async () => {
    setPath(`/dashboard/lists/${LIST_ID}/extra`)
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('List not found')).toBeInTheDocument()
    expect(api.lists.get).not.toHaveBeenCalled()
  })

  it('opens an upper-case address as the same list', async () => {
    setPath(`/dashboard/lists/${LIST_ID.toUpperCase()}`)
    mockLoaded([todo()])
    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    expect(api.lists.get).toHaveBeenCalledWith(LIST_ID)
  })

  it('turns line breaks pasted into a list name into spaces', async () => {
    mockLoaded([])
    const screen = await render(ListsPanel)
    const input = screen.getByLabelText('New list name').element() as HTMLInputElement
    const data = new DataTransfer()
    data.setData('text/plain', 'Work\nHome')
    const paste = new ClipboardEvent('paste', { bubbles: true, cancelable: true })
    Object.defineProperty(paste, 'clipboardData', { value: data })
    input.dispatchEvent(paste)
    expect(input.value).toBe('Work Home')
  })

  it('keeps keyboard focus in the list after a tick, and shows no empty group', async () => {
    mockLoaded([todo({ id: 'a', title: 'first' }), todo({ id: 'b', title: 'second' })])
    vi.mocked(api.lists.markTodo).mockReturnValue(new Promise(() => {}))
    const screen = await render(ListsPanel)
    await screen.getByRole('checkbox', { name: 'Mark "first" done' }).click()
    await expect.element(screen.getByRole('checkbox', { name: 'Mark "second" done' })).toHaveFocus()
    await screen.getByRole('checkbox', { name: 'Mark "second" done' }).click()
    await expect.element(screen.getByLabelText('New todo title')).toHaveFocus()
    expect(screen.getByRole('list', { name: 'Todos' }).elements()).toHaveLength(0)
  })

  it('marks the panel ready only when the open list has an answer', async () => {
    const answer = Promise.withResolvers<ListDetail>()
    vi.mocked(api.lists.list).mockResolvedValue([list()])
    vi.mocked(api.lists.get).mockReturnValue(answer.promise)
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('1 / 100')).toBeInTheDocument()
    expect(markReady).not.toHaveBeenCalled()
    answer.resolve(detail([]))
    await vi.waitFor(() => expect(markReady).toHaveBeenCalled())
  })

  it('contains a crash to the panel and rebuilds it on Try again', async () => {
    mockLoaded([todo()])
    const hook = globalThis as { __otCrashIsland?: string }
    hook.__otCrashIsland = 'ListsPanel'
    try {
      const screen = await render(ListsPanel)
      const again = screen.getByRole('button', { name: 'Try again' })
      await expect.element(again).toBeInTheDocument()
      expect(screen.getByText('Deliberate', { exact: false }).elements()).toHaveLength(0)
      delete hook.__otCrashIsland
      await again.click()
      await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()
    } finally {
      delete hook.__otCrashIsland
    }
  })

  it('keeps the picker usable when only the todos fail to load', async () => {
    vi.mocked(api.lists.list).mockResolvedValue([list(), list({ id: OTHER_ID, name: 'Home' })])
    vi.mocked(api.lists.get).mockRejectedValue(new ApiErrorException(null, { status: 500 }))
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText("Couldn't load this list.")).toBeInTheDocument()
    expect(screen.getByText("Couldn't load your lists.").elements()).toHaveLength(0)
    expect(screen.getByText('List not found').elements()).toHaveLength(0)
    expect(screen.getByText('No todos yet', { exact: false }).elements()).toHaveLength(0)
    await screen.getByRole('button', { name: /Home/ }).click()
    expect(navigate).toHaveBeenCalledWith(`/dashboard/lists/${OTHER_ID}`)
  })

  it('does not show a change as saved when the session ends, and asks nothing on leaving', async () => {
    mockLoaded([todo()])
    vi.mocked(api.lists.markTodo).mockRejectedValue(new SessionEndedError('Session expired'))
    const screen = await render(ListsPanel)
    await screen.getByRole('checkbox', { name: 'Mark "Buy milk" done' }).click()
    await vi.waitFor(() => expect(lists.snapshot.queueLength).toBe(0))
    expect(toastError).not.toHaveBeenCalled()
    expect(screen.getByRole('button', { name: /Completed/ }).elements()).toHaveLength(0)
    expect(leavingAsks()).toBe(false)
  })

  it('reports a save that fails after the Member moved away', async () => {
    mockLoaded([])
    const answer = Promise.withResolvers<Todo>()
    vi.mocked(api.lists.addTodo).mockReturnValue(answer.promise)
    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()
    lists.addTodo(LIST_ID, 'Left behind')
    await screen.unmount()
    answer.reject(new ApiErrorException(null, { status: 400, message: 'refused' }))
    await vi.waitFor(() => expect(toastError).toHaveBeenCalledTimes(1))

    // Back on the list, the text can be submitted again.
    const back = await render(ListsPanel)
    await expect.element(back.getByPlaceholder('Add a todo...')).toHaveValue('Left behind')
  })

  it('renames a todo in place and cancels with Escape', async () => {
    mockLoaded([todo({ id: 'todo-1', title: 'Buy milk' })])
    vi.mocked(api.lists.renameTodo).mockResolvedValue(
      todo({ id: 'todo-1', title: 'Buy oat milk', version: 2 })
    )
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()

    await screen.getByRole('button', { name: 'Rename' }).nth(1).click()
    const input = screen.getByLabelText('Todo title')
    await expect.element(input).toHaveValue('Buy milk')
    typeInto(input, 'not saved')
    input.element().dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))
    await expect.element(screen.getByText('Buy milk')).toBeInTheDocument()
    expect(api.lists.renameTodo).not.toHaveBeenCalled()

    await screen.getByRole('button', { name: 'Rename' }).nth(1).click()
    typeInto(screen.getByLabelText('Todo title'), 'Buy oat milk')
    await screen.getByRole('button', { name: 'Save' }).click()

    await expect.element(screen.getByText('Buy oat milk')).toBeInTheDocument()
    expect(api.lists.renameTodo).toHaveBeenCalledWith(
      LIST_ID,
      'todo-1',
      expect.objectContaining({ title: 'Buy oat milk', version: 1 })
    )
  })

  it('deletes a todo and moves focus to the next one', async () => {
    mockLoaded([todo({ id: 'a', title: 'one' }), todo({ id: 'b', title: 'two' })])
    vi.mocked(api.lists.deleteTodo).mockResolvedValue(undefined)
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('one')).toBeInTheDocument()

    await screen.getByRole('button', { name: 'Delete' }).first().click()

    await expect.element(screen.getByText('one')).not.toBeInTheDocument()
    await vi.waitFor(() =>
      expect(document.activeElement?.getAttribute('aria-label')).toBe('Mark "two" done')
    )
    expect(api.lists.deleteTodo).toHaveBeenCalledWith(LIST_ID, 'a', 1)
  })

  it('renames a list in place', async () => {
    mockLoaded([], list({ name: 'Work' }))
    vi.mocked(api.lists.rename).mockResolvedValue(list({ name: 'Job', version: 2 }))
    const screen = await render(ListsPanel)
    await expect.element(screen.getByRole('heading', { name: 'Work' })).toBeInTheDocument()

    await screen.getByRole('button', { name: 'Rename' }).first().click()
    const input = screen.getByLabelText('List name')
    await expect.element(input).toHaveValue('Work')
    typeInto(input, 'Job')
    await screen.getByRole('button', { name: 'Save' }).click()

    await expect.element(screen.getByRole('heading', { name: 'Job' })).toBeInTheDocument()
    expect(api.lists.rename).toHaveBeenCalledWith(
      LIST_ID,
      expect.objectContaining({ name: 'Job', version: 1 })
    )
  })

  it('confirms a list deletion with its name and todo count', async () => {
    const summary = list({ todo_count: 2, open_count: 1 })
    mockLoaded(
      [todo({ id: 'a', title: 'one' }), todo({ id: 'b', title: 'two', done: true })],
      summary
    )
    vi.mocked(api.lists.deleteList).mockResolvedValue(undefined)
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('one')).toBeInTheDocument()

    await screen.getByRole('button', { name: 'Delete list' }).click()
    const dialog = screen.getByRole('dialog')
    await expect.element(dialog).toBeInTheDocument()
    await expect.element(dialog.getByText('Delete "Work"?')).toBeInTheDocument()
    await expect.element(dialog.getByText(/It holds\s+2\s+todos/)).toBeInTheDocument()

    await dialog.getByRole('button', { name: 'Delete list' }).click()
    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith('/dashboard/lists'))
    expect(api.lists.deleteList).toHaveBeenCalledWith(LIST_ID, 1, 2)
  })

  it('shows the current title and a note when a rename changed elsewhere', async () => {
    mockLoaded([todo({ id: 'todo-1', title: 'one' })])
    vi.mocked(api.lists.renameTodo).mockRejectedValue(
      new ApiErrorException(
        {
          code: 'stale',
          message: 'This todo changed elsewhere',
          current: todo({ id: 'todo-1', title: 'theirs', version: 2 }),
        },
        { status: 409, message: 'This todo changed elsewhere' }
      )
    )
    const screen = await render(ListsPanel)
    await screen.getByRole('button', { name: 'Rename' }).nth(1).click()
    typeInto(screen.getByLabelText('Todo title'), 'mine')
    await screen.getByRole('button', { name: 'Save' }).click()

    await vi.waitFor(() => expect(toastError).toHaveBeenCalled())
    expect(toastError.mock.calls[0][0]).toMatch(/changed elsewhere/)
    await expect.element(screen.getByText('theirs')).toBeInTheDocument()
    await expect.element(screen.getByText('mine')).toBeInTheDocument()
  })

  it('asks again with the current count when the list changed meanwhile', async () => {
    const summary = list({ todo_count: 2, open_count: 2 })
    mockLoaded([todo({ id: 'a', title: 'one' }), todo({ id: 'b', title: 'two' })], summary)
    vi.mocked(api.lists.deleteList).mockRejectedValue(
      new ApiErrorException(
        {
          code: 'stale',
          message: 'This list changed elsewhere',
          current: list({ todo_count: 3, open_count: 3 }),
        },
        { status: 409, message: 'This list changed elsewhere' }
      )
    )
    const screen = await render(ListsPanel)
    await screen.getByRole('button', { name: 'Delete list' }).click()
    const dialog = screen.getByRole('dialog')
    await dialog.getByRole('button', { name: 'Delete list' }).click()

    await expect.element(dialog.getByText(/Check the name and number of todos/)).toBeInTheDocument()
    await expect.element(dialog.getByText(/It holds\s+3\s+todos/)).toBeInTheDocument()
    expect(navigate).not.toHaveBeenCalled()
  })

  it('turns line breaks pasted into the rename fields into spaces', async () => {
    mockLoaded([todo({ id: 'a', title: 'one' })])
    const screen = await render(ListsPanel)
    const pasteInto = (label: string) => {
      const input = screen.getByLabelText(label).element() as HTMLInputElement
      const data = new DataTransfer()
      data.setData('text/plain', 'Buy milk\nand eggs')
      const paste = new ClipboardEvent('paste', { bubbles: true, cancelable: true })
      Object.defineProperty(paste, 'clipboardData', { value: data })
      input.value = ''
      input.dispatchEvent(paste)
      return input.value
    }
    await screen.getByRole('button', { name: 'Rename' }).nth(1).click()
    expect(pasteInto('Todo title')).toBe('Buy milk and eggs')
    await screen.getByRole('button', { name: 'Rename' }).first().click()
    expect(pasteInto('List name')).toBe('Buy milk and eggs')
  })

  it('saves a rename at the version shown when the form opened', async () => {
    mockLoaded([todo({ id: 'a', title: 'one' })])
    vi.mocked(api.lists.renameTodo).mockReturnValue(new Promise(() => {}))
    const screen = await render(ListsPanel)
    await screen.getByRole('button', { name: 'Rename' }).nth(1).click()
    mockLoaded([todo({ id: 'a', title: 'theirs', version: 2 })])
    await lists.loadList(LIST_ID)
    typeInto(screen.getByLabelText('Todo title'), 'mine')
    await screen.getByRole('button', { name: 'Save' }).click()
    await vi.waitFor(() =>
      expect(api.lists.renameTodo).toHaveBeenCalledWith(LIST_ID, 'a', { version: 1, title: 'mine' })
    )
  })

  it('says a deletion failed, not that the list changed, when the server is down', async () => {
    mockLoaded([])
    vi.mocked(api.lists.deleteList).mockRejectedValue(
      new ApiErrorException(null, { status: 503, message: 'down' })
    )
    const screen = await render(ListsPanel)
    await screen.getByRole('button', { name: 'Delete list' }).click()
    const dialog = screen.getByRole('dialog')
    await dialog.getByRole('button', { name: 'Delete list' }).click()
    await expect.element(dialog.getByText(/couldn't be deleted/)).toBeInTheDocument()
    await expect.element(dialog.getByText(/changed elsewhere/)).not.toBeInTheDocument()
  })

  it('stays where the Member went while a deletion was saving', async () => {
    mockLoaded([])
    const { promise, resolve } = Promise.withResolvers<undefined>()
    vi.mocked(api.lists.deleteList).mockReturnValue(promise)
    const screen = await render(ListsPanel)
    await screen.getByRole('button', { name: 'Delete list' }).click()
    await screen.getByRole('dialog').getByRole('button', { name: 'Delete list' }).click()
    await vi.waitFor(() => expect(api.lists.deleteList).toHaveBeenCalled())
    setPath('/dashboard/settings')
    resolve(undefined)
    await vi.waitFor(() => expect(lists.snapshot.lists).toEqual([]))
    await new Promise((done) => setTimeout(done, 20))
    expect(navigate).not.toHaveBeenCalled()
  })

  it('makes the page behind the deletion dialog inert, and Escape cancels', async () => {
    mockLoaded([])
    const screen = await render(ListsPanel)
    await screen.getByRole('button', { name: 'Delete list' }).click()
    const dialog = screen.getByRole('dialog')
    await expect.element(dialog).toBeInTheDocument()
    expect(dialog.element().matches(':modal')).toBe(true)
    const field = document.getElementById('new-todo-title')!
    field.focus()
    expect(document.activeElement).not.toBe(field)
    await userEvent.keyboard('{Escape}')
    await expect.element(dialog).not.toBeInTheDocument()
    expect(api.lists.deleteList).not.toHaveBeenCalled()
  })

  it('fits 500-character titles at 360 px in every new state', async () => {
    await page.viewport(360, 740)
    try {
      mockLoaded([todo({ id: 'a', title: '😀'.repeat(500) })], list({ todo_count: 1 }))
      const screen = await render(ListsPanel)
      await expect.element(screen.getByRole('list', { name: 'Todos' })).toBeInTheDocument()

      const fits = () => expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(360)

      await screen
        .getByRole('list', { name: 'Todos' })
        .getByRole('button', { name: 'Rename' })
        .click()
      fits()
      screen
        .getByLabelText('Todo title')
        .element()
        .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))

      await screen.getByRole('button', { name: 'Rename' }).first().click()
      fits()
      screen
        .getByLabelText('List name')
        .element()
        .dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))

      await screen.getByRole('button', { name: 'Delete list' }).click()
      await expect.element(screen.getByRole('dialog')).toBeInTheDocument()
      fits()
    } finally {
      await page.viewport(1280, 720)
    }
  })
})

describe('ListsPanel: reordering', () => {
  const titles = ['one', 'two', 'three']
  const ids = titles.map((title) => `todo-${title}`)

  async function open(extra: Todo[] = []) {
    const todos = titles.map((title, index) => todo({ id: ids[index], title }))
    mockLoaded([...todos, ...extra])
    // Answers with the order the neighbours ask for, as the server does.
    vi.mocked(api.lists.moveTodo).mockImplementation(async (_list, id, { above }) => {
      const order = ids.filter((other) => other !== id)
      order.splice(above.length === 0 ? 0 : order.indexOf(above[0]) + 1, 0, id)
      return { open_todo_ids: order }
    })
    const screen = await render(ListsPanel)
    await expect.element(screen.getByText('three', { exact: true })).toBeInTheDocument()
    return screen
  }

  const shown = () =>
    [...document.querySelectorAll('[data-open-todo] > span')].map((node) =>
      node.textContent?.trim()
    )
  const handle = (title: string) =>
    document.querySelector<HTMLElement>(`[aria-label='Move "${title}"']`)!
  const rowTop = (title: string) => handle(title).closest('li')!.getBoundingClientRect().top

  function pointer(target: Element, type: string, clientY: number, init: PointerEventInit = {}) {
    target.dispatchEvent(
      new PointerEvent(type, {
        bubbles: true,
        pointerId: 7,
        isPrimary: true,
        button: 0,
        clientX: 20,
        clientY,
        ...init,
      })
    )
  }

  /** A press on a handle. Only a real pointer can be captured. */
  function press(title: string, init: PointerEventInit = {}) {
    const target = handle(title)
    target.setPointerCapture = () => {}
    pointer(target, 'pointerdown', rowTop(title) + 10, init)
    return target
  }

  for (const pointerType of ['mouse', 'touch']) {
    it(`drags the third todo to the top (${pointerType})`, async () => {
      await open()
      const target = press('three', { pointerType })
      pointer(target, 'pointermove', rowTop('one') + 1, { pointerType })
      expect(api.lists.moveTodo).not.toHaveBeenCalled()
      await expect.element(page.getByTestId('drop-line')).toBeInTheDocument()
      pointer(target, 'pointerup', rowTop('one') + 1, { pointerType })
      await vi.waitFor(() => expect(shown()).toEqual(['three', 'one', 'two']))
      expect(api.lists.moveTodo).toHaveBeenCalledExactlyOnceWith(LIST_ID, ids[2], {
        above: [],
        below: [ids[0], ids[1]],
      })
      expect(page.getByTestId('drop-line').elements()).toHaveLength(0)
    })
  }

  it('drags a todo to the bottom', async () => {
    await open()
    const target = press('one')
    pointer(target, 'pointermove', rowTop('three') + 1000)
    pointer(target, 'pointerup', rowTop('three') + 1000)
    await vi.waitFor(() => expect(shown()).toEqual(['two', 'three', 'one']))
  })

  it('saves nothing for a drag that is cancelled or dropped where it was', async () => {
    await open()
    const cancelled = press('three')
    pointer(cancelled, 'pointermove', rowTop('one') + 1)
    await userEvent.keyboard('{Escape}')
    pointer(cancelled, 'pointerup', rowTop('one') + 1)

    const lost = press('three')
    pointer(lost, 'pointermove', rowTop('one') + 1)
    pointer(lost, 'pointercancel', rowTop('one') + 1)
    pointer(lost, 'pointerup', rowTop('one') + 1)

    for (const title of titles) {
      const back = press(title)
      pointer(back, 'pointermove', rowTop('three') + 1)
      pointer(back, 'pointermove', rowTop(title) + 10)
      pointer(back, 'pointerup', rowTop(title) + 10)
    }

    expect(shown()).toEqual(titles)
    expect(api.lists.moveTodo).not.toHaveBeenCalled()
    expect(lists.snapshot.queueLength).toBe(0)
  })

  it('ignores a second button or a second finger', async () => {
    await open()
    for (const init of [{ button: 2 }, { isPrimary: false }]) {
      const target = press('three', init)
      pointer(target, 'pointermove', rowTop('one') + 1)
      pointer(target, 'pointerup', rowTop('one') + 1)
    }
    expect(api.lists.moveTodo).not.toHaveBeenCalled()
  })

  it('moves with the keyboard, keeping focus and announcing the position', async () => {
    const screen = await open()
    const status = screen.getByTestId('move-announcement')
    const steps: [string, string[], string][] = [
      ['{ArrowUp}', ['one', 'three', 'two'], '"three", position 2 of 3'],
      ['{Home}', ['three', 'one', 'two'], '"three", position 1 of 3'],
      ['{ArrowUp}', ['three', 'one', 'two'], '"three", position 1 of 3'],
      ['{ArrowDown}', ['one', 'three', 'two'], '"three", position 2 of 3'],
      ['{End}', ['one', 'two', 'three'], '"three", position 3 of 3'],
    ]
    handle('three').focus()
    for (const [key, order, announced] of steps) {
      await userEvent.keyboard(key)
      await vi.waitFor(() => expect(shown()).toEqual(order))
      await expect.element(status).toHaveTextContent(announced)
      await vi.waitFor(() => expect(document.activeElement).toBe(handle('three')))
    }
    await expect.element(status).toHaveAttribute('role', 'status')
  })

  it('gives done todos no handle, and turns off touch scrolling on handles alone', async () => {
    const done = todo({
      id: 'todo-done',
      title: 'finished',
      done: true,
      completed_at: '2026-01-02T00:00:00Z',
    })
    const screen = await open([done])
    await screen.getByRole('button', { name: /Completed/ }).click()
    await expect.element(screen.getByText('finished')).toBeInTheDocument()
    expect(handle('finished')).toBeNull()
    expect(document.querySelectorAll('[aria-label^="Move "]')).toHaveLength(3)

    const style = document.createElement('style')
    style.textContent = '.touch-none { touch-action: none }'
    document.head.append(style)
    expect(getComputedStyle(handle('one')).touchAction).toBe('none')
    expect(getComputedStyle(handle('one').closest('li')!).touchAction).toBe('auto')
    style.remove()
  })

  it('puts a todo back and says so when its move fails', async () => {
    await open()
    vi.mocked(api.lists.moveTodo).mockRejectedValue(
      new ApiErrorException(null, { status: 503, message: 'down' })
    )
    handle('three').focus()
    await userEvent.keyboard('{Home}')
    await vi.waitFor(() => expect(toastError).toHaveBeenCalledOnce())
    expect(shown()).toEqual(titles)
  })
})
