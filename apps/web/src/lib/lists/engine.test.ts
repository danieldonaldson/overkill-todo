import { describe, expect, it } from 'vitest'
import { ApiErrorException } from '../api/errors'
import { ListsEngine, type TodoView } from './engine'
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
  extra: { notices?: string[]; refetchAdvancesQueue?: boolean } = {}
) {
  const notices = extra.notices ?? []
  const engine = new ListsEngine({
    api: serverApi(server),
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
    const notices: { text: string; restore?: { target: string; text: string } }[] = []
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
    expect(notices[0].restore?.text).toBe('overflow')
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
})
