import { describe, expect, it } from 'vitest'
import { ApiErrorException, SessionEndedError } from '../api/errors'
import { ListsEngine, type ListsApi, type TodoView } from './engine'
import type { ListDetail } from '../types'
import { MemoryListsServer, serverApi } from './memory-server'

function keys() {
  let next = 0
  return () => `key-${++next}`
}

async function settle(engine: ListsEngine): Promise<void> {
  for (let i = 0; i < 500; i++) {
    const snapshot = engine.snapshot()
    if (snapshot.queueLength === 0 && !snapshot.saving) {
      await Promise.resolve()
      const after = engine.snapshot()
      if (after.queueLength === 0 && !after.saving) return
    }
    await Promise.resolve()
  }
  throw new Error('engine did not settle')
}

function deferred<T>() {
  const { promise, resolve, reject } = Promise.withResolvers<T>()
  return { promise, resolve, reject }
}

async function loadedEngine(
  server: MemoryListsServer,
  listId: string,
  extra: { notices?: string[]; refetchAdvancesQueue?: boolean; api?: Partial<ListsApi> } = {}
) {
  const notices = extra.notices ?? []
  const engine = new ListsEngine({
    api: { ...serverApi(server), ...extra.api },
    newKey: keys(),
    timeoutMs: Number.POSITIVE_INFINITY,
    refetchAdvancesQueue: extra.refetchAdvancesQueue,
    onNotice: (notice) => notices.push(notice.text),
  })
  await engine.loadLists()
  await engine.loadList(listId)
  return { engine, notices }
}

function todoView(id: string, overrides: Partial<TodoView> = {}): TodoView {
  return {
    id,
    title: 'todo',
    version: 1,
    done: false,
    pending: false,
    ...overrides,
  }
}

