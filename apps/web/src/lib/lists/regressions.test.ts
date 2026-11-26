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
        // Every move on screen must survive its save, with no refetch to hide it.
        const open = todos.filter((todo) => !todo.done)
        if (open.length > 1 && rng() < 0.5) {
          const moved = open[Math.floor(rng() * open.length)]
          engine.moveTodo(id, moved, Math.floor(rng() * open.length))
        }
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
      const onScreen = engine.snapshot().details[id].todos.map((todo) => todo.title)
      await idle(10)
      expect(
        engine.snapshot().details[id].todos.map((todo) => todo.title),
        `seed ${seed}: nothing moves when the saves are answered`
      ).toEqual(onScreen)
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

// Found by models/list_queue.qnt (task 4.4).
describe('lists engine: a save that changed nothing', () => {
  it("never moves later edits onto another tab's version of a todo", async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one'])
    const todoId = server.getList(id).todos[0].id
    const base = serverApi(server)
    let offline = true
    const gate = Promise.withResolvers<void>()
    const { engine, notices } = tab(server, {
      getList: async (listId) => {
        if (offline && engine.snapshot().details[id]?.status === 'ready') throw new TypeError('x')
        return base.getList(listId)
      },
      renameTodo: async (...args) => {
        if (offline) throw new TypeError('offline')
        await gate.promise
        return base.renameTodo(...args)
      },
    })
    await engine.loadList(id)
    const only = () => engine.snapshot().details[id].todos[0]
    engine.renameTodo(id, only(), 'two')
    // Started on the unsaved title, then that save fails and is undone.
    const draft = engine.startTodoRename(id, only())
    await idle()
    expect(only().title).toBe('one')
    offline = false
    // Asks for the title already stored, so the server changes nothing.
    engine.submitRename(draft, 'one')
    engine.deleteTodo(id, only())
    server.markTodo(id, todoId, 1, true)
    gate.resolve()
    await idle()

    // The other tab's tick was never on screen: the delete must not take it.
    expect(server.getList(id).todos).toMatchObject([{ done: true, version: 2 }])
    expect(notices.at(-1)).toBe('This changed elsewhere. The current version is shown.')
  })
})

