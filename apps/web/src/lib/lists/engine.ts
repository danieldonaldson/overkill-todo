// Lists client engine (OT-013 design D7).
//
// The screen is the server's confirmed state with this tab's queued operations
// applied. One save is sent at a time, in order, so rapid edits never conflict
// with each other; a refetch never advances a queued operation, so another
// tab's change can't be turned into a silent overwrite.

import { ApiErrorException } from '../api/errors'
import type { List, ListDetail, Todo } from '../types'

/** The API surface the engine needs. Tests substitute an in-memory server. */
export interface ListsApi {
  list(): Promise<List[]>
  createList(key: string, name: string): Promise<List>
  getList(id: string): Promise<ListDetail>
  addTodo(listId: string, key: string, title: string): Promise<Todo>
  markTodo(listId: string, todoId: string, version: number, done: boolean): Promise<Todo>
}

export interface ListView {
  id: string | null
  /** The client key while a new list is unsaved. */
  key?: string
  name: string
  version: number
  open_count: number
  todo_count: number
  pending: boolean
}

export interface TodoView {
  id: string | null
  /** The client key while a new todo is unsaved. */
  key?: string
  title: string
  version: number
  done: boolean
  created_at?: string
  completed_at?: string
  pending: boolean
}

export type DetailStatus = 'idle' | 'loading' | 'ready' | 'error' | 'not_found'
export type ListsStatus = 'idle' | 'loading' | 'ready' | 'error'

export interface ListDetailView {
  status: DetailStatus
  todos: TodoView[]
}

export interface Snapshot {
  lists: ListView[]
  listsStatus: ListsStatus
  details: Record<string, ListDetailView>
  queueLength: number
  /** Queued operations in order, for tests and the broken-variant check. */
  queue: { kind: Operation['kind']; version?: number }[]
  saving: boolean
  /** Set after a new list is saved, for the address replacement (D3). */
  lastCreatedListId: string | null
}

export interface NoticeRestore {
  target: 'list' | 'todo'
  listId?: string
  text: string
}

export interface Notice {
  text: string
  restore?: NoticeRestore
}

type FailureKind = 'stale' | 'not_found' | 'limit' | 'invalid' | 'unknown'

interface Failure {
  kind: FailureKind
  current?: Todo
  message?: string
}

type SendResult = { ok: true; value: List | Todo } | { ok: false; error: Failure }

type Operation =
  | { kind: 'create_list'; key: string; name: string }
  | { kind: 'add_todo'; key: string; listId: string; title: string }
  | {
      kind: 'mark_todo'
      listId: string
      todoId?: string
      todoKey?: string
      done: boolean
      /** The version on screen when the Member acted. */
      version: number
    }

export interface EngineOptions {
  api: ListsApi
  newKey?: () => string
  /** A request still unanswered after this long has an unknown outcome. */
  timeoutMs?: number
  /** Test-only: let a refetch advance queued edits (the broken variant). */
  refetchAdvancesQueue?: boolean
  onChange?: (snapshot: Snapshot) => void
  onNotice?: (notice: Notice) => void
}

const DEFAULT_TIMEOUT_MS = 15_000

const itemKey = (op: Operation): string => {
  switch (op.kind) {
    case 'create_list':
      return `list-key:${op.key}`
    case 'add_todo':
      return `todo-key:${op.key}`
    case 'mark_todo':
      return op.todoId ? `todo-id:${op.todoId}` : `todo-key:${op.todoKey}`
  }
}

function insertTodo(todos: TodoView[], todo: TodoView): void {
  if (!todo.done) {
    const doneAt = todos.findIndex((t) => t.done)
    if (doneAt === -1) todos.push(todo)
    else todos.splice(doneAt, 0, todo)
  } else {
    todos.unshift(todo)
  }
}

function removeTodo(
  todos: TodoView[],
  target: { id?: string; key?: string }
): TodoView | undefined {
  const index = todos.findIndex(
    (t) => (target.id != null && t.id === target.id) || (target.key != null && t.key === target.key)
  )
  if (index === -1) return undefined
  return todos.splice(index, 1)[0]
}

function moveTodo(todos: TodoView[], todo: TodoView): void {
  const index = todos.findIndex((t) =>
    todo.id != null ? t.id === todo.id : t.key != null && t.key === todo.key
  )
  if (index !== -1) todos.splice(index, 1)
  insertTodo(todos, todo)
}

