// Interleavings the two-tab random test can't see, because it refetches before
// comparing (OT-013 design D7). Here one tab is compared with the server as it
// stands, with no final refetch.

import { describe, expect, it } from 'vitest'
import { ApiErrorException, SessionEndedError } from '../api/errors'
import { ListsEngine, type ListsApi } from './engine'
import { MemoryListsServer, serverApi } from './memory-server'

function keys() {
  let next = 0
  return () => `key-${++next}`
}

const macrotask = () => new Promise<void>((resolve) => setTimeout(resolve, 0))

/** Let every request, reload and repeat finish. Bounded, so a loop shows up. */
async function idle(rounds = 30): Promise<void> {
  for (let i = 0; i < rounds; i++) await macrotask()
}

function tab(server: MemoryListsServer, override: Partial<ListsApi> = {}) {
  const notices: string[] = []
  const engine = new ListsEngine({
    api: { ...serverApi(server), ...override },
    newKey: keys(),
    timeoutMs: Number.POSITIVE_INFINITY,
    onNotice: (notice) => notices.push(notice.text),
  })
  return { engine, notices }
}

const shape = (todos: { title: string; done: boolean; version: number }[]) =>
  todos.map((todo) => [todo.title, todo.done, todo.version])

describe('lists engine: overlapping loads', () => {
  it('requests each thing at most once per load, and settles', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one'])
    let listCalls = 0
    let detailCalls = 0
    const { engine } = tab(server, {
      list: async () => {
        listCalls += 1
        await macrotask()
        return server.list()
      },
      getList: async (listId) => {
        detailCalls += 1
        await macrotask()
        return server.getList(listId)
      },
    })
    void engine.loadLists()
    void engine.loadLists()
    void engine.loadList(id)
    void engine.loadList(id)
    await idle()

    expect(listCalls).toBe(2)
    expect(detailCalls).toBe(2)
    expect(engine.snapshot().listsStatus).toBe('ready')
    expect(engine.snapshot().details[id].status).toBe('ready')
  })
})

describe('lists engine: order without a refetch', () => {
  it('puts an unticked todo at the bottom of the open ones', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['A', 'B', 'C'])
    const { engine } = tab(server)
    await engine.loadList(id)
    const mark = (title: string, done: boolean) => {
      const todo = engine.snapshot().details[id].todos.find((t) => t.title === title)!
      engine.markTodo(id, todo, done)
    }
    mark('A', true)
    await idle()
    mark('B', true)
    await idle()
    mark('A', false)
    expect(
      engine
        .snapshot()
        .details[id].todos.filter((t) => !t.done)
        .map((t) => t.title)
    ).toEqual(['C', 'A'])
    await idle()
    expect(shape(engine.snapshot().details[id].todos)).toEqual(shape(server.getList(id).todos))
  })

  it('matches the server for random edits with no faults and no conflicts', async () => {
    for (let seed = 1; seed <= 100; seed++) {
      let state = seed
      const rng = () => {
        state = (Math.imul(state, 1_664_525) + 1_013_904_223) >>> 0
        return state / 4_294_967_296
      }
      const server = new MemoryListsServer()
      const id = server.seededList('Work', ['one', 'two', 'three'])
      const between = async () => {
        for (let i = Math.floor(rng() * 6); i > 0; i--) await Promise.resolve()
      }
      const { engine, notices } = tab(server, serverApi(server, between))
      await engine.loadLists()
      await engine.loadList(id)
      const intent = new Map<string, boolean>()
      let lastName = 'Work'
      for (let step = 0; step < 30; step++) {
        const todos = engine.snapshot().details[id].todos
        const roll = rng()
        if (roll < 0.25 || todos.length === 0) {
          engine.addTodo(id, `t-${step}`)
          intent.set(`t-${step}`, false)
        } else if (roll < 0.5) {
          const todo = todos[Math.floor(rng() * todos.length)]
          engine.markTodo(id, todo, !todo.done)
          intent.set(todo.title, !todo.done)
        } else if (roll < 0.65) {
          const todo = todos[Math.floor(rng() * todos.length)]
          const title = `r-${step}`
          engine.renameTodo(id, todo, title)
          intent.set(title, intent.get(todo.title) ?? todo.done)
          intent.delete(todo.title)
        } else if (roll < 0.78 && todos.length > 1) {
          const todo = todos[Math.floor(rng() * todos.length)]
          engine.deleteTodo(id, todo)
          intent.delete(todo.title)
        } else if (roll < 0.9) {
          // Also back to a name already used: it must not be lost on the way.
          lastName = step % 2 ? 'Work' : `L-${step}`
          engine.renameList(id, lastName)
        } else {
          void engine.loadList(id)
          void engine.loadLists()
        }
        for (let i = Math.floor(rng() * 8); i > 0; i--) await Promise.resolve()
      }
      await idle(10)
      expect(notices, `seed ${seed}: no false conflict`).toEqual([])
      const stored = server.getList(id).todos
      expect(shape(engine.snapshot().details[id].todos), `seed ${seed}`).toEqual(shape(stored))
      expect(
        engine.snapshot().lists.find((list) => list.id === id)?.name,
        `seed ${seed}: list name`
      ).toBe(server.getList(id).list.name)
      expect(server.getList(id).list.name, `seed ${seed}: last name`).toBe(lastName)
      for (const [title, done] of intent) {
        expect(stored.find((t) => t.title === title)?.done, `seed ${seed}: ${title}`).toBe(done)
      }
    }
  })
})

