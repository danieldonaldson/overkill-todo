// The in-memory server must follow the Postgres adapter's rules, since the
// client tests and the client model's replay trust it (OT-013 design D13).

import { describe, expect, it } from 'vitest'
import { MemoryListsServer } from './memory-server'

describe('MemoryListsServer', () => {
  it('creates a new list for the key of a deleted list, as Postgres does', () => {
    const server = new MemoryListsServer()
    const first = server.createList('key-1', 'Work')
    server.deleteList(first.id, first.version, 0)

    const again = server.createList('key-1', 'Work')

    expect(again.id).not.toBe(first.id)
    expect(server.list().map((list) => list.id)).toEqual([again.id])
  })

  it('returns the stored list for a repeated key while it exists', () => {
    const server = new MemoryListsServer()
    const first = server.createList('key-1', 'Work')

    expect(server.createList('key-1', 'Work').id).toBe(first.id)
    expect(server.list()).toHaveLength(1)
  })

  it('refuses at the limits it was given', () => {
    const server = new MemoryListsServer({ lists: 1, todos: 1 })
    const list = server.createList('key-1', 'Work')
    server.addTodo(list.id, 'todo-1', 'milk')

    expect(() => server.createList('key-2', 'Home')).toThrow('at most 100 lists')
    expect(() => server.addTodo(list.id, 'todo-2', 'eggs')).toThrow('at most 1,000 todos')
  })
})