const toListView = (list: List): ListView => ({
  id: list.id,
  name: list.name,
  version: list.version,
  open_count: list.open_count,
  todo_count: list.todo_count,
  pending: false,
})

const toTodoView = (todo: Todo, key?: string): TodoView => ({
  id: todo.id,
  key,
  title: todo.title,
  version: todo.version,
  done: todo.done,
  created_at: todo.created_at,
  completed_at: todo.completed_at,
  pending: false,
})

export class ListsEngine {
  private readonly api: ListsApi
  private readonly newKey: () => string
  private readonly timeoutMs: number
  private readonly refetchAdvancesQueue: boolean
  private readonly onChange: (snapshot: Snapshot) => void
  private readonly onNotice: (notice: Notice) => void

  private lists: ListView[] = []
  private listsStatus: ListsStatus = 'idle'
  private details = new Map<string, { status: DetailStatus; todos: TodoView[] }>()
  private queue: Operation[] = []
  private running = false
  private epoch = 0
  private savesInFlight = 0
  private globalGeneration = 0
  private listGenerations = new Map<string, number>()
  private inFlightByList = new Map<string, number>()
  private listReloads = new Set<string>()
  private listsReloadPending = false
  private recoveryNeeded = false
  private lastCreatedListId: string | null = null

  constructor(options: EngineOptions) {
    this.api = options.api
    this.newKey = options.newKey ?? (() => crypto.randomUUID())
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS
    this.refetchAdvancesQueue = options.refetchAdvancesQueue ?? false
    this.onChange = options.onChange ?? (() => {})
    this.onNotice = options.onNotice ?? (() => {})
  }

  snapshot(): Snapshot {
    return {
      lists: this.listsView(),
      listsStatus: this.listsStatus,
      details: this.viewDetails(),
      queueLength: this.queue.length,
      queue: this.queue.map((op) => ({
        kind: op.kind,
        version: 'version' in op ? op.version : undefined,
      })),
      saving: this.savesInFlight > 0,
      lastCreatedListId: this.lastCreatedListId,
    }
  }

  /** Submit a new list and return its client key. */
  createList(name: string): string {
    const key = this.newKey()
    this.queue.push({ kind: 'create_list', key, name })
    this.notify()
    this.kick()
    return key
  }

  /** Add a todo at the bottom of a saved list and return its client key. */
  addTodo(listId: string, title: string): string {
    const key = this.newKey()
    this.queue.push({ kind: 'add_todo', key, listId, title })
    this.notify()
    this.kick()
    return key
  }

  /** Mark a todo done or not done, at the version currently on screen. */
  markTodo(listId: string, todo: TodoView, done: boolean): void {
    if (todo.done === done) return
    this.queue.push({
      kind: 'mark_todo',
      listId,
      ...(todo.id ? { todoId: todo.id } : { todoKey: todo.key }),
      done,
      version: todo.version,
    })
    this.notify()
    this.kick()
  }

  /** Load the caller's lists (beside the queue; a stale answer is dropped). */
  async loadLists(): Promise<void> {
    const epoch = this.epoch
    const generation = ++this.globalGeneration
    this.listsStatus = 'loading'
    this.notify()
    try {
      const lists = await this.api.list()
      if (epoch !== this.epoch) return
      if (generation !== this.globalGeneration || this.savesInFlight > 0) {
        this.listsReloadPending = true
        return
      }
      this.lists = lists.map(toListView)
      this.listsStatus = 'ready'
    } catch {
      if (epoch !== this.epoch) return
      if (generation !== this.globalGeneration || this.savesInFlight > 0) {
        this.listsReloadPending = true
        return
      }
      this.listsStatus = 'error'
    } finally {
      if (epoch === this.epoch) {
        this.notify()
        this.flushReloads()
      }
    }
  }

