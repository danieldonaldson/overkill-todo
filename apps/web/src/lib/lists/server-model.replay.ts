// Replays models/list_writes.qnt's traces through MemoryListsServer (OT-013
// task 7.6), which the client tests and the client model's replay trust.
//
// Only what the memory server implements: one Member per server (each model
// Member gets their own, so another Member's IDs are simply unknown there), and
// no purge, sweep or purged account: a trace is replayed up to its first such
// step. Each request is applied at its `commit`; `find_list` isn't replayed.
// Limits are the model's (2 lists, 3 todos). After every step each server's
// lists and todos must equal the model's, versions and open order included.

import { readdirSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ApiErrorException } from '../api/errors'
import { MemoryListsServer, type SeedList } from './memory-server'

interface ModelList {
  used: boolean
  exists: boolean
  owner: number
  key: number
  name: number
  version: number
}
interface ModelTodo {
  used: boolean
  exists: boolean
  list: number
  key: number
  title: number
  version: number
  done: boolean
  pos: number
}
interface ModelWriter {
  op: string
  member: number
  list: number
  todo: number
  key: number
  version: number
  text: number
  done: boolean
  count: number
  above: number
  below: number
}
interface State {
  deleted: Map<number, boolean>
  lists: Map<number, ModelList>
  todos: Map<number, ModelTodo>
  writers: Map<number, ModelWriter>
  last: { act: string; writer: number; op: string; outcome: string; id: number }
}

/** Decodes ITF values: big integers, maps and records. */
function decode(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(decode)
  if (value && typeof value === 'object') {
    const object = value as Record<string, unknown>
    if ('#bigint' in object) return Number(object['#bigint'])
    if ('#map' in object)
      return new Map(
        (object['#map'] as [unknown, unknown][]).map(([k, v]) => [decode(k), decode(v)])
      )
    return Object.fromEntries(
      Object.entries(object).map(([key, v]) => [key.split('::').pop()!, decode(v)])
    )
  }
  return value
}

const MEMBERS = [1, 2]
const name = (n: number) => `name ${n}`
const title = (n: number) => `title ${n}`

/** What a call to the memory server came to, in the model's terms. */
function outcomeOf(error: unknown, op: string): string {
  if (!(error instanceof ApiErrorException)) throw error
  if (error.status === 404) return error.code === 'todo_not_found' ? 'todoNotFound' : 'listNotFound'
  if (error.code === 'limit_reached') return op === 'createList' ? 'listLimit' : 'todoLimit'
  if (error.code === 'stale') return 'stale'
  throw error
}