describe('lists engine: moves', () => {
  const titles = (engine: ListsEngine, id: string) =>
    engine.snapshot().details[id].todos.map((todo) => todo.title)
  const find = (engine: ListsEngine, id: string, title: string) =>
    engine.snapshot().details[id].todos.find((todo) => todo.title === title)!

  it('sends up to 20 neighbours each way, nearest first', async () => {
    const server = new MemoryListsServer()
    const names = Array.from({ length: 60 }, (_, index) => `t${index}`)
    const id = server.seededList('Work', names)
    const sent: [string[], string[]][] = []
    const base = serverApi(server)
    const { engine } = tab(server, {
      moveTodo: async (listId, todoId, above, below) => {
        sent.push([above, below])
        return base.moveTodo(listId, todoId, above, below)
      },
    })
    await engine.loadList(id)
    const ids = new Map(server.getList(id).todos.map((todo) => [todo.title, todo.id]))
    engine.moveTodo(id, find(engine, id, 't0'), 30)
    await idle(5)
    const named = (from: number, step: number) =>
      Array.from({ length: 20 }, (_, index) => ids.get(`t${from + index * step}`))
    expect(sent).toEqual([[named(30, -1), named(31, 1)]])
    expect(titles(engine, id)).toEqual(server.getList(id).todos.map((todo) => todo.title))
  })

  it('does nothing for the same place, a done todo or an unknown list', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two'])
    const { engine } = tab(server, { moveTodo: () => Promise.reject(new Error('sent')) })
    await engine.loadList(id)
    const one = find(engine, id, 'one')
    engine.moveTodo(id, one, 0)
    engine.moveTodo('elsewhere', one, 1)
    engine.markTodo(id, one, true)
    engine.moveTodo(id, find(engine, id, 'one'), 1)
    expect(engine.snapshot().queue.map((op) => op.kind)).toEqual(['mark_todo'])
  })

  it('sends the move of a todo still being added after its creation', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two'])
    const gate = Promise.withResolvers<void>()
    const base = serverApi(server)
    const { engine, notices } = tab(server, {
      addTodo: async (...args) => {
        await gate.promise
        return base.addTodo(...args)
      },
    })
    await engine.loadList(id)
    engine.addTodo(id, 'new')
    engine.moveTodo(id, find(engine, id, 'new'), 0)
    // A neighbour without an ID yet, too.
    engine.moveTodo(id, find(engine, id, 'two'), 1)
    expect(titles(engine, id)).toEqual(['new', 'two', 'one'])
    gate.resolve()
    await idle()
    expect(notices).toEqual([])
    expect(titles(engine, id)).toEqual(['new', 'two', 'one'])
    expect(server.getList(id).todos.map((todo) => todo.title)).toEqual(['new', 'two', 'one'])
  })

  it('leaves out a neighbour that was never saved, without moving to the top', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two', 'three'])
    const gate = Promise.withResolvers<void>()
    const { engine } = tab(server, {
      addTodo: async () => {
        await gate.promise
        throw new ApiErrorException(null, { status: 400, message: 'refused' })
      },
    })
    await engine.loadList(id)
    engine.addTodo(id, 'new')
    // Below `new` alone, then everything else is put under it.
    engine.moveTodo(id, find(engine, id, 'new'), 0)
    engine.moveTodo(id, find(engine, id, 'three'), 1)
    gate.resolve()
    await idle()
    expect(server.getList(id).todos.map((todo) => todo.title)).toEqual(['three', 'one', 'two'])
    expect(titles(engine, id)).toEqual(['three', 'one', 'two'])
  })

  it('refetches when the answer names other todos than the ones known', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two', 'three'])
    let loads = 0
    const base = serverApi(server)
    const { engine, notices } = tab(server, {
      getList: (listId) => {
        loads += 1
        return base.getList(listId)
      },
    })
    await engine.loadList(id)
    const [one, two] = server.getList(id).todos

    engine.moveTodo(id, find(engine, id, 'three'), 0)
    await idle(5)
    expect(loads).toBe(1)

    server.addTodo(id, 'other-tab', 'added')
    server.removeTodo(id, one.id)
    server.markTodo(id, two.id, 1, true)
    engine.moveTodo(id, find(engine, id, 'three'), 2)
    await idle()
    expect(loads).toBe(2)
    expect(notices).toEqual([])
    expect(shape(engine.snapshot().details[id].todos)).toEqual(shape(server.getList(id).todos))
  })

  it('undoes a failed move, and shows a todo completed or deleted elsewhere', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two', 'three'])
    const base = serverApi(server)
    let refuse = true
    const { engine, notices } = tab(server, {
      moveTodo: async (...args) => {
        if (refuse) throw new ApiErrorException(null, { status: 503, message: 'down' })
        return base.moveTodo(...args)
      },
    })
    await engine.loadList(id)
    const [one, two] = server.getList(id).todos
    engine.moveTodo(id, find(engine, id, 'three'), 0)
    expect(titles(engine, id)).toEqual(['three', 'one', 'two'])
    await idle()
    expect(titles(engine, id)).toEqual(['one', 'two', 'three'])
    expect(notices).toEqual([
      "Couldn't save your change. The list will refresh when the server is reachable.",
    ])

    refuse = false
    server.markTodo(id, one.id, 1, true)
    server.removeTodo(id, two.id)
    engine.moveTodo(id, find(engine, id, 'one'), 2)
    engine.moveTodo(id, find(engine, id, 'two'), 2)
    await idle()
    expect(notices.slice(1)).toEqual([
      'This changed elsewhere. The current version is shown.',
      'This todo was deleted elsewhere.',
    ])
    expect(shape(engine.snapshot().details[id].todos)).toEqual(shape(server.getList(id).todos))
  })

  it('applies the answer beneath what is still queued', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two', 'three'])
    const gates = [Promise.withResolvers<void>(), Promise.withResolvers<void>()]
    let sent = 0
    const base = serverApi(server)
    const { engine } = tab(server, {
      moveTodo: async (...args) => {
        await gates[sent++].promise
        return base.moveTodo(...args)
      },
    })
    await engine.loadList(id)
    engine.moveTodo(id, find(engine, id, 'three'), 0)
    engine.moveTodo(id, find(engine, id, 'one'), 2)
    engine.renameTodo(id, find(engine, id, 'two'), 'TWO')
    gates[0].resolve()
    await idle(5)
    expect(titles(engine, id)).toEqual(['three', 'TWO', 'one'])
    gates[1].resolve()
    await idle(5)
    expect(titles(engine, id)).toEqual(['three', 'TWO', 'one'])
  })

  it('keeps a move beside a rename made elsewhere', async () => {
    const server = new MemoryListsServer()
    const id = server.seededList('Work', ['one', 'two'])
    const first = tab(server)
    const second = tab(server)
    await first.engine.loadList(id)
    await second.engine.loadList(id)
    first.engine.moveTodo(id, find(first.engine, id, 'two'), 0)
    await idle(5)
    second.engine.renameTodo(id, find(second.engine, id, 'two'), 'TWO')
    await idle(5)
    expect([...first.notices, ...second.notices]).toEqual([])
    expect(server.getList(id).todos.map((todo) => todo.title)).toEqual(['TWO', 'one'])
  })
})
