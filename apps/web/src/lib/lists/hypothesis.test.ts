// Two copies of the client queue against one in-memory server (OT-013 design
// Hypothesis). The random test checks that repeated saves are idempotent, that
// a tab never applies a version it did not see, and that after both queues
// drain a single refetch in each tab leaves both showing exactly the server's
// state. The deterministic case below shows the property fails if a refetch is
// allowed to advance a queued edit.

import { describe, expect, it } from 'vitest'
import { ApiErrorException } from '../api/errors'
import { ListsEngine, type ListsApi, type TodoView } from './engine'
import { MemoryListsServer } from './memory-server'

const SEEDS = Number(import.meta.env.VITE_HYPOTHESIS_SEEDS ?? 200)

/** Deterministic 32-bit PRNG (mulberry32). */
function rngFor(seed: number): () => number {
  let state = seed >>> 0
  return () => {
    state = (state + 0x6d2b79f5) >>> 0
    let t = state
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4_294_967_296
  }
}

async function yieldTicks(rng: () => number): Promise<void> {
  const ticks = Math.floor(rng() * 3)
  for (let i = 0; i < ticks; i++) await Promise.resolve()
}

/** A tab's transport with seeded delays, 5xx failures and lost answers. */
function faultyApi(
  server: MemoryListsServer,
  rng: () => number,
  faults: { reads: boolean }
): ListsApi {
  const write = async <T>(apply: () => T, fault: boolean): Promise<T> => {
    await yieldTicks(rng)
    if (fault) {
      const roll = rng()
      if (roll < 0.07) throw new ApiErrorException(null, { status: 503, message: 'injected' })
      if (roll < 0.14) {
        // The server applies it; the answer never arrives.
        apply()
        throw new TypeError('lost answer')
      }
    }
    return apply()
  }
  const read = async <T>(answer: () => T): Promise<T> => {
    await yieldTicks(rng)
    if (faults.reads && rng() < 0.1) throw new TypeError('read failed')
    return answer()
  }
  return {
    list: () => read(() => server.list()),
    createList: (key, name) => write(() => server.createList(key, name), true),
    renameList: (id, version, name) => write(() => server.renameList(id, version, name), true),
    deleteList: (id, version, todoCount) =>
      write(() => server.deleteList(id, version, todoCount), true),
    getList: (id) => read(() => server.getList(id)),
    addTodo: (listId, key, title) => write(() => server.addTodo(listId, key, title), true),
    markTodo: (listId, todoId, version, done) =>
      write(() => server.markTodo(listId, todoId, version, done), true),
    renameTodo: (listId, todoId, version, title) =>
      write(() => server.renameTodo(listId, todoId, version, title), true),
    deleteTodo: (listId, todoId, version) =>
      write(() => server.deleteTodo(listId, todoId, version), true),
  }
}

