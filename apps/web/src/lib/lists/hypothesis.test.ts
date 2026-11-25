// Two copies of the client queue against one in-memory server (OT-013 design
// Hypothesis). The random test checks that repeated saves are idempotent, that
// a tab never applies a version it did not see, and that after both queues
// drain a single refetch in each tab leaves both showing exactly the server's
// state. The deterministic case below shows the property fails if a refetch is
// allowed to advance a queued edit.

import { describe, expect, it } from 'vitest'
import { ApiErrorException } from '../api/errors'
import { ListsEngine, type ListsApi } from './engine'
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
function faultyApi(server: MemoryListsServer, rng: () => number): ListsApi {
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
  return {
    list: async () => {
      await yieldTicks(rng)
      return server.list()
    },
    createList: (key, name) => write(() => server.createList(key, name), true),
    getList: async (id) => {
      await yieldTicks(rng)
      return server.getList(id)
    },
    addTodo: (listId, key, title) => write(() => server.addTodo(listId, key, title), true),
    markTodo: (listId, todoId, version, done) =>
      write(() => server.markTodo(listId, todoId, version, done), true),
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

async function runSeed(seed: number): Promise<void> {
  const rng = rngFor(seed)
  const server = new MemoryListsServer()
  const listId = server.seededList('Work', ['one', 'two'])
  const tabA = new ListsEngine({
    api: faultyApi(server, rng),
    newKey: keys('a'),
    timeoutMs: Number.POSITIVE_INFINITY,
  })
  const tabB = new ListsEngine({
    api: faultyApi(server, rng),
    newKey: keys('b'),
    timeoutMs: Number.POSITIVE_INFINITY,
  })
  await tabA.loadLists()
  await tabA.loadList(listId)
  await tabB.loadLists()
  await tabB.loadList(listId)

  for (let step = 0; step < 18; step++) {
    const tab = rng() < 0.5 ? tabA : tabB
    const other = tab === tabA ? tabB : tabA
    const todos = tab.snapshot().details[listId].todos
    if (todos.length === 0 || rng() < 0.45) {
      const key = tab.addTodo(listId, `t-${seed}-${step}`)
      // Sometimes mark the addition before its answer (design D7).
      if (rng() < 0.4) {
        const pending = tab
          .snapshot()
          .details[listId].todos.find((candidate) => candidate.key === key)
        if (pending) tab.markTodo(listId, pending, true)
      }
    } else {
      const todo = todos[Math.floor(rng() * todos.length)]
      tab.markTodo(listId, todo, !todo.done)
    }
    if (rng() < 0.25) void tab.loadList(listId)
    if (rng() < 0.1) void other.loadList(listId)
  }

  await settle(tabA)
  await settle(tabB)
  // Tabs aren't live: compare only after one final load each.
  await tabA.loadList(listId)
  await tabB.loadList(listId)

  const serverTodos = server.getList(listId).todos
  const ids = serverTodos.map((todo) => todo.id)
  expect(new Set(ids).size, `seed ${seed}: no duplicated todo`).toBe(ids.length)

  const shape = (todos: { id: string | null; title: string; done: boolean; version: number }[]) =>
    todos.map((todo) => [todo.id, todo.title, todo.done, todo.version])
  expect(shape(tabA.snapshot().details[listId].todos), `seed ${seed}: tab A`).toEqual(
    shape(serverTodos)
  )
  expect(shape(tabB.snapshot().details[listId].todos), `seed ${seed}: tab B`).toEqual(
    shape(serverTodos)
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
      getList: async (id) => server.getList(id),
      addTodo: async (listId, key, title) => {
        if (listId === listTwo) await gate.promise
        return server.addTodo(listId, key, title)
      },
      markTodo: async (listId, todoId, version, done) =>
        server.markTodo(listId, todoId, version, done),
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