describe('lists engine', () => {
  it('undoes only the failed save among several', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one', 'two', 'three'])
    const [one, two, three] = server.getList(listId).todos
    const notices: string[] = []
    const base = serverApi(server)
    const engine = new ListsEngine({
      api: {
        ...base,
        markTodo: async (list, todo, version, done) => {
          if (todo === two.id) {
            throw new ApiErrorException(null, { status: 503, message: 'boom' })
          }
          return base.markTodo(list, todo, version, done)
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
      onNotice: (notice) => notices.push(notice.text),
    })
    await engine.loadLists()
    await engine.loadList(listId)

    engine.markTodo(listId, todoView(one.id), true)
    engine.markTodo(listId, todoView(two.id), true)
    engine.markTodo(listId, todoView(three.id), true)
    await settle(engine)

    const todos = engine.snapshot().details[listId].todos
    expect(todos.find((todo) => todo.id === one.id)?.done).toBe(true)
    expect(todos.find((todo) => todo.id === two.id)?.done).toBe(false)
    expect(todos.find((todo) => todo.id === three.id)?.done).toBe(true)
    expect(notices).toHaveLength(1)
  })

  it('drops dependent edits with one message', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const notices: string[] = []
    const gate = deferred<void>()
    const base = serverApi(server)
    const engine = new ListsEngine({
      api: {
        ...base,
        markTodo: async () => {
          await gate.promise
          throw new ApiErrorException(null, { status: 503, message: 'boom' })
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
      onNotice: (notice) => notices.push(notice.text),
    })
    await engine.loadLists()
    await engine.loadList(listId)

    engine.markTodo(listId, todoView(todo.id), true)
    engine.markTodo(listId, todoView(todo.id, { done: true, version: 2 }), false)
    gate.resolve()
    await settle(engine)

    expect(notices).toHaveLength(1)
    expect(engine.snapshot().details[listId].todos[0].done).toBe(false)
    expect(engine.snapshot().queueLength).toBe(0)
  })

  it('shows the current row when a save is stale', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const { engine, notices } = await loadedEngine(server, listId)

    // Another tab marks it done first.
    server.markTodo(listId, todo.id, 1, true)
    engine.markTodo(listId, todoView(todo.id, { done: true, version: 1 }), false)
    await settle(engine)

    const view = engine.snapshot().details[listId].todos[0]
    expect(view.done).toBe(true)
    expect(view.version).toBe(2)
    expect(notices[0]).toContain('changed elsewhere')
  })

  it('removes a todo deleted elsewhere', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const { engine, notices } = await loadedEngine(server, listId)

    server.removeTodo(listId, todo.id)
    engine.markTodo(listId, todoView(todo.id), true)
    await settle(engine)

    expect(engine.snapshot().details[listId].todos).toHaveLength(0)
    expect(notices[0]).toContain('deleted elsewhere')
  })

  it('removes an addition refused at the limit and keeps its text', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', [])
    const notices: { text: string; restore?: { target: string; text: string }[] }[] = []
    const base = serverApi(server)
    const engine = new ListsEngine({
      api: {
        ...base,
        addTodo: async () => {
          throw new ApiErrorException(
            { code: 'limit_reached', message: 'A list can hold at most 1,000 todos' },
            { status: 409, message: 'A list can hold at most 1,000 todos' }
          )
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
      onNotice: (notice) => notices.push(notice),
    })
    await engine.loadLists()
    await engine.loadList(listId)

    engine.addTodo(listId, 'overflow')
    await settle(engine)

    expect(engine.snapshot().details[listId].todos).toHaveLength(0)
    expect(notices).toHaveLength(1)
    expect(notices[0].restore?.map((item) => item.text)).toEqual(['overflow'])
  })

  it('refetches after an unknown addition outcome', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', [])
    const notices: string[] = []
    const base = serverApi(server)
    const engine = new ListsEngine({
      api: {
        ...base,
        addTodo: async (list, key, title) => {
          // The server saves it, but neither answer reaches the tab.
          await base.addTodo(list, key, title)
          throw new TypeError('network lost')
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
      onNotice: (notice) => notices.push(notice.text),
    })
    await engine.loadLists()
    await engine.loadList(listId)

    engine.addTodo(listId, 'saved once')
    await settle(engine)

    const todos = engine.snapshot().details[listId].todos
    expect(todos.map((todo) => todo.title)).toEqual(['saved once'])
    expect(notices).toHaveLength(1)
    expect(engine.snapshot().queueLength).toBe(0)
  })

  it('rebases later queued edits only from n to n + 1', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const sent: string[] = []
    const base = serverApi(server)
    const engine = new ListsEngine({
      api: {
        ...base,
        markTodo: async (list, id, version, done) => {
          try {
            const result = await base.markTodo(list, id, version, done)
            sent.push(`ok:${version}`)
            return result
          } catch (error) {
            sent.push(`err:${version}:${(error as { status?: number }).status}`)
            throw error
          }
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
    })
    await engine.loadLists()
    await engine.loadList(listId)

    engine.markTodo(listId, todoView(todo.id), true)
    engine.markTodo(listId, todoView(todo.id, { done: true }), false)
    engine.markTodo(listId, todoView(todo.id, { done: false }), true)
    await settle(engine)

    expect(sent).toEqual(['ok:1', 'ok:2', 'ok:3'])
    expect(engine.snapshot().details[listId].todos[0].done).toBe(true)
  })

  it('drops a stale load and reapplies only the newest answer', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const first = deferred<ListDetail>()
    const second = deferred<ListDetail>()
    const calls = [first, second]
    const engine = new ListsEngine({
      api: {
        ...serverApi(server),
        getList: () => calls.shift()!.promise,
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
    })
    await engine.loadLists()

    const stale = engine.loadList(listId)
    const fresh = engine.loadList(listId)
    second.resolve(server.getList(listId))
    await fresh
    server.markTodo(listId, todo.id, 1, true)
    first.resolve(server.getList(listId))
    await stale
    await settle(engine)

    const view = engine.snapshot().details[listId].todos[0]
    expect(view.version).toBe(1)
    expect(view.done).toBe(false)
  })

  it('drops a load answered after a save and repeats it', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const gate = deferred<void>()
    let first = true
    const engine = new ListsEngine({
      api: {
        ...serverApi(server),
        getList: async (id) => {
          if (first) {
            first = false
            await gate.promise
            return server.getList(id)
          }
          return server.getList(id)
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
    })
    await engine.loadLists()
    const load = engine.loadList(listId)
    await Promise.resolve()

    engine.markTodo(listId, todoView(todo.id), true)
    await settle(engine)
    gate.resolve()
    await load
    await settle(engine)

    expect(engine.snapshot().details[listId].todos[0].done).toBe(true)
  })

  it('marks a todo that is still being added', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', [])
    const gate = deferred<void>()
    const base = serverApi(server)
    const engine = new ListsEngine({
      api: {
        ...base,
        addTodo: async (list, key, title) => {
          await gate.promise
          return base.addTodo(list, key, title)
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
    })
    await engine.loadLists()
    await engine.loadList(listId)

    engine.addTodo(listId, 'new')
    const pending = engine.snapshot().details[listId].todos[0]
    expect(pending.pending).toBe(true)
    engine.markTodo(listId, pending, true)
    gate.resolve()
    await settle(engine)

    const todo = engine.snapshot().details[listId].todos[0]
    expect(todo.done).toBe(true)
    expect(todo.pending).toBe(false)
  })

  it('counts a loaded list from its todos and other lists from GET /lists', async () => {
    const server = new MemoryListsServer()
    const work = server.seededList('Work', ['a', 'b'])
    const home = server.seededList('Home', ['c'])
    const { engine } = await loadedEngine(server, work)

    engine.addTodo(work, 'd')
    await settle(engine)

    const lists = engine.snapshot().lists
    expect(lists.find((list) => list.id === work)?.open_count).toBe(3)
    expect(lists.find((list) => list.id === home)?.open_count).toBe(1)
  })

  it('clears everything on reset', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const { engine } = await loadedEngine(server, listId)

    engine.markTodo(listId, todoView(todo.id), true)
    engine.reset()

    const snapshot = engine.snapshot()
    expect(snapshot.lists).toEqual([])
    expect(snapshot.details).toEqual({})
    expect(snapshot.queueLength).toBe(0)
    expect(snapshot.listsStatus).toBe('idle')
  })

  it('renames a todo at once and rebases a queued mark', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const { engine, notices } = await loadedEngine(server, listId)

    engine.renameTodo(listId, todoView(todo.id), 'renamed')
    expect(engine.snapshot().details[listId].todos[0].title).toBe('renamed')
    engine.markTodo(listId, todoView(todo.id, { title: 'renamed' }), true)
    await settle(engine)

    expect(notices).toEqual([])
    const stored = server.getList(listId).todos[0]
    expect(stored).toMatchObject({ title: 'renamed', done: true, version: 3 })
    expect(engine.snapshot().details[listId].todos[0]).toMatchObject({
      title: 'renamed',
      done: true,
      version: 3,
    })
  })

  it('shows the current title on a stale rename and keeps the text', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const notices: { text: string; restore?: { action: string; text: string }[] }[] = []
    const engine = new ListsEngine({
      api: serverApi(server),
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
      onNotice: (notice) => notices.push(notice),
    })
    await engine.loadLists()
    await engine.loadList(listId)

    server.renameTodo(listId, todo.id, 1, 'theirs')
    engine.renameTodo(listId, todoView(todo.id), 'mine')
    await settle(engine)

    const view = engine.snapshot().details[listId].todos[0]
    expect(view).toMatchObject({ title: 'theirs', version: 2 })
    expect(notices[0].text).toContain('changed elsewhere')
    expect(notices[0].restore).toEqual([
      { target: 'todo', action: 'rename', listId, todoId: todo.id, text: 'mine' },
    ])
  })

  it('deletes a todo at once, and a 404 counts as success', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const { engine, notices } = await loadedEngine(server, listId)

    engine.deleteTodo(listId, todoView(todo.id))
    expect(engine.snapshot().details[listId].todos).toHaveLength(0)
    await settle(engine)
    expect(server.getList(listId).todos).toHaveLength(0)

    // Deleted in another tab meanwhile: no message, still gone.
    server.removeTodo(listId, todo.id)
    engine.deleteTodo(listId, todoView(todo.id))
    await settle(engine)
    expect(notices).toEqual([])
    expect(engine.snapshot().details[listId].todos).toHaveLength(0)
  })

  it('restores a todo whose delete failed', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const [todo] = server.getList(listId).todos
    const base = serverApi(server)
    const { engine, notices } = await loadedEngine(server, listId, {
      notices: [],
      api: {
        ...base,
        deleteTodo: async () => {
          throw new ApiErrorException(null, { status: 503, message: 'boom' })
        },
      },
    })

    engine.deleteTodo(listId, todoView(todo.id))
    expect(engine.snapshot().details[listId].todos).toHaveLength(0)
    await settle(engine)

    expect(engine.snapshot().details[listId].todos).toHaveLength(1)
    expect(notices[0]).toContain("Couldn't save")
    expect(server.getList(listId).todos).toHaveLength(1)
  })

  it('renames a list at once and deletes it only after the server confirms', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const { engine, notices } = await loadedEngine(server, listId)

    engine.renameList(listId, 'Job')
    expect(engine.snapshot().lists[0].name).toBe('Job')
    await settle(engine)
    expect(server.getList(listId).list.name).toBe('Job')

    const result = engine.deleteList(listId, 2, 1)
    expect(engine.snapshot().deleting).toEqual([listId])
    expect(engine.snapshot().lists).toHaveLength(1)
    await settle(engine)
    expect(await result).toBe(true)
    expect(engine.snapshot().lists).toEqual([])
    expect(engine.snapshot().details[listId]).toBeUndefined()
    expect(notices).toEqual([])
    expect(server.list()).toEqual([])
  })

  it('keeps a list whose delete conflicts, and shows the current count', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', ['one'])
    const { engine, notices } = await loadedEngine(server, listId)

    // Another tab adds a sixth todo after the confirmation was shown.
    server.addTodo(listId, 'other-tab', 'two')
    const result = engine.deleteList(listId, 1, 1)
    await settle(engine)
    expect(await result).toBe(false)
    expect(engine.snapshot().lists[0].todo_count).toBe(2)
    expect(notices[0]).toContain('list changed elsewhere')
    expect(server.getList(listId).todos).toHaveLength(2)
  })

  it('renames and deletes a todo that is still being added', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', [])
    const gate = deferred<void>()
    const base = serverApi(server)
    const engine = new ListsEngine({
      api: {
        ...base,
        addTodo: async (list, key, title) => {
          await gate.promise
          return base.addTodo(list, key, title)
        },
      },
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
    })
    await engine.loadLists()
    await engine.loadList(listId)

    engine.addTodo(listId, 'new')
    const pending = engine.snapshot().details[listId].todos[0]
    engine.renameTodo(listId, pending, 'renamed')
    engine.deleteTodo(listId, { ...pending, title: 'renamed' })
    expect(engine.snapshot().details[listId].todos).toHaveLength(0)
    gate.resolve()
    await settle(engine)

    expect(server.getList(listId).todos).toHaveLength(0)
    expect(engine.snapshot().details[listId].todos).toHaveLength(0)
  })

  it('deletes a list and settles its queued delete when the session ends', async () => {
    const server = new MemoryListsServer()
    const listId = server.seededList('Work', [])
    const base = serverApi(server)
    const { engine } = await loadedEngine(server, listId, {
      notices: [],
      api: {
        ...base,
        deleteList: async () => {
          throw new SessionEndedError('Session expired')
        },
      },
    })

    const result = engine.deleteList(listId, 1, 0)
    await settle(engine)
    expect(await result).toBe(false)
    expect(engine.snapshot().lists).toHaveLength(1)
  })
})