  /** Load one list and its todos (beside the queue). */
  async loadList(id: string): Promise<void> {
    const epoch = this.epoch
    const generation = this.bump(id)
    const previous = this.details.get(id)
    this.details.set(id, { status: 'loading', todos: previous?.todos ?? [] })
    this.notify()
    try {
      const detail = await this.api.getList(id)
      if (epoch !== this.epoch) return
      if (generation !== this.generation(id) || this.inFlight(id) > 0) {
        this.listReloads.add(id)
        return
      }
      const list = toListView(detail.list)
      const index = this.lists.findIndex((candidate) => candidate.id === id)
      if (index === -1) this.lists.push(list)
      else this.lists[index] = list
      this.details.set(id, {
        status: 'ready',
        todos: detail.todos.map((todo) => toTodoView(todo)),
      })
      if (this.refetchAdvancesQueue) this.advanceQueuedVersions(id, detail.todos)
    } catch {
      if (epoch !== this.epoch) return
      if (generation !== this.generation(id) || this.inFlight(id) > 0) {
        this.listReloads.add(id)
        return
      }
      if (error instanceof ApiErrorException && error.status === 404) {
        this.details.set(id, { status: 'not_found', todos: [] })
        this.lists = this.lists.filter((list) => list.id !== id)
      } else {
        this.details.set(id, { status: 'error', todos: previous?.todos ?? [] })
      }
    } finally {
      if (epoch === this.epoch) {
        this.notify()
        this.flushReloads()
      }
    }
  }

  /** Refetch after an unknown outcome, once the server is reachable again. */
  retry(): void {
    if (!this.recoveryNeeded) return
    this.recoveryNeeded = false
    if (this.listsStatus !== 'idle') void this.loadLists()
    for (const [id, detail] of this.details) {
      if (detail.status === 'ready' || detail.status === 'error') void this.loadList(id)
    }
  }

  /** Forget everything: sign-out or a different account signing in. */
  reset(): void {
    this.epoch += 1
    this.lists = []
    this.listsStatus = 'idle'
    this.details.clear()
    this.queue = []
    this.listGenerations.clear()
    this.inFlightByList.clear()
    this.listReloads.clear()
    this.listsReloadPending = false
    this.recoveryNeeded = false
    this.lastCreatedListId = null
    this.savesInFlight = 0
    this.notify()
  }

  private generation(id: string): number {
    return this.listGenerations.get(id) ?? 0
  }

  private bump(id: string): number {
    const next = this.generation(id) + 1
    this.listGenerations.set(id, next)
    return next
  }

  private inFlight(id: string): number {
    return this.inFlightByList.get(id) ?? 0
  }

  private setInFlight(id: string, delta: number): void {
    const next = Math.max(0, this.inFlight(id) + delta)
    if (next === 0) this.inFlightByList.delete(id)
    else this.inFlightByList.set(id, next)
  }

  private viewDetails(): Record<string, ListDetailView> {
    const view: Record<string, ListDetailView> = {}
    for (const [id, detail] of this.details) {
      const todos = detail.todos.map((todo) => ({ ...todo }))
      if (detail.status === 'ready') this.applyQueueToTodos(id, todos)
      view[id] = { status: detail.status, todos }
    }
    return view
  }

  private listsView(): ListView[] {
    const lists = this.lists.map((list) => ({ ...list }))
    for (const [id, detail] of this.details) {
      if (detail.status !== 'ready') continue
      const todos = detail.todos.map((todo) => ({ ...todo }))
      this.applyQueueToTodos(id, todos)
      const index = lists.findIndex((list) => list.id === id)
      if (index !== -1) {
        lists[index].open_count = todos.filter((todo) => !todo.done).length
        lists[index].todo_count = todos.length
      }
    }
    for (const op of this.queue) {
      if (op.kind !== 'create_list') continue
      lists.push({
        id: null,
        key: op.key,
        name: op.name,
        version: 1,
        open_count: 0,
        todo_count: 0,
        pending: true,
      })
    }
    return lists
  }

  private applyQueueToTodos(listId: string, todos: TodoView[]): void {
    for (const op of this.queue) {
      if (op.kind === 'add_todo' && op.listId === listId) {
        todos.push({
          id: null,
          key: op.key,
          title: op.title,
          version: 1,
          done: false,
          pending: true,
        })
        continue
      }
      if (op.kind !== 'mark_todo' || op.listId !== listId) continue
      const target = todos.find(
        (todo) =>
          (op.todoId != null && todo.id === op.todoId) ||
          (op.todoKey != null && todo.key === op.todoKey)
      )
      if (!target) continue
      target.done = op.done
      target.version = op.version
      target.completed_at = op.done ? new Date().toISOString() : undefined
      moveTodo(todos, target)
    }
  }

  private notify(): void {
    this.onChange(this.snapshot())
  }

