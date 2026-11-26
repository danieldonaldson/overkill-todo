// Lists client engine (OT-013 design D7).
//
// The screen is the server's confirmed state with this tab's queued operations
// applied. One save is sent at a time, in order, so rapid edits never conflict
// with each other; a refetch never advances a queued operation, so another
// tab's change can't be turned into a silent overwrite.

import { ApiErrorException, SessionEndedError } from '../api/errors'
import type { List, ListDetail, Todo } from '../types'

/** The API surface the engine needs. Tests substitute an in-memory server. */
export interface ListsApi {
  list(): Promise<List[]>
  createList(key: string, name: string): Promise<List>
  renameList(id: string, version: number, name: string): Promise<List>
  deleteList(id: string, version: number, todoCount: number): Promise<void>
  getList(id: string): Promise<ListDetail>
  addTodo(listId: string, key: string, title: string): Promise<Todo>
  markTodo(listId: string, todoId: string, version: number, done: boolean): Promise<Todo>
  renameTodo(listId: string, todoId: string, version: number, title: string): Promise<Todo>
  deleteTodo(listId: string, todoId: string, version: number): Promise<void>
  /** Neighbours are nearest first. Answers the open todos' IDs in order. */
  moveTodo(listId: string, todoId: string, above: string[], below: string[]): Promise<string[]>
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
  /** Stays `ready` during a refresh: confirmed todos are never hidden again. */
  status: DetailStatus
  /** The last refresh failed, so the todos may be out of date. */
  refreshFailed: boolean
  todos: TodoView[]
}

interface Detail {
  status: DetailStatus
  /** The server has answered for this list, so `todos` is its confirmed state. */
  loaded: boolean
  refreshFailed: boolean
  /** When the server last answered about this list (see `clock`). */
  knownAt: number
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
  /** Lists with a delete awaiting the server (deletion is not optimistic). */
  deleting: string[]
  /** The newest list this tab saved, by its client key, to open it (D3). */
  lastCreated: { key: string; id: string } | null
}

export interface NoticeRestore {
  target: 'list' | 'todo'
  /** A create resubmits into an add field; a rename offers the text again. */
  action: 'create' | 'rename'
  /** The list a todo belongs to, or the list being renamed. */
  listId?: string
  /** The todo being renamed. */
  todoId?: string
  text: string
}

export interface Notice {
  text: string
  /** Texts dropped with the failed save. */
  restore?: NoticeRestore[]
}

/** `changed`: refused, so the Member must confirm again. `failed`: not known or not allowed. */
export type DeleteListResult = 'deleted' | 'changed' | 'failed'

type FailureKind = 'stale' | 'not_found' | 'limit' | 'invalid' | 'unknown' | 'ended'

interface Failure {
  kind: FailureKind
  code?: string
  current?: Todo | List
  message?: string
}

type Saved = List | Todo | string[] | undefined

type SendResult = { ok: true; value: Saved } | { ok: false; error: Failure }

type Operation =
  | { kind: 'create_list'; key: string; name: string }
  | {
      kind: 'rename_list'
      listId?: string
      listKey?: string
      name: string
      /** The version on screen when the Member acted. */
      version: number
    }
  | {
      kind: 'delete_list'
      listId: string
      name: string
      version: number
      todoCount: number
      /** UI awaiting the server's answer (deletion is not optimistic). */
      resolve?: (result: DeleteListResult) => void
    }
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
  | {
      kind: 'rename_todo'
      listId: string
      todoId?: string
      todoKey?: string
      title: string
      version: number
    }
  | {
      kind: 'delete_todo'
      listId: string
      todoId?: string
      todoKey?: string
      version: number
    }
  | {
      kind: 'move_todo'
      listId: string
      todoId?: string
      todoKey?: string
      /** Its new neighbours, nearest first. No version: moves never conflict (D1). */
      above: TodoRef[]
      below: TodoRef[]
    }

/** A todo by its ID or, until it has one, by its client key. */
interface TodoRef {
  todoId?: string
  todoKey?: string
}

/**
 * A rename the Member has started. It holds the version they saw then, so what
 * another tab saves meanwhile is never overwritten; this tab's own saves move it
 * on like a queued edit (design D7).
 */
export type RenameDraft = Extract<Operation, { kind: 'rename_list' | 'rename_todo' }>

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

/** Neighbours a move sends each way (design D6). */
export const MAX_MOVE_NEIGHBOURS = 20