async function settle(engine: ListsEngine): Promise<void> {
  for (let i = 0; i < 2000; i++) {
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

function keys(prefix: string) {
  let next = 0
  return () => `${prefix}-${++next}`
}

/**
 * Wraps a tab's transport to check each accepted edit: its version must be one
 * this tab showed when the Member acted, or the result of its own last save.
 */
function watchVersions(
  server: MemoryListsServer,
  api: ListsApi,
  seen: Map<string, Set<number>>,
  pending: Map<string, Set<number>>,
  violations: string[]
): ListsApi {
  const alias = new Map<string, string>()
  const check = (listId: string, todoId: string, version: number, label: string) => {
    const todo = server.getList(listId).todos.find((candidate) => candidate.id === todoId)
    if (!todo) return undefined
    const versions = seen.get(todo.id) ?? new Set<number>()
    // A version noted while the todo had no ID belongs to it now (D7).
    for (const [key, noted] of pending) {
      if (alias.get(key) !== todo.id) continue
      for (const value of noted) versions.add(value)
      pending.delete(key)
    }
    seen.set(todo.id, versions)
    if (!versions.has(version)) violations.push(`${label} ${todo.title} at ${version}`)
    return versions
  }
  return {
    ...api,
    addTodo: async (listId, key, title) => {
      const answer = await api.addTodo(listId, key, title)
      alias.set(key, answer.id)
      return answer
    },
    markTodo: async (listId, todoId, version, done) => {
      const versions = check(listId, todoId, version, 'mark')
      const answer = await api.markTodo(listId, todoId, version, done)
      if (answer.version === version + 1) versions?.add(answer.version)
      return answer
    },
    renameTodo: async (listId, todoId, version, title) => {
      const versions = check(listId, todoId, version, 'rename')
      const answer = await api.renameTodo(listId, todoId, version, title)
      if (answer.version === version + 1) versions?.add(answer.version)
      return answer
    },
    deleteTodo: async (listId, todoId, version) => {
      check(listId, todoId, version, 'delete')
      return api.deleteTodo(listId, todoId, version)
    },
  }
}

async function runSeed(seed: number, refetchAdvancesQueue = false): Promise<void> {
  const rng = rngFor(seed)
  const server = new MemoryListsServer()
  const listId = server.seededList('Work', ['one', 'two'])
  // A second list: its saves hold the queue while list one is refetched.
  const otherId = server.seededList('Other', [])
  const faults = { reads: false }
  const violations: string[] = []
  const seenA = new Map<string, Set<number>>()
  const seenB = new Map<string, Set<number>>()
  const pendingA = new Map<string, Set<number>>()
  const pendingB = new Map<string, Set<number>>()
  const tabA = new ListsEngine({
    api: watchVersions(server, faultyApi(server, rng, faults), seenA, pendingA, violations),
    newKey: keys('a'),
    timeoutMs: Number.POSITIVE_INFINITY,
    refetchAdvancesQueue,
  })
  const tabB = new ListsEngine({
    api: watchVersions(server, faultyApi(server, rng, faults), seenB, pendingB, violations),
    newKey: keys('b'),
    timeoutMs: Number.POSITIVE_INFINITY,
    refetchAdvancesQueue,
  })
  await tabA.loadLists()
  await tabA.loadList(listId)
  await tabB.loadLists()
  await tabB.loadList(listId)
  faults.reads = true

  const remember = (tab: ListsEngine, todo: TodoView) => {
    const seen = tab === tabA ? seenA : seenB
    const pending = tab === tabA ? pendingA : pendingB
    const target = todo.id ? seen : pending
    const key = todo.id ?? todo.key
    if (!key) return
    if (!target.has(key)) target.set(key, new Set())
    target.get(key)!.add(todo.version)
  }
  const mark = (tab: ListsEngine, todo: TodoView) => {
    remember(tab, todo)
    tab.markTodo(listId, todo, !todo.done)
  }
  const rename = (tab: ListsEngine, todo: TodoView, title: string) => {
    remember(tab, todo)
    tab.renameTodo(listId, todo, title)
  }
  const remove = (tab: ListsEngine, todo: TodoView) => {
    remember(tab, todo)
    tab.deleteTodo(listId, todo)
  }
  for (let step = 0; step < 18; step++) {
    const tab = rng() < 0.5 ? tabA : tabB
    const other = tab === tabA ? tabB : tabA
    const todos = tab.snapshot().details[listId].todos
    const roll = rng()
    if (todos.length === 0 || roll < 0.3) {
      const key = tab.addTodo(listId, `t-${seed}-${step}`)
      // Sometimes mark the addition before its answer (design D7).
      if (rng() < 0.4) {
        const pending = tab
          .snapshot()
          .details[listId].todos.find((candidate) => candidate.key === key)
        if (pending) mark(tab, pending)
      }
    } else {
      const todo = todos[Math.floor(rng() * todos.length)]
      if (roll < 0.55) {
        mark(tab, todo)
      } else if (roll < 0.7 && todos.length > 2) {
        remove(tab, todo)
      } else if (roll < 0.85) {
        rename(tab, todo, `r-${seed}-${step}-${Math.floor(rng() * 1e6)}`)
        // A rename queued on a pending todo is sent after its add (D7).
      } else {
        tab.renameList(listId, `l-${seed}-${step}`)
      }
    }
    if (rng() < 0.3) tab.addTodo(otherId, `o-${seed}-${step}`)
    await yieldTicks(rng)
    if (rng() < 0.25) void tab.loadList(listId)
    if (rng() < 0.1) void other.loadList(listId)
  }

  await settle(tabA)
  await settle(tabB)
  faults.reads = false
  // Tabs aren't live: compare only after one final load each.
  await tabA.loadList(listId)
  await tabB.loadList(listId)

  expect(violations, `seed ${seed}: no unseen version`).toEqual([])
  const serverTodos = server.getList(listId).todos
  for (const id of [listId, otherId]) {
    const titles = server.getList(id).todos.map((todo) => todo.title)
    expect(new Set(titles).size, `seed ${seed}: no duplicated todo`).toBe(titles.length)
  }

  const shape = (todos: { id: string | null; title: string; done: boolean; version: number }[]) =>
    todos.map((todo) => [todo.id, todo.title, todo.done, todo.version])
  expect(shape(tabA.snapshot().details[listId].todos), `seed ${seed}: tab A`).toEqual(
    shape(serverTodos)
  )
  expect(shape(tabB.snapshot().details[listId].todos), `seed ${seed}: tab B`).toEqual(
    shape(serverTodos)
  )
  const serverName = server.getList(listId).list.name
  expect(tabA.snapshot().lists.find((list) => list.id === listId)?.name, `seed ${seed}`).toBe(
    serverName
  )
  expect(tabB.snapshot().lists.find((list) => list.id === listId)?.name, `seed ${seed}`).toBe(
    serverName
  )
}

describe('two-tab lists queue', () => {
  it(`converges for ${SEEDS} seeds`, async () => {
    for (let seed = 1; seed <= SEEDS; seed++) {
      await runSeed(seed)
    }
  }, 600_000)

  it('a refetch never advances a queued edit', async () => {
    expect(await refetchAdvancedQueuedVersion(false)).toBe(false)
  })

  it('the random test catches a refetch that advances queued edits', async () => {
    let caught = 0
    for (let seed = 1; seed <= 200; seed++) {
      await runSeed(seed, true).catch(() => {
        caught += 1
      })
    }
    expect(caught).toBeGreaterThan(0)
  }, 600_000)

  it('the broken variant is caught', async () => {
    // Documents the failure mode: with the flag on, a refetch rewrites the
    // version of a queued edit, so it no longer remembers what its tab saw.
    await expect(
      (async () => {
        if (await refetchAdvancedQueuedVersion(true)) {
          throw new Error('a refetch advanced a queued edit')
        }
      })()
    ).rejects.toThrow('a refetch advanced a queued edit')
  })
})

/**
 * Tab A queues a mark on list 1 behind a slow add to list 2, then refetches
 * list 1 after another tab has changed the todo. Returns true when the queued
 * edit's stored version moved with the refetch.
 */
async function refetchAdvancedQueuedVersion(refetchAdvancesQueue: boolean): Promise<boolean> {
  const server = new MemoryListsServer()
  const listOne = server.seededList('One', ['shared'])
  const listTwo = server.seededList('Two', [])
  const todo = server.getList(listOne).todos[0]
  const gate = Promise.withResolvers<void>()
  const tabA = new ListsEngine({
    api: {
      list: async () => server.list(),
      createList: async (key, name) => server.createList(key, name),
      renameList: async (id, version, name) => server.renameList(id, version, name),
      deleteList: async (id, version, todoCount) => server.deleteList(id, version, todoCount),
      getList: async (id) => server.getList(id),
      addTodo: async (listId, key, title) => {
        if (listId === listTwo) await gate.promise
        return server.addTodo(listId, key, title)
      },
      markTodo: async (listId, todoId, version, done) =>
        server.markTodo(listId, todoId, version, done),
      renameTodo: async (listId, todoId, version, title) =>
        server.renameTodo(listId, todoId, version, title),
      deleteTodo: async (listId, todoId, version) => server.deleteTodo(listId, todoId, version),
    },
    newKey: keys('a'),
    timeoutMs: Number.POSITIVE_INFINITY,
    refetchAdvancesQueue,
  })
  await tabA.loadLists()
  await tabA.loadList(listOne)

  // Another tab marks it done (version 2) after this tab loaded version 1.
  server.markTodo(listOne, todo.id, 1, true)

  // A slow save to another list holds the queue; the mark on list one waits.
  tabA.addTodo(listTwo, 'blocker')
  tabA.markTodo(listOne, { ...todo, version: 1, pending: false, done: false }, true)
  await tabA.loadList(listOne)
  const queued = tabA.snapshot().queue.find((op) => op.kind === 'mark_todo')
  const advanced = queued?.version !== 1
  gate.resolve()
  await settle(tabA)
  return advanced
}