  private kick(): void {
    if (this.running) return
    this.running = true
    const epoch = this.epoch
    void this.drain(epoch).finally(() => {
      this.running = false
      if (this.epoch === epoch && this.queue.length > 0) this.kick()
    })
  }

  private async drain(epoch: number): Promise<void> {
    while (this.queue.length > 0 && this.epoch === epoch) {
      const op = this.queue[0]
      const listId = op.kind === 'create_list' ? null : op.listId
      if (listId) this.bump(listId)
      this.globalGeneration += 1
      this.savesInFlight += 1
      if (listId) this.setInFlight(listId, 1)
      this.notify()

      const result = await this.sendWithRepeat(op)
      if (this.epoch !== epoch) return

      this.savesInFlight -= 1
      this.globalGeneration += 1
      if (listId) {
        this.setInFlight(listId, -1)
        this.bump(listId)
      }
      if (result.ok) this.applySuccess(op, result.value)
      else this.applyFailure(op, result.error)
      this.notify()
      this.flushReloads()
    }
  }

  private async sendWithRepeat(op: Operation): Promise<SendResult> {
    const first = await this.send(op)
    // One retry with the same key or version, so a lost answer can't create a
    // duplicate or apply twice.
    if (first.ok || first.error.kind !== 'unknown') return first
    return this.send(op)
  }

  private async send(op: Operation): Promise<SendResult> {
    try {
      switch (op.kind) {
        case 'create_list':
          return { ok: true, value: await this.timeout(this.api.createList(op.key, op.name)) }
        case 'add_todo':
          return {
            ok: true,
            value: await this.timeout(this.api.addTodo(op.listId, op.key, op.title)),
          }
        case 'mark_todo': {
          const todoId = op.todoId ?? this.resolveTodoKey(op.todoKey)
          if (!todoId) return { ok: false, error: { kind: 'not_found' } }
          return {
            ok: true,
            value: await this.timeout(this.api.markTodo(op.listId, todoId, op.version, op.done)),
          }
        }
      }
    } catch (error) {
      return { ok: false, error: this.classify(error) }
    }
  }

  private resolveTodoKey(key: string | undefined): string | undefined {
    if (!key) return undefined
    for (const detail of this.details.values()) {
      const todo = detail.todos.find((candidate) => candidate.key === key && candidate.id)
      if (todo?.id) return todo.id
    }
    return undefined
  }

  private timeout<T>(promise: Promise<T>): Promise<T> {
    if (!Number.isFinite(this.timeoutMs)) return promise
    const { promise: timedOut, resolve, reject } = Promise.withResolvers<T>()
    const timer = setTimeout(() => reject(new Error('request timed out')), this.timeoutMs)
    promise.then(
      (value) => {
        clearTimeout(timer)
        resolve(value)
      },
      (error: unknown) => {
        clearTimeout(timer)
        reject(error instanceof Error ? error : new Error(String(error)))
      }
    )
    return timedOut
  }

  private classify(error: unknown): Failure {
    if (error instanceof ApiErrorException) {
      if (error.status === 409 && error.code === 'stale') {
        return { kind: 'stale', current: error.current as Todo | undefined }
      }
      if (error.status === 409) return { kind: 'limit', message: error.message }
      if (error.status === 404) return { kind: 'not_found' }
      if (error.status === 400) return { kind: 'invalid', message: error.message }
      if ((error.status ?? 0) >= 500) return { kind: 'unknown' }
      return { kind: 'invalid', message: error.message }
    }
    return { kind: 'unknown' }
  }

  private applySuccess(op: Operation, value: List | Todo): void {
    this.recoveryNeeded = false
    switch (op.kind) {
      case 'create_list': {
        const list = value as List
        this.lists.push(toListView(list))
        this.lastCreatedListId = list.id
        this.details.set(list.id, { status: 'idle', todos: [] })
        break
      }
      case 'add_todo': {
        const todo = value as Todo
        const detail = this.details.get(op.listId)
        if (detail) detail.todos.push(toTodoView(todo, op.key))
        break
      }
      case 'mark_todo': {
        const todo = toTodoView(value as Todo, this.keyOf(op))
        const detail = this.details.get(op.listId)
        if (detail) moveTodo(detail.todos, todo)
        this.rebaseQueuedVersions(op, todo.version)
        break
      }
    }
    this.queue.shift()
  }

