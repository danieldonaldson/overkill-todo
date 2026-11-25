// In-memory lists server for the client tests and the two-tab random test.
// It follows the same rules as the Postgres adapter (OT-013 design D4/D5):
// creation keys are idempotent, a limit refuses, and a state change applies
// only at the version the caller saw. The same state already stored succeeds
// without a version change.

import { ApiErrorException } from '../api/errors'
import type { List, ListDetail, Todo } from '../types'
import type { ListsApi } from './engine'

interface StoredTodo {
  id: string
  title: string
  version: number
  done: boolean
  created_at: string
  completed_at?: string
}

interface StoredList {
  id: string
  name: string
  version: number
  created_at: string
  todos: StoredTodo[]
}

export const MAX_LISTS = 100
export const MAX_TODOS = 1000

const now = () => new Date().toISOString()

export class MemoryListsServer {
  private lists = new Map<string, StoredList>()
  private keys = new Map<string, string>()
  private nextId = 1

  createList(key: string, name: string): List {
    const existing = this.keys.get(`list:${key}`)
    if (existing) return this.summary(this.lists.get(existing)!)
    if (this.lists.size >= MAX_LISTS) {
      throw new ApiErrorException(
        { code: 'limit_reached', message: 'You can have at most 100 lists' },
        { status: 409, message: 'You can have at most 100 lists' }
      )
    }
    const id = `list-${this.nextId++}`
    const list: StoredList = {
      id,
      name: name.trim(),
      version: 1,
      created_at: now(),
      todos: [],
    }
    this.lists.set(id, list)
    this.keys.set(`list:${key}`, id)
    return this.summary(list)
  }

  addTodo(listId: string, key: string, title: string): Todo {
    const list = this.lists.get(listId)
    if (!list) throw notFound('list_not_found', 'List not found')
    const existing = this.keys.get(`todo:${listId}:${key}`)
    if (existing) {
      const todo = list.todos.find((candidate) => candidate.id === existing)
      if (todo) return this.todo(todo)
    }
    if (list.todos.length >= MAX_TODOS) {
      throw new ApiErrorException(
        { code: 'limit_reached', message: 'A list can hold at most 1,000 todos' },
        { status: 409, message: 'A list can hold at most 1,000 todos' }
      )
    }
    const id = `todo-${this.nextId++}`
    const todo: StoredTodo = {
      id,
      title: title.trim(),
      version: 1,
      done: false,
      created_at: now(),
    }
    // Open todos stay before the completed group.
    const doneAt = list.todos.findIndex((candidate) => candidate.done)
    if (doneAt === -1) list.todos.push(todo)
    else list.todos.splice(doneAt, 0, todo)
    this.keys.set(`todo:${listId}:${key}`, id)
    return this.todo(todo)
  }

  markTodo(listId: string, todoId: string, version: number, done: boolean): Todo {
    const list = this.lists.get(listId)
    if (!list) throw notFound('list_not_found', 'List not found')
    const todo = list.todos.find((candidate) => candidate.id === todoId)
    if (!todo) throw notFound('todo_not_found', 'Todo not found')
    if (todo.done === done) return this.todo(todo)
    if (todo.version !== version) {
      throw new ApiErrorException(
        { code: 'stale', message: 'This todo changed elsewhere', current: this.todo(todo) },
        { status: 409, message: 'This todo changed elsewhere' }
      )
    }
    todo.done = done
    todo.version += 1
    todo.completed_at = done ? now() : undefined
    list.todos = list.todos.filter((candidate) => candidate.id !== todoId)
    const doneAt = list.todos.findIndex((candidate) => candidate.done)
    list.todos.splice(doneAt === -1 ? list.todos.length : doneAt, 0, todo)
    return this.todo(todo)
  }

  list(): List[] {
    return [...this.lists.values()].map((list) => this.summary(list))
  }

  getList(id: string): ListDetail {
    const list = this.lists.get(id)
    if (!list) throw notFound('list_not_found', 'List not found')
    return {
      list: this.summary(list),
      todos: list.todos.map((todo) => this.todo(todo)),
    }
  }

  /** The server's current contract state, for a final comparison. */
  state(): { lists: List[]; todos: Record<string, Todo[]> } {
    const todos: Record<string, Todo[]> = {}
    for (const list of this.lists.values())
      todos[list.id] = list.todos.map((todo) => this.todo(todo))
    return { lists: this.list(), todos }
  }

  /** Test helper: another tab deletes a todo. */
  removeTodo(listId: string, todoId: string): void {
    const list = this.lists.get(listId)
    if (list) list.todos = list.todos.filter((todo) => todo.id !== todoId)
  }

  /** Test helper: another tab deletes a list. */
  removeList(listId: string): void {
    this.lists.delete(listId)
  }

  seededList(name: string, titles: string[]): string {
    const list = this.createList(`seed-${this.nextId}`, name)
    for (const title of titles) this.addTodo(list.id, `seed-${this.nextId}`, title)
    return list.id
  }

  private summary(list: StoredList): List {
    return {
      id: list.id,
      name: list.name,
      version: list.version,
      open_count: list.todos.filter((todo) => !todo.done).length,
      todo_count: list.todos.length,
    }
  }

  private todo(todo: StoredTodo): Todo {
    return {
      id: todo.id,
      title: todo.title,
      version: todo.version,
      done: todo.done,
      created_at: todo.created_at,
      completed_at: todo.completed_at,
    }
  }
}

function notFound(code: string, message: string): ApiErrorException {
  return new ApiErrorException({ code, message }, { status: 404, message })
}

/** A tab's transport onto the shared server, with optional injected faults. */
export function serverApi(
  server: MemoryListsServer,
  between: () => Promise<void> = async () => {}
): ListsApi {
  return {
    list: async () => {
      await between()
      return server.list()
    },
    createList: async (key, name) => {
      await between()
      return server.createList(key, name)
    },
    getList: async (id) => {
      await between()
      return server.getList(id)
    },
    addTodo: async (listId, key, title) => {
      await between()
      return server.addTodo(listId, key, title)
    },
    markTodo: async (listId, todoId, version, done) => {
      await between()
      return server.markTodo(listId, todoId, version, done)
    },
  }
}