describe('lists engine: one item before and after it has an ID', () => {
  it('ticks and unticks a new todo without a conflict', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', [])
    const addGate = Promise.withResolvers<void>()
    const markGate = Promise.withResolvers<void>()
    const base = serverApi(server)
    const { engine, notices } = tab(server, {
      addTodo: async (...args) => {
        await addGate.promise
        return base.addTodo(...args)
      },
      markTodo: async (...args) => {
        await markGate.promise
        return base.markTodo(...args)
      },
    })
    await engine.loadList(id)
    const only = () => engine.snapshot().details[id].todos[0]
    engine.addTodo(id, 'X')
    engine.markTodo(id, only(), true)
    addGate.resolve()
    await idle(5)
    engine.markTodo(id, only(), false)
    markGate.resolve()
    await idle()

    expect(notices).toEqual([])
    expect(server.getList(id).todos[0]).toMatchObject({ done: false, version: 3 })
    expect(only()).toMatchObject({ done: false, version: 3 })
  })
})

describe('lists engine: refresh and recovery', () => {
  it('keeps todos and queued edits on screen during a refresh', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one'])
    const gate = Promise.withResolvers<void>()
    let gated = false
    const { engine } = tab(server, {
      getList: async (listId) => {
        if (gated) await gate.promise
        return server.getList(listId)
      },
      addTodo: () => new Promise(() => {}),
    })
    await engine.loadList(id)
    gated = true
    void engine.loadList(id)
    engine.addTodo(id, 'two')

    const detail = engine.snapshot().details[id]
    expect(detail.status).toBe('ready')
    expect(detail.todos.map((t) => t.title)).toEqual(['one', 'two'])
  })

  it('keeps the list offline and recovers when the server answers', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one'])
    let offline = false
    const base = serverApi(server)
    const guard = async () => {
      if (offline) throw new TypeError('offline')
    }
    const { engine } = tab(server, {
      list: async () => (await guard(), base.list()),
      getList: async (listId) => (await guard(), base.getList(listId)),
      addTodo: async (listId, key, title) => {
        // Saved, but the answer is lost.
        const todo = await base.addTodo(listId, key, title)
        await guard()
        return todo
      },
    })
    await engine.loadLists()
    await engine.loadList(id)
    offline = true
    engine.addTodo(id, 'two')
    await idle()
    expect(engine.snapshot().details[id].status).toBe('ready')
    expect(engine.snapshot().details[id].todos.map((t) => t.title)).toEqual(['one'])

    // Focus while still offline must not use up the recovery.
    engine.retry()
    await idle()
    offline = false
    engine.retry()
    await idle()
    expect(engine.snapshot().details[id].todos.map((t) => t.title)).toEqual(['one', 'two'])
  })

  it('refetches after the next save the server answers', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', [])
    let offline = false
    const base = serverApi(server)
    const { engine } = tab(server, {
      getList: async (listId) => {
        if (offline) throw new TypeError('offline')
        return base.getList(listId)
      },
      addTodo: async (listId, key, title) => {
        const todo = await base.addTodo(listId, key, title)
        if (offline) throw new TypeError('lost')
        return todo
      },
    })
    await engine.loadList(id)
    offline = true
    engine.addTodo(id, 'one')
    await idle()
    offline = false
    engine.addTodo(id, 'two')
    await idle()
    expect(engine.snapshot().details[id].todos.map((t) => t.title)).toEqual(['one', 'two'])
  })
})