  private keyOf(op: Extract<Operation, { kind: 'mark_todo' }>): string | undefined {
    if (op.todoKey) return op.todoKey
    for (const detail of this.details.values()) {
      const todo = detail.todos.find((candidate) => candidate.id === op.todoId)
      if (todo?.key) return todo.key
    }
    return undefined
  }

  /** A save that moved `n → n + 1` moves later queued edits of it to `n + 1`. */
  private rebaseQueuedVersions(
    op: Extract<Operation, { kind: 'mark_todo' }>,
    newVersion: number
  ): void {
    const oldVersion = op.version
    if (newVersion !== oldVersion + 1) return
    const key = itemKey(op)
    for (const queued of this.queue) {
      if (queued === op) continue
      if (queued.kind === 'mark_todo' && itemKey(queued) === key && queued.version === oldVersion) {
        queued.version = newVersion
      }
    }
  }

  private applyFailure(op: Operation, failure: Failure): void {
    const key = itemKey(op)
    this.queue.shift()
    // Later edits of the same item depend on this one; drop them together and
    // keep a single message.
    this.queue = this.queue.filter((queued) => itemKey(queued) !== key)

    switch (failure.kind) {
      case 'stale': {
        if (failure.current) {
          const todo = toTodoView(failure.current)
          const detail = op.kind === 'create_list' ? undefined : this.details.get(op.listId)
          if (detail) {
            const existing = detail.todos.find((candidate) => candidate.id === todo.id)
            if (existing?.key) todo.key = existing.key
            moveTodo(detail.todos, todo)
          }
        }
        this.onNotice({ text: 'This changed elsewhere. The current version is shown.' })
        break
      }
      case 'not_found': {
        if (op.kind === 'add_todo' || op.kind === 'mark_todo') {
          const detail = this.details.get(op.listId)
          if (detail) {
            if (op.kind === 'mark_todo') {
              removeTodo(detail.todos, { id: op.todoId, key: op.todoKey })
            } else {
              removeTodo(detail.todos, { key: op.key })
            }
          }
          if (op.kind === 'add_todo') {
            this.lists = this.lists.filter((list) => list.id !== op.listId)
            this.details.set(op.listId, { status: 'not_found', todos: [] })
            this.onNotice({ text: 'This list was deleted elsewhere.' })
          } else {
            this.onNotice({ text: 'This todo was deleted elsewhere.' })
          }
        } else {
          this.onNotice({ text: 'This list was deleted elsewhere.' })
        }
        break
      }
      case 'limit':
      case 'invalid': {
        this.onNotice({
          text: failure.message ?? 'That change could not be saved.',
          ...(op.kind === 'add_todo'
            ? { restore: { target: 'todo' as const, listId: op.listId, text: op.title } }
            : {}),
          ...(op.kind === 'create_list'
            ? { restore: { target: 'list' as const, text: op.name } }
            : {}),
        })
        break
      }
      case 'unknown': {
        this.onNotice({
          text: "Couldn't save your change. The list will refresh when the server is reachable.",
          ...(op.kind === 'add_todo'
            ? { restore: { target: 'todo' as const, listId: op.listId, text: op.title } }
            : {}),
          ...(op.kind === 'create_list'
            ? { restore: { target: 'list' as const, text: op.name } }
            : {}),
        })
        this.recoveryNeeded = true
        if (op.kind !== 'create_list') this.listReloads.add(op.listId)
        else this.listsReloadPending = true
        break
      }
    }
  }

  /** Test-only broken variant: a refetch advances queued edits' versions. */
  private advanceQueuedVersions(listId: string, todos: Todo[]): void {
    for (const op of this.queue) {
      if (op.kind !== 'mark_todo' || op.listId !== listId) continue
      const current = todos.find(
        (todo) =>
          (op.todoId != null && todo.id === op.todoId) ||
          (op.todoKey != null &&
            this.details.get(listId)?.todos.find((candidate) => candidate.key === op.todoKey)
              ?.id === todo.id)
      )
      if (current) op.version = current.version
    }
  }

  private flushReloads(): void {
    if (this.savesInFlight > 0) return
    if (this.listsReloadPending) {
      this.listsReloadPending = false
      void this.loadLists()
    }
    for (const id of [...this.listReloads]) {
      if (this.inFlight(id) > 0) continue
      this.listReloads.delete(id)
      void this.loadList(id)
    }
  }
}
