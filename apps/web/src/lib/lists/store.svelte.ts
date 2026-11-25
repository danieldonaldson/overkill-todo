// The tab's single lists store: one engine, exposed as runes state so every
// island reads the same view. The toaster is mounted in the persisted header,
// so a failed save is seen and announced on any dashboard page (design D7).

import { toast } from 'svelte-sonner'
import { api } from '../api'
import {
  ListsEngine,
  type ListsApi,
  type Notice,
  type NoticeRestore,
  type Snapshot,
} from './engine'

// Set by the session store, which knows whose account this tab shows.
let guard: () => Promise<void> = async () => {}

const transport: ListsApi = {
  list: () => api.lists.list(),
  createList: async (key, name) => {
    await guard()
    return api.lists.create({ key, name })
  },
  getList: (id) => api.lists.get(id),
  addTodo: async (listId, key, title) => {
    await guard()
    return api.lists.addTodo(listId, { key, title })
  },
  markTodo: async (listId, todoId, version, done) => {
    await guard()
    return api.lists.markTodo(listId, todoId, { version, done })
  },
}

/** Text of a save that failed, kept until the Member submits or discards it. */
export interface UnsavedText extends NoticeRestore {
  id: number
}

const unsaved = $state<{ items: UnsavedText[] }>({ items: [] })
let nextUnsavedId = 1

const view = $state<{ snapshot: Snapshot; account: number }>({
  snapshot: null as unknown as Snapshot,
  account: 0,
})

function handleNotice(notice: Notice): void {
  toast.error(notice.text)
  for (const text of notice.restore ?? []) {
    unsaved.items.push({ ...text, id: nextUnsavedId++ })
  }
}

function reset(): void {
  unsaved.items = []
  engine.reset()
}

const engine = new ListsEngine({
  api: transport,
  onChange: (snapshot) => {
    view.snapshot = snapshot
  },
  onNotice: handleNotice,
})
view.snapshot = engine.snapshot()

export const lists = {
  get snapshot(): Snapshot {
    return view.snapshot
  },
  /** Goes up when another Member's account replaces this one: load again. */
  get account(): number {
    return view.account
  },
  /** Texts of failed saves, oldest first. None replaces another (design D7). */
  get unsaved(): UnsavedText[] {
    return unsaved.items
  },
  takeUnsaved(id: number): void {
    unsaved.items = unsaved.items.filter((item) => item.id !== id)
  },
  createList: (name: string): string => engine.createList(name),
  addTodo: (listId: string, title: string): string => engine.addTodo(listId, title),
  markTodo: (listId: string, todo: Parameters<ListsEngine['markTodo']>[1], done: boolean): void =>
    engine.markTodo(listId, todo, done),
  loadLists: (): Promise<void> => engine.loadLists(),
  loadList: (id: string): Promise<void> => engine.loadList(id),
  retry: (): void => engine.retry(),
  /** Run before each save; a throw stops it from being sent. */
  guardSaves(check: () => Promise<void>): void {
    guard = check
  },
  /** Sign-out: forget everything and load nothing. */
  reset,
  /** Another Member is signed in: forget everything, then panels load again. */
  switchAccount(): void {
    reset()
    view.account += 1
  },
}

// A save with an unknown outcome is retried as soon as the browser can reach
// the server again, or the Member returns to the tab (design D7). The handlers
// are registered once and are harmless when the queue is empty.
if (typeof window !== 'undefined') {
  window.addEventListener('online', () => lists.retry())
  window.addEventListener('focus', () => lists.retry())
}