const refOf = (todo: TodoView): TodoRef => (todo.id ? { todoId: todo.id } : { todoKey: todo.key })

/**
 * Todos are kept as the server orders them: open ones, then done ones newest
 * first. A todo that is added or unticked goes last among the open ones, and
 * one that is ticked goes first among the done ones, so it is the same index.
 */
function insertTodo(todos: TodoView[], todo: TodoView): void {
  const doneAt = todos.findIndex((candidate) => candidate.done)
  todos.splice(doneAt === -1 ? todos.length : doneAt, 0, todo)
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

/** Put in the server's answer for a todo, moving it only if its state changed. */
function putTodo(todos: TodoView[], todo: TodoView): void {
  const index = todos.findIndex((candidate) => candidate.id === todo.id)
  if (index !== -1 && todos[index].done === todo.done) {
    todos[index] = todo
    return
  }
  if (index !== -1) todos.splice(index, 1)
  insertTodo(todos, todo)
}

const unloaded = (status: DetailStatus): Detail => ({
  status,
  loaded: false,
  refreshFailed: false,
  knownAt: 0,
  todos: [],
})

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
  private listsLoaded = false
  /** Orders the server's answers, so the newer count of a list is shown. */
  private clock = 0
  private listsKnownAt = 0
  private details = new Map<string, Detail>()
  private queue: Operation[] = []
  /** Renames being typed; not saved until submitted. */
  private drafts = new Set<RenameDraft>()
  private listKnownAt = new Map<string, number>()
  private running = false
  private epoch = 0
  private savesInFlight = 0
  // A newer load replaces an older one; only a save makes a load repeat (D7).
  // Counting both with one number made overlapping loads repeat each other.
  private listsLoad = 0
  private listsSaves = 0
  private listLoads = new Map<string, number>()
  private listSaves = new Map<string, number>()
  private inFlightByList = new Map<string, number>()
  private listReloads = new Set<string>()
  private listsReloadPending = false
  /** What still has to be fetched once the server answers again. */
  private listRecovery = new Set<string>()
  private listsRecovery = false
  /** A new todo's client key and its ID, so both name the same item. */
  private idByKey = new Map<string, string>()
  private keyById = new Map<string, string>()
  /** A new list's client key and its ID, so a rename queued early can wait. */
  private idByListKey = new Map<string, string>()
  private lastCreated: { key: string; id: string } | null = null

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
      deleting: this.queue.flatMap((op) => (op.kind === 'delete_list' ? [op.listId] : [])),
      lastCreated: this.lastCreated,
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

  /** Start renaming a saved list, at the version currently on screen. */
  startListRename(listId: string): RenameDraft | undefined {
    const list = this.listsView().find((candidate) => candidate.id === listId)
    if (!list) return undefined
    const draft: RenameDraft = {
      kind: 'rename_list',
      listId,
      name: list.name,
      version: list.version,
    }
    this.drafts.add(draft)
    return draft
  }

  /** Start renaming a todo, at the version currently on screen. */
  startTodoRename(listId: string, todo: TodoView): RenameDraft {
    const draft: RenameDraft = {
      kind: 'rename_todo',
      listId,
      ...(todo.id ? { todoId: todo.id } : { todoKey: todo.key }),
      title: todo.title,
      version: todo.version,
    }
    this.drafts.add(draft)
    return draft
  }

  /** Save a rename. A draft cancelled, submitted or reset earlier does nothing. */
  submitRename(draft: RenameDraft, text: string): void {
    if (!this.drafts.delete(draft)) return
    const trimmed = text.trim()
    const before = draft.kind === 'rename_list' ? draft.name : draft.title
    if (trimmed === before) return
    this.queue.push(
      draft.kind === 'rename_list' ? { ...draft, name: trimmed } : { ...draft, title: trimmed }
    )
    this.notify()
    this.kick()
  }

  cancelRename(draft: RenameDraft): void {
    this.drafts.delete(draft)
  }

  /** Rename a saved list, at the version currently on screen. */
  renameList(listId: string, name: string): void {
    const draft = this.startListRename(listId)
    if (draft) this.submitRename(draft, name)
  }

  /**
   * Delete a list and its todos. Not optimistic: resolves `deleted` only when the
   * server confirms, so the page stays and the confirmation can ask again.
   */
  deleteList(listId: string, version: number, todoCount: number): Promise<DeleteListResult> {
    const list = this.lists.find((candidate) => candidate.id === listId)
    if (!list) return Promise.resolve('deleted')
    const { promise, resolve } = Promise.withResolvers<DeleteListResult>()
    this.queue.push({
      kind: 'delete_list',
      listId,
      name: list.name,
      version,
      todoCount,
      resolve,
    })
    this.notify()
    this.kick()
    return promise
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

  /** Rename a todo, at the version currently on screen. */
  renameTodo(listId: string, todo: TodoView, title: string): void {
    this.submitRename(this.startTodoRename(listId, todo), title)
  }

  /** Delete a todo, at the version currently on screen. */
  deleteTodo(listId: string, todo: TodoView): void {
    this.queue.push({
      kind: 'delete_todo',
      listId,
      ...(todo.id ? { todoId: todo.id } : { todoKey: todo.key }),
      version: todo.version,
    })
    this.notify()
    this.kick()
  }

  /** Move an open todo to `index` among the open todos now on screen. */
  moveTodo(listId: string, todo: TodoView, index: number): void {
    const open = (this.viewDetails()[listId]?.todos ?? []).filter((other) => !other.done)
    const same = (other: TodoView) =>
      (todo.id != null && other.id === todo.id) || (todo.key != null && other.key === todo.key)
    const from = open.findIndex(same)
    const to = Math.max(0, Math.min(index, open.length - 1))
    if (from === -1 || from === to) return
    const [moved] = open.splice(from, 1)
    this.queue.push({
      kind: 'move_todo',
      listId,
      ...refOf(moved),
      above: open
        .slice(Math.max(0, to - MAX_MOVE_NEIGHBOURS), to)
        .reverse()
        .map(refOf),
      below: open.slice(to, to + MAX_MOVE_NEIGHBOURS).map(refOf),
    })
    this.notify()
    this.kick()
  }

  /** Load the caller's lists (beside the queue; a stale answer is dropped). */
  async loadLists(): Promise<void> {
    const epoch = this.epoch
    const load = ++this.listsLoad
    const saves = this.listsSaves
    // A newer load replaced this one, or a save made its answer stale.
    const dropped = (): boolean => {
      if (load !== this.listsLoad) return true
      if (saves === this.listsSaves && this.savesInFlight === 0) return false
      this.listsReloadPending = true
      return true
    }
    this.listsStatus = this.listsLoaded ? 'ready' : 'loading'
    this.notify()
    try {
      const lists = await this.timeout(this.api.list())
      if (epoch !== this.epoch || dropped()) return
      this.lists = lists.map(toListView)
      this.listsLoaded = true
      this.listsKnownAt = ++this.clock
      this.listsStatus = 'ready'
      this.listsRecovery = false
    } catch {
      if (epoch !== this.epoch || dropped()) return
      this.listsStatus = 'error'
      this.listsRecovery = true
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
    const load = (this.listLoads.get(id) ?? 0) + 1
    this.listLoads.set(id, load)
    const saves = this.listSaves.get(id) ?? 0
    const dropped = (): boolean => {
      if (load !== this.listLoads.get(id)) return true
      if (saves === (this.listSaves.get(id) ?? 0) && this.inFlight(id) === 0) return false
      this.listReloads.add(id)
      return true
    }
    if (!this.details.get(id)?.loaded) {
      this.details.set(id, unloaded('loading'))
    }
    this.notify()
    try {
      const detail = await this.timeout(this.api.getList(id))
      if (epoch !== this.epoch || dropped()) return
      this.putList(toListView(detail.list))
      this.details.set(id, {
        status: 'ready',
        loaded: true,
        refreshFailed: false,
        knownAt: ++this.clock,
        todos: detail.todos.map((todo) => toTodoView(todo, this.keyById.get(todo.id))),
      })
      this.listRecovery.delete(id)
      if (this.refetchAdvancesQueue) this.advanceQueuedVersions(id, detail.todos)
    } catch (error) {
      if (epoch !== this.epoch || dropped()) return
      const known = this.details.get(id)
      if (error instanceof ApiErrorException && error.status === 404) {
        this.listGone(id)
      } else {
        this.listRecovery.add(id)
        this.details.set(id, known?.loaded ? { ...known, refreshFailed: true } : unloaded('error'))
      }
    } finally {
      if (epoch === this.epoch) {
        this.notify()
        this.flushReloads()
      }
    }
  }

  /**
   * Fetch what failed or has an unknown outcome, when the server may be
   * reachable again. Each entry stays until its own load succeeds.
   */
  retry(): void {
    if (this.listsRecovery) this.listsReloadPending = true
    for (const id of this.listRecovery) this.listReloads.add(id)
    this.flushReloads()
  }

  /** Forget everything: sign-out or a different account signing in. */
  reset(): void {
    for (const op of this.queue) {
      if (op.kind === 'delete_list') op.resolve?.('failed')
    }
    this.epoch += 1
    this.running = false
    this.lists = []
    this.listsStatus = 'idle'
    this.listsLoaded = false
    this.listsKnownAt = 0
    this.details.clear()
    this.queue = []
    this.drafts.clear()
    this.listKnownAt.clear()
    this.listLoads.clear()
    this.listSaves.clear()
    this.inFlightByList.clear()
    this.listReloads.clear()
    this.listsReloadPending = false
    this.listRecovery.clear()
    this.listsRecovery = false
    this.idByKey.clear()
    this.keyById.clear()
    this.idByListKey.clear()
    this.lastCreated = null
    this.savesInFlight = 0
    this.notify()
  }

  private countSave(listId: string | null): void {
    this.listsSaves += 1
    if (listId) this.listSaves.set(listId, (this.listSaves.get(listId) ?? 0) + 1)
  }

  private putList(list: ListView): void {
    const index = this.lists.findIndex((candidate) => candidate.id === list.id)
    if (index === -1) this.lists.push(list)
    else this.lists[index] = list
  }

  private listGone(id: string): void {
    this.details.set(id, unloaded('not_found'))
    this.lists = this.lists.filter((list) => list.id !== id)
    this.listRecovery.delete(id)
  }

  private todoIdOf(op: { todoId?: string; todoKey?: string }): string | undefined {
    return op.todoId ?? (op.todoKey ? this.idByKey.get(op.todoKey) : undefined)
  }

  private listIdOf(op: { listId?: string; listKey?: string }): string | undefined {
    return op.listId ?? (op.listKey ? this.idByListKey.get(op.listKey) : undefined)
  }

  /** What the Member typed for this save, to offer again if it fails. */
  private textOf(op: Operation): NoticeRestore[] {
    switch (op.kind) {
      case 'create_list':
        return [{ target: 'list', action: 'create', text: op.name }]
      case 'rename_list':
        return [{ target: 'list', action: 'rename', listId: this.listIdOf(op), text: op.name }]
      case 'add_todo':
        return [{ target: 'todo', action: 'create', listId: op.listId, text: op.title }]
      case 'rename_todo':
        return [
          {
            target: 'todo',
            action: 'rename',
            listId: op.listId,
            todoId: this.todoIdOf(op),
            text: op.title,
          },
        ]
      case 'delete_list':
      case 'delete_todo':
      case 'mark_todo':
      case 'move_todo':
        return []
    }
  }

  /** One name per item, whether or not it had an ID when the edit was queued. */
  private itemKey(op: Operation): string {
    if (op.kind === 'create_list') return `list-key:${op.key}`
    if (op.kind === 'rename_list' || op.kind === 'delete_list') {
      const id = this.listIdOf(op)
      if (id) return `list-id:${id}`
      return `list-key:${'listKey' in op ? op.listKey : undefined}`
    }
    const id = op.kind === 'add_todo' ? this.idByKey.get(op.key) : this.todoIdOf(op)
    if (id) return `todo-id:${id}`
    return `todo-key:${op.kind === 'add_todo' ? op.key : op.todoKey}`
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
      if (detail.loaded) this.applyQueueToTodos(id, todos)
      view[id] = { status: detail.status, refreshFailed: detail.refreshFailed, todos }
    }
    return view
  }

  private listsView(): ListView[] {
    const lists = this.lists.map((list) => ({ ...list }))
    for (const [id, detail] of this.details) {
      if (!detail.loaded) continue
      // The list itself was answered after its todos: its count is newer.
      const queued = this.queue.some((op) => op.kind !== 'create_list' && op.listId === id)
      const listAt = Math.max(this.listsKnownAt, this.listKnownAt.get(id) ?? 0)
      if (!queued && detail.knownAt < listAt) continue
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
    for (const op of this.queue) {
      if (op.kind !== 'rename_list') continue
      const id = this.listIdOf(op)
      const target = id
        ? lists.find((list) => list.id === id)
        : lists.find((list) => list.key === op.listKey)
      if (target) target.name = op.name
    }
    return lists
  }

  private applyQueueToTodos(listId: string, todos: TodoView[]): void {
    for (const op of this.queue) {
      if (op.kind === 'add_todo' && op.listId === listId) {
        insertTodo(todos, {
          id: null,
          key: op.key,
          title: op.title,
          version: 1,
          done: false,
          pending: true,
        })
        continue
      }
      if (op.kind === 'rename_todo' && op.listId === listId) {
        const id = this.todoIdOf(op)
        const index = todos.findIndex(
          (todo) =>
            (id != null && todo.id === id) || (op.todoKey != null && todo.key === op.todoKey)
        )
        if (index !== -1) {
          todos[index].title = op.title
          todos[index].version = op.version
        }
        continue
      }
      if (op.kind === 'delete_todo' && op.listId === listId) {
        removeTodo(todos, { id: this.todoIdOf(op), key: op.todoKey })
        continue
      }
      if (op.kind === 'move_todo' && op.listId === listId) {
        this.applyMove(op, todos)
        continue
      }
      if (op.kind !== 'mark_todo' || op.listId !== listId) continue
      const id = this.todoIdOf(op)
      const index = todos.findIndex(
        (todo) => (id != null && todo.id === id) || (op.todoKey != null && todo.key === op.todoKey)
      )
      if (index === -1) continue
      const target = todos[index]
      target.version = op.version
      if (target.done === op.done) continue
      todos.splice(index, 1)
      target.done = op.done
      target.completed_at = op.done ? new Date().toISOString() : undefined
      insertTodo(todos, target)
    }
  }

  /** Place a queued move where the server's rule would (design D6). */
  private applyMove(op: Extract<Operation, { kind: 'move_todo' }>, todos: TodoView[]): void {
    const indexOf = (ref: TodoRef) => {
      const id = this.todoIdOf(ref)
      return todos.findIndex(
        (todo) =>
          !todo.done &&
          ((id != null && todo.id === id) || (ref.todoKey != null && todo.key === ref.todoKey))
      )
    }
    const from = indexOf(op)
    if (from === -1) return
    const [moved] = todos.splice(from, 1)
    const first = (refs: TodoRef[]) => refs.map(indexOf).find((index) => index !== -1)
    const above = first(op.above)
    const below = first(op.below)
    if (op.above.length === 0) todos.unshift(moved)
    else if (above !== undefined) todos.splice(above + 1, 0, moved)
    else if (below !== undefined) todos.splice(below, 0, moved)
    else insertTodo(todos, moved)
  }

  private notify(): void {
    this.onChange(this.snapshot())
  }

  private kick(): void {
    if (this.running) return
    this.running = true
    const epoch = this.epoch
    void this.drain(epoch).finally(() => {
      // A reset has already released the queue to a new run.
      if (this.epoch !== epoch) return
      this.running = false
      if (this.queue.length > 0) this.kick()
    })
  }

  private async drain(epoch: number): Promise<void> {
    while (this.queue.length > 0 && this.epoch === epoch) {
      const op = this.queue[0]
      const listId = op.kind === 'create_list' ? null : (op.listId ?? null)
      this.countSave(listId)
      this.savesInFlight += 1
      if (listId) this.setInFlight(listId, 1)
      this.notify()

      const result = await this.sendWithRepeat(op, epoch)
      if (this.epoch !== epoch) return

      this.savesInFlight -= 1
      this.countSave(listId)
      if (listId) this.setInFlight(listId, -1)
      // Off the queue first: an answer that can't be applied must not leave the
      // save at its head, to be sent again without end.
      this.queue.shift()
      if (result.ok) {
        try {
          this.applySuccess(op, result.value)
        } catch {
          this.applyFailure(op, { kind: 'unknown' })
        }
      } else {
        this.applyFailure(op, result.error)
      }
      this.notify()
      // Any answer, a refusal included, means the server is reachable again.
      if (result.ok || (result.error.kind !== 'unknown' && result.error.kind !== 'ended'))
        this.retry()
      else this.flushReloads()
    }
  }

  private async sendWithRepeat(op: Operation, epoch: number): Promise<SendResult> {
    const first = await this.send(op)
    // One retry with the same key or version, so a lost answer can't create a
    // duplicate or apply twice. Never after a reset: the cookie may by then
    // belong to another Member.
    if (first.ok || first.error.kind !== 'unknown' || this.epoch !== epoch) return first
    return this.send(op)
  }

  private async send(op: Operation): Promise<SendResult> {
    try {
      switch (op.kind) {
        case 'create_list':
          return { ok: true, value: await this.timeout(this.api.createList(op.key, op.name)) }
        case 'rename_list': {
          const listId = this.listIdOf(op)
          if (!listId) return { ok: false, error: { kind: 'not_found' } }
          return {
            ok: true,
            value: await this.timeout(this.api.renameList(listId, op.version, op.name)),
          }
        }
        case 'delete_list':
          await this.timeout(this.api.deleteList(op.listId, op.version, op.todoCount))
          return { ok: true, value: undefined }
        case 'add_todo':
          return {
            ok: true,
            value: await this.timeout(this.api.addTodo(op.listId, op.key, op.title)),
          }
        case 'mark_todo': {
          const todoId = this.todoIdOf(op)
          if (!todoId) return { ok: false, error: { kind: 'not_found' } }
          return {
            ok: true,
            value: await this.timeout(this.api.markTodo(op.listId, todoId, op.version, op.done)),
          }
        }
        case 'rename_todo': {
          const todoId = this.todoIdOf(op)
          if (!todoId) return { ok: false, error: { kind: 'not_found' } }
          return {
            ok: true,
            value: await this.timeout(this.api.renameTodo(op.listId, todoId, op.version, op.title)),
          }
        }
        case 'delete_todo': {
          const todoId = this.todoIdOf(op)
          if (!todoId) return { ok: false, error: { kind: 'not_found' } }
          await this.timeout(this.api.deleteTodo(op.listId, todoId, op.version))
          return { ok: true, value: undefined }
        }
        case 'move_todo': {
          const todoId = this.todoIdOf(op)
          if (!todoId) return { ok: false, error: { kind: 'not_found' } }
          const ids = (refs: TodoRef[]) =>
            refs.map((ref) => this.todoIdOf(ref)).filter((id) => id !== undefined)
          // An empty `above` means the top. When neighbours were left out because
          // they were never saved, the todo's own ID, which the server ignores,
          // says that this is not the top.
          const known = ids(op.above)
          const above = known.length === 0 && op.above.length > 0 ? [todoId] : known
          return {
            ok: true,
            value: await this.timeout(this.api.moveTodo(op.listId, todoId, above, ids(op.below))),
          }
        }
      }
    } catch (error) {
      return { ok: false, error: this.classify(error) }
    }
  }

  private timeout<T>(promise: Promise<T>): Promise<T> {
    if (!Number.isFinite(this.timeoutMs)) return promise
    return new Promise<T>((resolve, reject) => {
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
    })
  }

  private classify(error: unknown): Failure {
    if (error instanceof ApiErrorException) {
      if (error.status === 409 && error.code === 'stale') {
        return { kind: 'stale', current: error.current as Todo | List | undefined }
      }
      const message = error.serverMessage ?? error.message
      if (error.status === 409) return { kind: 'limit', message }
      if (error.status === 404) return { kind: 'not_found', code: error.code }
      if (error.status === 400) return { kind: 'invalid', message }
      if ((error.status ?? 0) >= 500) return { kind: 'unknown' }
      return { kind: 'invalid', message }
    }
    if (error instanceof SessionEndedError) return { kind: 'ended' }
    return { kind: 'unknown' }
  }

  private applySuccess(op: Operation, value: Saved): void {
    switch (op.kind) {
      case 'create_list': {
        const list = value as List
        this.idByListKey.set(op.key, list.id)
        this.putList(toListView(list))
        this.lastCreated = { key: op.key, id: list.id }
        // A list just created is known to be empty, so it opens at once.
        const empty = list.todo_count === 0
        this.details.set(list.id, {
          status: empty ? 'ready' : 'idle',
          loaded: empty,
          refreshFailed: false,
          knownAt: ++this.clock,
          todos: [],
        })
        break
      }
      case 'rename_list': {
        const answer = value as List
        this.putList(toListView(answer))
        this.rebaseQueuedVersions(op, answer.version)
        break
      }
      case 'delete_list': {
        this.listDeleted(op)
        break
      }
      case 'add_todo': {
        const todo = value as Todo
        this.idByKey.set(op.key, todo.id)
        this.keyById.set(todo.id, op.key)
        const detail = this.details.get(op.listId)
        if (detail) {
          detail.knownAt = ++this.clock
          putTodo(detail.todos, toTodoView(todo, op.key))
        }
        break
      }
      case 'mark_todo':
      case 'rename_todo': {
        const answer = value as Todo
        const detail = this.details.get(op.listId)
        const known = detail?.todos.find((todo) => todo.id === answer.id)
        // Only a save that changed something made `n → n + 1`. One that asked
        // for what version n already held is a success too, and n + 1 is then
        // another tab's change, which the Member may not have seen (D7).
        const changed =
          known?.version === op.version &&
          (op.kind === 'mark_todo' ? known.done !== op.done : known.title !== op.title)
        if (detail) {
          detail.knownAt = ++this.clock
          putTodo(detail.todos, toTodoView(answer, this.keyById.get(answer.id)))
        }
        if (changed) this.rebaseQueuedVersions(op, answer.version)
        break
      }
      case 'delete_todo': {
        const detail = this.details.get(op.listId)
        const todoId = this.todoIdOf(op)
        if (detail) {
          detail.knownAt = ++this.clock
          removeTodo(detail.todos, { id: todoId, key: op.todoKey })
        }
        if (todoId) {
          this.keyById.delete(todoId)
          this.idByKey.delete(op.todoKey ?? '')
        }
        break
      }
      case 'move_todo': {
        this.applyOrder(op.listId, value as string[])
        break
      }
    }
  }

  /** The order after a move. Todos that came or went elsewhere need a load. */
  private applyOrder(listId: string, order: string[]): void {
    const detail = this.details.get(listId)
    if (!detail?.loaded) return
    const rank = new Map(order.map((id, index) => [id, index]))
    const open = detail.todos.filter((todo) => !todo.done)
    const rankOf = (todo: TodoView) => rank.get(todo.id ?? '') ?? order.length
    detail.todos = [
      ...open.sort((a, b) => rankOf(a) - rankOf(b)),
      ...detail.todos.filter((todo) => todo.done),
    ]
    if (open.length !== rank.size || open.some((todo) => !rank.has(todo.id ?? ''))) {
      this.listReloads.add(listId)
    }
  }

  /** Take every queued operation on a list, and on its todos, off the queue. */
  private dropListOperations(id: string): Operation[] {
    const ofList = (op: Operation) => op.kind !== 'create_list' && this.listIdOf(op) === id
    const dropped = this.queue.filter(ofList)
    this.queue = this.queue.filter((op) => !ofList(op))
    return dropped
  }

  /** This tab's deletion took effect: nothing may follow it. */
  private listDeleted(
    op: Extract<Operation, { kind: 'delete_list' }>,
    already: Operation[] = []
  ): void {
    const dropped = [...already, ...this.dropListOperations(op.listId)]
    this.removeListState(op.listId)
    for (const other of [op, ...dropped]) {
      if (other.kind === 'delete_list') other.resolve?.('deleted')
    }
    const restore = dropped.flatMap((other) => this.textOf(other))
    if (restore.length > 0) {
      this.onNotice({ text: 'The list was deleted, so a later change was not saved.', restore })
    }
  }

  private removeListState(id: string): void {
    this.lists = this.lists.filter((list) => list.id !== id)
    this.details.delete(id)
    this.listRecovery.delete(id)
    this.listReloads.delete(id)
    if (this.lastCreated?.id === id) this.lastCreated = null
  }

  /** A save that moved `n → n + 1` moves later queued edits of it to `n + 1`. */
  private rebaseQueuedVersions(op: Operation & { version: number }, newVersion: number): void {
    const oldVersion = op.version
    if (newVersion !== oldVersion + 1) return
    const key = this.itemKey(op)
    for (const queued of [...this.queue, ...this.drafts]) {
      if (!('version' in queued)) continue
      if (this.itemKey(queued) === key && queued.version === oldVersion) {
        queued.version = newVersion
      }
    }
  }

  private applyFailure(op: Operation, failure: Failure): void {
    const key = this.itemKey(op)
    // Later edits of the same item depend on this one; drop them together and
    // keep a single message, with every text that was dropped.
    const dropped = this.queue.filter((queued) => this.itemKey(queued) === key)
    const outcome =
      failure.kind === 'stale' ? 'changed' : failure.kind === 'not_found' ? 'deleted' : 'failed'
    for (const queued of dropped) {
      if (queued.kind === 'delete_list') queued.resolve?.(outcome)
    }
    this.queue = this.queue.filter((queued) => this.itemKey(queued) !== key)
    const restore = [op, ...dropped].flatMap((dependent) => this.textOf(dependent))

    switch (failure.kind) {
      case 'stale': {
        if (failure.current && 'name' in failure.current) {
          this.putList(toListView(failure.current))
          // The refusal carries the server's current counts: newer than the
          // last loaded detail, so don't let that detail override them. Only this
          // list's: other lists' stored counts may be older than their details.
          this.listKnownAt.set(failure.current.id, ++this.clock)
          this.onNotice({
            text: 'This list changed elsewhere. The current version is shown.',
            restore,
          })
        } else if (failure.current) {
          const todo = toTodoView(failure.current, this.keyById.get(failure.current.id))
          const listId = 'listId' in op ? op.listId : undefined
          const detail = listId ? this.details.get(listId) : undefined
          if (detail) putTodo(detail.todos, todo)
          this.onNotice({
            text: 'This changed elsewhere. The current version is shown.',
            restore,
          })
        } else {
          this.onNotice({ text: 'This changed elsewhere.', restore })
        }
        if (op.kind === 'delete_list') op.resolve?.('changed')
        break
      }
      case 'not_found': {
        if (op.kind === 'create_list') break
        // Deleting something already deleted simply removes it.
        if (op.kind === 'delete_list') {
          this.listDeleted(op, dropped)
          break
        }
        // Adding and renaming a list answer 404 only for the list; other
        // changes to a todo name what is missing.
        if (
          op.kind === 'add_todo' ||
          op.kind === 'rename_list' ||
          failure.code === 'list_not_found'
        ) {
          const listId = this.listIdOf(op)
          if (!listId) break
          const ofList = this.dropListOperations(listId)
          for (const other of ofList) {
            if (other.kind === 'delete_list') other.resolve?.('deleted')
          }
          this.listGone(listId)
          this.onNotice({
            text: 'This list was deleted elsewhere.',
            restore: [...restore, ...ofList.flatMap((other) => this.textOf(other))],
          })
          break
        }
        const detail = this.details.get(op.listId)
        if (detail) removeTodo(detail.todos, { id: this.todoIdOf(op), key: op.todoKey })
        if (op.kind === 'delete_todo') break
        this.onNotice({ text: 'This todo was deleted elsewhere.', restore })
        break
      }
      case 'ended': {
        // The page is being replaced by the sign-in or verification page.
        if (op.kind === 'delete_list') op.resolve?.('failed')
        for (const queued of this.queue) {
          if (queued.kind === 'delete_list') queued.resolve?.('failed')
        }
        this.queue = []
        break
      }
      case 'limit':
      case 'invalid': {
        this.onNotice({
          text: failure.message ?? 'That change could not be saved.',
          restore,
        })
        if (op.kind === 'delete_list') op.resolve?.('failed')
        break
      }
      case 'unknown': {
        this.onNotice({
          text: "Couldn't save your change. The list will refresh when the server is reachable.",
          restore,
        })
        if (op.kind === 'delete_list') op.resolve?.('failed')
        // Tried at once, then again by retry() until the load succeeds.
        if (op.kind === 'create_list') {
          this.listsRecovery = true
          this.listsReloadPending = true
        } else if (op.listId) {
          this.listRecovery.add(op.listId)
          this.listReloads.add(op.listId)
        }
        break
      }
    }
  }

  /** Test-only broken variant: a refetch advances queued edits' versions. */
  private advanceQueuedVersions(listId: string, todos: Todo[]): void {
    for (const op of this.queue) {
      if (op.kind !== 'mark_todo' && op.kind !== 'rename_todo' && op.kind !== 'delete_todo') {
        continue
      }
      if (op.listId !== listId) continue
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
    // Wait for the queue, or every queued save would drop the reload again.
    if (this.savesInFlight > 0) return
    if (this.listsReloadPending && this.queue.length === 0) {
      this.listsReloadPending = false
      void this.loadLists()
    }
    for (const id of [...this.listReloads]) {
      if (this.queue.some((op) => op.kind !== 'create_list' && op.listId === id)) continue
      this.listReloads.delete(id)
      void this.loadList(id)
    }
  }
}