describe('lists engine: reset during a save', () => {
  it('never repeats the old save, and sends the next one at once', async () => {
    const server = new MemoryListsServer()
    const first = Promise.withResolvers<never>()
    const sent: string[] = []
    const base = serverApi(server)
    const { engine } = tab(server, {
      createList: async (key, name) => {
        sent.push(name)
        if (name === 'old') return first.promise
        return base.createList(key, name)
      },
    })
    engine.createList('old')
    engine.reset()
    engine.createList('new')
    await idle(5)
    expect(sent).toEqual(['old', 'new'])
    first.reject(new TypeError('lost'))
    await idle(5)
    expect(sent).toEqual(['old', 'new'])
    expect(engine.snapshot().lists.map((l) => l.name)).toEqual(['new'])
  })
})

describe('lists engine: refusals and broken answers', () => {
  it('does not resend a save whose answer makes no sense', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one'])
    let sent = 0
    const { engine, notices } = tab(server, {
      markTodo: async () => {
        sent += 1
        return null as never
      },
    })
    await engine.loadList(id)
    engine.markTodo(id, engine.snapshot().details[id].todos[0], true)
    await idle()
    expect(sent).toBe(1)
    expect(notices).toHaveLength(1)
    expect(engine.snapshot().queueLength).toBe(0)
  })

  it('drops every edit of a deleted list with one message and keeps the texts', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two'])
    const restored: string[] = []
    let count = 0
    const engine = new ListsEngine({
      api: serverApi(server),
      newKey: keys(),
      timeoutMs: Number.POSITIVE_INFINITY,
      onNotice: (notice) => {
        count += 1
        restored.push(...(notice.restore ?? []).map((item) => item.text))
      },
    })
    await engine.loadLists()
    await engine.loadList(id)
    const [one, two] = engine.snapshot().details[id].todos
    server.removeList(id)
    engine.markTodo(id, one, true)
    engine.markTodo(id, two, true)
    engine.addTodo(id, 'three')
    await idle()
    expect(count).toBe(1)
    expect(restored).toEqual(['three'])
    expect(engine.snapshot().details[id].status).toBe('not_found')
    expect(engine.snapshot().lists).toEqual([])
  })

  it('treats an ended session as final: no repeat, no message', async () => {
    const server = new MemoryListsServer()
    let sent = 0
    const { engine, notices } = tab(server, {
      createList: async () => {
        sent += 1
        throw new SessionEndedError('Session expired')
      },
    })
    engine.createList('x')
    await idle(5)
    expect(sent).toBe(1)
    expect(notices).toEqual([])
  })

  it("shows the server's message without the trace ID", async () => {
    const server = new MemoryListsServer()
    const { engine, notices } = tab(server, {
      createList: async () => {
        throw new ApiErrorException(
          { code: 'limit_reached', message: 'You can have at most 100 lists' },
          { status: 409, message: 'You can have at most 100 lists (trace: abc)' }
        )
      },
    })
    engine.createList('x')
    await idle(5)
    expect(notices).toEqual(['You can have at most 100 lists'])
  })

  it('fails a load that is never answered', async () => {
    const engine = new ListsEngine({
      api: { ...serverApi(new MemoryListsServer()), list: () => new Promise(() => {}) },
      timeoutMs: 5,
    })
    await engine.loadLists()
    expect(engine.snapshot().listsStatus).toBe('error')
  })

  it('shows the newer count of a list that is not open', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one'])
    const { engine } = tab(server)
    await engine.loadList(id)
    server.addTodo(id, 'other-tab', 'two')
    await engine.loadLists()
    expect(engine.snapshot().lists[0].open_count).toBe(2)
  })
})
