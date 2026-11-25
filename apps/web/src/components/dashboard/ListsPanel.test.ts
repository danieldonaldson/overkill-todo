import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import ListsPanel from './ListsPanel.svelte'

const toastError = vi.hoisted(() => vi.fn())

vi.mock('svelte-sonner', () => ({ toast: { error: toastError } }))

vi.mock('../../lib/api', () => ({
  api: {
    lists: {
      list: vi.fn(),
      create: vi.fn(),
      get: vi.fn(),
      addTodo: vi.fn(),
      markTodo: vi.fn(),
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

import { ApiErrorException } from '../../lib/api/errors'
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
})