function replay(file: string, coverage: Map<string, number>): void {
  const trace = JSON.parse(readFileSync(file, 'utf8')) as { states: unknown[] }
  const states = trace.states.map((state) => decode(state) as State)
  const at = (index: number, what: string) => `${file} step ${index} ${what}`

  // Model IDs and keys to memory ones, per Member.
  const listIds = new Map<number, string>()
  const todoIds = new Map<number, string>()
  for (const l of states[0].lists.keys()) listIds.set(l, `model-list-${l}`)
  for (const t of states[0].todos.keys()) todoIds.set(t, `model-todo-${t}`)
  const servers = new Map(
    MEMBERS.map((m) => [m, new MemoryListsServer({ lists: 2, todos: 3, idPrefix: `member-${m}-` })])
  )

  const first = states[0]
  for (const m of MEMBERS) {
    const seeds: SeedList[] = [...first.lists]
      .filter(([, list]) => list.exists && list.owner === m)
      .map(([l, list]) => ({
        id: listIds.get(l)!,
        key: `key-${list.key}`,
        name: name(list.name),
        version: list.version,
        todos: [...first.todos]
          .filter(([, todo]) => todo.exists && todo.list === l)
          .sort(([, a], [, b]) => a.pos - b.pos)
          .map(([t, todo]) => ({
            id: todoIds.get(t)!,
            key: `key-${todo.key}`,
            title: title(todo.title),
            version: todo.version,
            done: todo.done,
          })),
      }))
    servers.get(m)!.restore(seeds)
  }

  const check = (state: State, where: string) => {
    for (const m of MEMBERS) {
      const stored = servers.get(m)!.state()
      const lists = [...state.lists].filter(([, list]) => list.exists && list.owner === m)
      expect(stored.lists.map((list) => list.id).sort(), `${where}: Member ${m}'s lists`).toEqual(
        lists.map(([l]) => listIds.get(l)!).sort()
      )
      for (const [l, list] of lists) {
        const id = listIds.get(l)!
        const summary = stored.lists.find((candidate) => candidate.id === id)!
        expect(summary.name, `${where}: list ${l} name`).toBe(name(list.name))
        expect(summary.version, `${where}: list ${l} version`).toBe(list.version)
        const todos = [...state.todos].filter(([, todo]) => todo.exists && todo.list === l)
        const open = todos.filter(([, todo]) => !todo.done).sort(([, a], [, b]) => a.pos - b.pos)
        const real = stored.todos[id]
        expect(
          real.filter((todo) => !todo.done).map((todo) => todo.id),
          `${where}: list ${l} open order`
        ).toEqual(open.map(([t]) => todoIds.get(t)!))
        expect(real.map((todo) => todo.id).sort(), `${where}: list ${l} todos`).toEqual(
          todos.map(([t]) => todoIds.get(t)!).sort()
        )
        for (const [t, todo] of todos) {
          const row = real.find((candidate) => candidate.id === todoIds.get(t))!
          expect([row.title, row.version, row.done], `${where}: todo ${t}`).toEqual([
            title(todo.title),
            todo.version,
            todo.done,
          ])
        }
      }
    }
  }
  check(first, `${file} initial state`)

  for (let index = 1; index < states.length; index++) {
    const before = states[index - 1]
    const step = states[index].last
    if (step.act !== 'commit' || step.op === 'findList') {
      // Only `sweep` changes rows outside a commit, and it isn't implemented.
      if (step.act === 'sweep') return
      continue
    }
    const request = before.writers.get(step.writer)!
    // The memory server has no purge, no sweep and no purged accounts.
    if (step.op === 'purge' || step.outcome === 'bound' || before.deleted.get(request.member))
      return

    const server = servers.get(request.member)!
    const list = listIds.get(request.list) ?? 'unknown-list'
    const todo = todoIds.get(request.todo) ?? 'unknown-todo'
    const neighbours = (t: number) => (t === 0 ? [] : [todoIds.get(t)!])
    let outcome = 'success'
    let answered: string | undefined
    try {
      switch (request.op) {
        case 'createList':
          answered = server.createList(`key-${request.key}`, name(request.text)).id
          break
        case 'renameList':
          server.renameList(list, request.version, name(request.text))
          break
        case 'deleteList':
          server.deleteList(list, request.version, request.count)
          break
        case 'addTodo':
          answered = server.addTodo(list, `key-${request.key}`, title(request.text)).id
          break
        case 'renameTodo':
          server.renameTodo(list, todo, request.version, title(request.text))
          break
        case 'markTodo':
          server.markTodo(list, todo, request.version, request.done)
          break
        case 'deleteTodo':
          server.deleteTodo(list, todo, request.version)
          break
        case 'moveTodo': {
          const order = server.moveTodo(
            list,
            todo,
            neighbours(request.above),
            neighbours(request.below)
          )
          const expected = [...states[index].todos]
            .filter(([, t]) => t.exists && !t.done && t.list === request.list)
            .sort(([, a], [, b]) => a.pos - b.pos)
            .map(([t]) => todoIds.get(t)!)
          expect(order, at(index, 'order answered')).toEqual(expected)
          break
        }
        default:
          throw new Error(`${at(index, request.op)}: not a memory server operation`)
      }
    } catch (error) {
      outcome = outcomeOf(error, request.op)
    }

    const model = ['applied', 'repeated', 'moved'].includes(step.outcome) ? 'success' : step.outcome
    expect(outcome, at(index, `${request.op} outcome`)).toBe(model)
    coverage.set(step.outcome, (coverage.get(step.outcome) ?? 0) + 1)
    if (answered !== undefined) {
      if (step.outcome === 'applied') {
        if (request.op === 'createList') listIds.set(step.id, answered)
        else todoIds.set(step.id, answered)
      } else {
        const known = request.op === 'createList' ? listIds.get(step.id) : todoIds.get(step.id)
        expect(answered, at(index, 'row answered')).toBe(known)
      }
    }
    check(states[index], at(index, request.op))
  }
}

describe('MemoryListsServer follows models/list_writes.qnt', () => {
  it('replays every exported trace for the operations it implements', () => {
    const dir = process.env.MODEL_TRACES_DIR
    if (!dir) throw new Error('MODEL_TRACES_DIR is unset: run this replay with `just models`')
    const files = readdirSync(dir)
      .filter((file) => file.startsWith('list_writes') && file.endsWith('.itf.json'))
      .sort()
    expect(files.length, `no list_writes traces in ${dir}`).toBeGreaterThan(0)
    const coverage = new Map<string, number>()
    for (const file of files) replay(join(dir, file), coverage)
    console.log(
      `list_writes on the memory server: ${files.length} traces,`,
      Object.fromEntries(coverage)
    )
    for (const outcome of [
      'applied',
      'repeated',
      'stale',
      'listLimit',
      'todoLimit',
      'listNotFound',
      'todoNotFound',
      'moved',
    ])
      expect(coverage.get(outcome) ?? 0, `no trace reached ${outcome}`).toBeGreaterThan(0)
  })
})
