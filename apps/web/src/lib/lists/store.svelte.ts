// The tab's single lists store: one engine, exposed as runes state so every
// island reads the same view. The toaster is mounted in the persisted header,
// so a failed save is seen and announced on any dashboard page (design D7).

import { toast } from 'svelte-sonner'
import { api } from '../api'
import { ListsEngine, type ListsApi, type Notice, type Snapshot } from './engine'

const transport: ListsApi = {
  list: () => api.lists.list(),
  createList: (key, name) => api.lists.create({ key, name }),
  getList: (id) => api.lists.get(id),
  addTodo: (listId, key, title) => api.lists.addTodo(listId, { key, title }),
  markTodo: (listId, todoId, version, done) =>
    api.lists.markTodo(listId, todoId, { version, done }),
}

const restoreState = $state<{
  listName: string | null
  todo: { listId: string; title: string } | null
}>({ listName: null, todo: null })

const view = $state<{ snapshot: Snapshot }>({ snapshot: null as unknown as Snapshot })

function handleNotice(notice: Notice): void {
  toast.error(notice.text)
  if (notice.restore?.target === 'list') restoreState.listName = notice.restore.text
  if (notice.restore?.target === 'todo' && notice.restore.listId) {
    restoreState.todo = { listId: notice.restore.listId, title: notice.restore.text }
  }
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
  get restoreListName(): string | null {
    return restoreState.listName
  },
  get restoreTodo(): { listId: string; title: string } | null {
    return restoreState.todo
  },
  consumeListRestore(): void {
    restoreState.listName = null
  },
  consumeTodoRestore(): void {
    restoreState.todo = null
  },
  createList: (name: string): string => engine.createList(name),
  addTodo: (listId: string, title: string): string => engine.addTodo(listId, title),
  markTodo: (listId: string, todo: Parameters<ListsEngine['markTodo']>[1], done: boolean): void =>
    engine.markTodo(listId, todo, done),
  loadLists: (): Promise<void> => engine.loadLists(),
  loadList: (id: string): Promise<void> => engine.loadList(id),
  retry: (): void => engine.retry(),
  reset: (): void => engine.reset(),
}

// A save with an unknown outcome is retried as soon as the browser can reach
// the server again, or the Member returns to the tab (design D7). The handlers
// are registered once and are harmless when the queue is empty.
if (typeof window !== 'undefined') {
  window.addEventListener('online', () => lists.retry())
  window.addEventListener('focus', () => lists.retry())
}
