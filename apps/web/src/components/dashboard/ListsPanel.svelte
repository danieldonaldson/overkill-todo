<script lang="ts">
  import TelemetryBoundary from '$lib/telemetry/TelemetryBoundary.svelte'
  import { createIslandTiming } from '$lib/telemetry/island-timing'
  import { Button } from '$lib/components/ui/button'
  import { navigate } from '$lib/navigation'
  import { tick, untrack } from 'svelte'
  import { lists, type UnsavedText } from '../../lib/lists/store.svelte'
  import type { RenameDraft, TodoView } from '../../lib/lists/engine'

  // One page for every list address: the shell is built once with a
  // placeholder id and the web server rewrites real addresses to it (design
  // D8). The panel reads the list id from `location.pathname`; an id that isn't
  // a UUID shows "list not found" without a request.
  const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

  function idFromPath(): string | null {
    if (typeof location === 'undefined') return null
    const match = location.pathname.replace(/\/+$/, '').match(/^\/dashboard\/lists\/(.+)$/)
    if (!match) return null
    try {
      // The server writes IDs in lower case, and they are compared as text.
      return decodeURIComponent(match[1]).toLowerCase()
    } catch {
      return match[1]
    }
  }

  const initialId = idFromPath()
  let selectedId = $state(initialId !== null && UUID.test(initialId) ? initialId : null)
  let malformedId = $state(initialId !== null && !UUID.test(initialId))
  let creatingName = $state('')
  let addTitle = $state('')
  let createError = $state('')
  let addError = $state('')
  let completedOpen = $state(false)
  let retrying = $state(false)
  let renamingList = $state(false)
  let listName = $state('')
  let listRenameError = $state('')
  let renamingTodo = $state<string | null>(null)
  let todoTitle = $state('')
  let todoRenameError = $state('')
  let deleteOpen = $state(false)
  let deleteError = $state('')
  /** What is being renamed, at the version seen when the form opened (D7). */
  let listDraft: RenameDraft | undefined
  let todoDraft: RenameDraft | undefined
  /** The list this panel submitted, to open it once it is saved (D3). */
  let submittedListKey: string | null = null

  const snapshot = $derived(lists.snapshot)
  const currentList = $derived(snapshot.lists.find((list) => list.id === selectedId))
  const detail = $derived(selectedId ? snapshot.details[selectedId] : undefined)
  const todos = $derived(detail?.todos ?? [])
  const openTodos = $derived(todos.filter((todo) => !todo.done))
  const doneTodos = $derived(todos.filter((todo) => todo.done))

  const timing = createIslandTiming()
  const ready = $derived(
    malformedId ||
      (selectedId
        ? detail?.status === 'ready' || detail?.status === 'not_found' || detail?.status === 'error'
        : snapshot.listsStatus === 'ready' || snapshot.listsStatus === 'error')
  )
  $effect(() => {
    if (ready) timing.markReady()
  })

  $effect(() => {
    const created = snapshot.lastCreated
    if (!created || created.key !== submittedListKey) return
    // Once only: coming back to the overview later must not open it again.
    submittedListKey = null
    const { id } = created
    if (location.pathname.replace(/\/+$/, '') === '/dashboard/lists') {
      // Keep the router's own `history.state`: only the address changes, so the
      // queue and the persisted header survive (design D3).
      history.replaceState(history.state, '', `/dashboard/lists/${id}`)
      selectedId = id
      malformedId = false
      void lists.loadList(id)
    }
  })

  const unsavedNewLists = $derived(
    lists.unsaved.filter((item) => item.target === 'list' && item.action === 'create')
  )
  const unsavedListRenames = $derived(
    lists.unsaved.filter((item) => item.target === 'list' && item.action === 'rename')
  )
  const unsavedNewTodos = $derived(
    lists.unsaved.filter(
      (item) => item.target === 'todo' && item.action === 'create' && item.listId === selectedId
    )
  )
  const unsavedTodoRenames = $derived(
    lists.unsaved.filter(
      (item) => item.target === 'todo' && item.action === 'rename' && item.listId === selectedId
    )
  )
  const deleting = $derived(selectedId ? snapshot.deleting.includes(selectedId) : false)

  // A failed text goes back into an empty input; otherwise it is offered below
  // it, so it never replaces what is being typed (D7). Reading the input
  // untracked means emptying it later doesn't put a text in mid-edit.
  $effect(() => {
    const item = unsavedNewLists[0]
    if (!item || untrack(() => creatingName) !== '') return
    creatingName = item.text
    lists.takeUnsaved(item.id)
  })

  $effect(() => {
    const item = unsavedNewTodos[0]
    if (!item || detail?.status !== 'ready' || untrack(() => addTitle) !== '') return
    addTitle = item.text
    lists.takeUnsaved(item.id)
  })

  // On mount, and again when another Member's account replaces this one.
  let account = lists.account
  $effect(() => {
    const changed = lists.account !== account
    account = lists.account
    untrack(() => {
      if (changed) {
        creatingName = ''
        addTitle = ''
        submittedListKey = null
        cancelListRename()
        cancelTodoRename()
        deleteOpen = false
        deleteError = ''
      }
      void lists.loadLists()
      if (selectedId) void lists.loadList(selectedId)
    })
  })

  $effect(() => {
    void selectedId
    untrack(() => {
      cancelListRename()
      cancelTodoRename()
      deleteOpen = false
      deleteError = ''
    })
  })

  $effect(() => {
    if (!deleteOpen) return
    void tick().then(() => document.getElementById('cancel-delete-list')?.focus())
  })

  $effect(() => {
    if (!renamingList) return
    void tick().then(() => document.getElementById('rename-list-name')?.focus())
  })

  $effect(() => {
    if (!renamingTodo) return
    const id = renamingTodo
    void tick().then(() => document.getElementById(`rename-todo-${id}`)?.focus())
  })

  function submitAgain(item: UnsavedText) {
    lists.takeUnsaved(item.id)
    if (item.action === 'create') {
      if (item.target === 'list') submittedListKey = lists.createList(item.text)
      else if (item.listId) lists.addTodo(item.listId, item.text)
    } else if (item.target === 'list') {
      if (item.listId) lists.renameList(item.listId, item.text)
    } else if (item.listId && item.todoId) {
      const todo = todos.find((candidate) => candidate.id === item.todoId)
      if (todo) lists.renameTodo(item.listId, todo, item.text)
    }
  }

  /** A dropped text can be submitted again only while its item is still shown. */
  function canResubmit(item: UnsavedText): boolean {
    if (item.action === 'create') return item.target === 'list' || !!item.listId
    if (item.target === 'list') {
      return !!item.listId && snapshot.lists.some((list) => list.id === item.listId)
    }
    return !!item.listId && !!item.todoId && todos.some((candidate) => candidate.id === item.todoId)
  }

  function beginListRename() {
    if (!currentList?.id) return
    cancelListRename()
    listDraft = lists.startListRename(currentList.id)
    listName = currentList.name
    listRenameError = ''
    renamingList = true
  }

  function cancelListRename() {
    if (listDraft) lists.cancelRename(listDraft)
    listDraft = undefined
    renamingList = false
    listRenameError = ''
  }

  function submitListRename(event: SubmitEvent) {
    event.preventDefault()
    if (!selectedId) return
    const name = listName.trim()
    listRenameError = ''
    if (codePoints(name) === 0) {
      listRenameError = "A list name can't be empty"
      return
    }
    if (codePoints(name) > 100) {
      listRenameError = 'A list name can be at most 100 characters'
      return
    }
    if (listDraft) lists.submitRename(listDraft, name)
    cancelListRename()
  }

  function beginTodoRename(todo: TodoView) {
    if (!selectedId) return
    cancelTodoRename()
    todoDraft = lists.startTodoRename(selectedId, todo)
    renamingTodo = todo.key ?? todo.id
    todoTitle = todo.title
    todoRenameError = ''
  }

  function cancelTodoRename() {
    if (todoDraft) lists.cancelRename(todoDraft)
    todoDraft = undefined
    renamingTodo = null
    todoRenameError = ''
  }

  function submitTodoRename(event: SubmitEvent) {
    event.preventDefault()
    if (!selectedId) return
    const title = todoTitle.trim()
    todoRenameError = ''
    if (codePoints(title) === 0) {
      todoRenameError = "A todo can't be empty"
      return
    }
    if (codePoints(title) > 500) {
      todoRenameError = 'A todo can be at most 500 characters'
      return
    }
    if (todoDraft) lists.submitRename(todoDraft, title)
    cancelTodoRename()
  }

  /** Delete one todo and keep a keyboard user's place (as after a tick). */
  async function removeTodo(todo: TodoView, event: Event) {
    if (!selectedId) return
    const button = event.currentTarget as HTMLElement
    const row = button.closest('li')
    const group = row?.closest('ul')
    const boxes = () => [
      ...(group?.querySelectorAll<HTMLInputElement>('input[type="checkbox"]') ?? []),
    ]
    const own = row?.querySelector<HTMLInputElement>('input[type="checkbox"]')
    const index = own ? boxes().indexOf(own) : -1
    const focused = row?.contains(document.activeElement) ?? false
    lists.deleteTodo(selectedId, todo)
    await tick()
    if (!focused || row?.isConnected) return
    const left = group?.isConnected ? boxes() : []
    const next = left[index] ?? left[left.length - 1]
    ;(next ?? document.getElementById('new-todo-title'))?.focus()
  }

  function openDelete() {
    if (!selectedId) return
    deleteError = ''
    deleteOpen = true
  }

  function closeDelete() {
    deleteOpen = false
    deleteError = ''
  }

  function cancelDelete() {
    if (!deleting) closeDelete()
  }

  function modal(node: HTMLDialogElement) {
    node.showModal()
    // Closing first returns focus to where it was.
    return { destroy: () => node.close() }
  }

  async function confirmDelete(event: SubmitEvent) {
    event.preventDefault()
    // Deletion is permanent and waits for the server (D7); the queue drains
    // first so the count the Member confirmed is the one the server sees.
    if (!selectedId || !currentList || deleting || snapshot.queueLength > 0) return
    deleteError = ''
    const id = selectedId
    const result = await lists.deleteList(id, currentList.version, currentList.todo_count)
    if (result === 'deleted') {
      deleteOpen = false
      // The Member may have moved on while the server answered.
      if (idFromPath() === id) navigate('/dashboard/lists')
    } else if (result === 'changed') {
      deleteError =
        'This list changed elsewhere. Check the name and number of todos, then confirm again.'
    } else {
      deleteError = "The list couldn't be deleted. Try again."
    }
  }

  const codePoints = (text: string) => [...text.trim()].length

  function submitList(event: SubmitEvent) {
    event.preventDefault()
    const name = creatingName.trim()
    createError = ''
    if (codePoints(name) === 0) {
      createError = "A list name can't be empty"
      return
    }
    if (codePoints(name) > 100) {
      createError = 'A list name can be at most 100 characters'
      return
    }
    submittedListKey = lists.createList(name)
    creatingName = ''
  }

  function submitTodo(event: SubmitEvent) {
    event.preventDefault()
    if (!selectedId) return
    const title = addTitle.trim()
    addError = ''
    if (codePoints(title) === 0) {
      addError = "A todo can't be empty"
      return
    }
    if (codePoints(title) > 500) {
      addError = 'A todo can be at most 500 characters'
      return
    }
    lists.addTodo(selectedId, title)
    addTitle = ''
  }

  /** Svelte's event delegation misses `paste` here; attach it to the input. */
  function pasteNormalizer(node: HTMLInputElement, set: (value: string) => void) {
    const handler = (event: Event) => {
      const paste = event as ClipboardEvent
      // A single-line input strips line breaks on paste; put spaces in instead.
      const text = paste.clipboardData?.getData('text/plain')
      if (!text || !/[\r\n]/.test(text)) return
      paste.preventDefault()
      const start = node.selectionStart ?? node.value.length
      const end = node.selectionEnd ?? node.value.length
      node.setRangeText(text.replace(/[ \t]*[\r\n]+[ \t]*/g, ' '), start, end, 'end')
      set(node.value)
    }
    node.addEventListener('paste', handler)
    return { destroy: () => node.removeEventListener('paste', handler) }
  }

  async function toggleTodo(todo: TodoView, event: Event) {
    if (!selectedId) return
    const box = event.currentTarget as HTMLInputElement
    const group = box.closest('ul')
    const boxes = () => [...(group?.querySelectorAll<HTMLInputElement>('input') ?? [])]
    const index = boxes().indexOf(box)
    const focused = document.activeElement === box
    lists.markTodo(selectedId, todo, !todo.done)
    await tick()
    // The row moves to the other group. Keep a keyboard user's place: the next
    // row, else the one before, else the add field.
    if (!focused || box.isConnected) return
    const left = group?.isConnected ? boxes() : []
    const next = left[index] ?? left[left.length - 1]
    ;(next ?? document.getElementById('new-todo-title'))?.focus()
  }

  function chooseList(id: string) {
    if (id === selectedId) return
    navigate(`/dashboard/lists/${id}`)
  }

  async function retry() {
    retrying = true
    try {
      await Promise.all([lists.loadLists(), selectedId ? lists.loadList(selectedId) : undefined])
      const detailFailed = detail?.status === 'error' || detail?.refreshFailed
      // Still down: these would only be sent again.
      if (snapshot.listsStatus === 'ready' && !detailFailed) lists.retry()
    } finally {
      retrying = false
    }
  }
</script>

<TelemetryBoundary name="ListsPanel" {timing}>
  <div class="grid grid-cols-[minmax(0,1fr)] gap-6 md:grid-cols-[16rem_minmax(0,1fr)]">
    {#snippet unsavedTexts(items: UnsavedText[], canSubmit: (item: UnsavedText) => boolean)}
      {#if items.length > 0}
        <ul class="space-y-1 rounded-md border border-border p-2 text-sm" aria-label="Not saved">
          {#each items as item (item.id)}
            <li class="flex items-center gap-2">
              <span class="min-w-0 flex-1 wrap-anywhere">{item.text}</span>
              {#if canSubmit(item)}
                <Button type="button" size="sm" variant="outline" onclick={() => submitAgain(item)}>
                  Try again
                </Button>
              {/if}
              <Button
                type="button"
                size="sm"
                variant="ghost"
                onclick={() => lists.takeUnsaved(item.id)}
              >
                Discard
              </Button>
            </li>
          {/each}
        </ul>
      {/if}
    {/snippet}

    {#snippet todoRow(todo: TodoView, completed: boolean)}
      <li class="flex flex-wrap items-center gap-2 px-3 py-2">
        <input
          type="checkbox"
          class="size-4"
          checked={todo.done}
          aria-label={`Mark "${todo.title}" ${completed ? 'not done' : 'done'}`}
          onchange={(event) => toggleTodo(todo, event)}
        />
        {#if renamingTodo === (todo.key ?? todo.id)}
          <form
            class="flex min-w-0 flex-1 flex-wrap items-center gap-2"
            onsubmit={(event) => submitTodoRename(event)}
            novalidate
          >
            <label class="sr-only" for={`rename-todo-${todo.key ?? todo.id}`}>Todo title</label>
            <input
              id={`rename-todo-${todo.key ?? todo.id}`}
              class="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1 text-sm"
              bind:value={todoTitle}
              use:pasteNormalizer={(value) => (todoTitle = value)}
              onkeydown={(event) => {
                if (event.key === 'Escape') {
                  event.preventDefault()
                  cancelTodoRename()
                }
              }}
            />
            <Button type="submit" size="sm">Save</Button>
            <Button type="button" size="sm" variant="ghost" onclick={cancelTodoRename}>
              Cancel
            </Button>
            {#if todoRenameError}
              <p role="alert" class="basis-full text-xs text-destructive">{todoRenameError}</p>
            {/if}
          </form>
        {:else}
          <span
            class="min-w-0 flex-1 wrap-anywhere {completed
              ? 'text-muted-foreground line-through'
              : ''}"
          >
            {todo.title}
          </span>
          <div class="flex shrink-0 gap-1">
            <Button type="button" size="sm" variant="ghost" onclick={() => beginTodoRename(todo)}>
              Rename
            </Button>
            <Button
              type="button"
              size="sm"
              variant="ghost"
              onclick={(event) => removeTodo(todo, event)}
            >
              Delete
            </Button>
          </div>
        {/if}
      </li>
    {/snippet}

    <aside aria-label="Your lists" class="min-w-0 space-y-3">
      <div class="flex items-center justify-between gap-2">
        <h1 class="text-lg font-semibold">Lists</h1>
        {#if snapshot.listsStatus === 'ready'}
          <span class="text-xs text-muted-foreground">{snapshot.lists.length} / 100</span>
        {/if}
      </div>

      <form class="flex gap-2" onsubmit={submitList} novalidate>
        <label class="sr-only" for="new-list-name">New list name</label>
        <input
          id="new-list-name"
          class="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1.5 text-sm"
          bind:value={creatingName}
          aria-invalid={createError ? 'true' : undefined}
          aria-describedby={createError ? 'new-list-error' : undefined}
          use:pasteNormalizer={(value) => (creatingName = value)}
          placeholder="New list"
        />
        <Button type="submit" size="sm" variant="outline">Create</Button>
      </form>
      {#if createError}
        <p id="new-list-error" role="alert" class="text-xs text-destructive">{createError}</p>
      {/if}

      {@render unsavedTexts(unsavedNewLists, () => true)}
      {@render unsavedTexts(unsavedListRenames, canResubmit)}

      {#if snapshot.listsStatus === 'error'}
        <div role="alert" class="rounded-md border border-border p-3 text-sm">
          <p>Couldn't load your lists.</p>
          <Button type="button" size="sm" variant="outline" loading={retrying} onclick={retry}>
            Retry
          </Button>
        </div>
      {/if}
      {#if snapshot.listsStatus === 'idle' || snapshot.listsStatus === 'loading'}
        <div class="space-y-2" aria-label="Loading lists" data-testid="lists-skeleton">
          {#each [0, 1, 2] as index (index)}
            <div class="h-8 animate-pulse rounded bg-muted"></div>
          {/each}
        </div>
      {/if}
      {#if snapshot.lists.length === 0 && snapshot.listsStatus === 'ready'}
        <p class="text-sm text-muted-foreground">No lists yet. Create one to get started.</p>
      {:else if snapshot.lists.length > 0}
        <ul class="space-y-1">
          {#each snapshot.lists as list (list.id ?? list.key)}
            <li>
              <button
                type="button"
                class="flex w-full items-center justify-between gap-2 rounded-md px-2 py-1.5 text-left text-sm hover:bg-muted aria-[current=page]:bg-muted"
                aria-current={list.id === selectedId ? 'page' : undefined}
                disabled={list.pending || !list.id}
                onclick={() => list.id && chooseList(list.id)}
              >
                <span class="min-w-0 truncate">{list.name}</span>
                <span class="text-xs text-muted-foreground">{list.open_count}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </aside>

    <section aria-label="List" class="min-w-0">
      {#if detail?.status !== 'ready'}
        {@render unsavedTexts(unsavedNewTodos, () => false)}
        {@render unsavedTexts(unsavedTodoRenames, () => false)}
      {/if}
      {#if malformedId || detail?.status === 'not_found'}
        <div role="alert" class="rounded-md border border-border p-6 text-sm">
          <h2 class="text-lg font-semibold">List not found</h2>
          <p class="mt-1 text-muted-foreground">This list doesn't exist or isn't yours.</p>
          <a class="mt-3 inline-block underline" href="/dashboard/lists">Back to your lists</a>
        </div>
      {:else if !selectedId && snapshot.lists.length === 0 && snapshot.listsStatus !== 'ready'}
        <!-- Unknown yet whether there are lists. The picker shows any failure. -->
        {#if snapshot.listsStatus !== 'error'}
          <div class="h-20 animate-pulse rounded bg-muted" aria-hidden="true"></div>
        {/if}
      {:else if !selectedId}
        <div
          class="rounded-md border border-dashed border-border p-6 text-sm text-muted-foreground"
        >
          {#if snapshot.lists.length === 0}
            Create your first list on the left.
          {:else}
            Select a list to see its todos.
          {/if}
        </div>
      {:else if detail?.status === 'error'}
        <div role="alert" class="rounded-md border border-border p-6 text-sm">
          <p>Couldn't load this list.</p>
          <Button type="button" size="sm" variant="outline" loading={retrying} onclick={retry}>
            Retry
          </Button>
        </div>
      {:else if !detail || detail.status === 'idle' || detail.status === 'loading'}
        <div class="space-y-3" aria-label="Loading this list" data-testid="list-skeleton">
          <div class="h-6 w-40 animate-pulse rounded bg-muted"></div>
          {#each [0, 1, 2] as index (index)}
            <div class="h-8 animate-pulse rounded bg-muted"></div>
          {/each}
        </div>
      {:else}
        <div class="space-y-4">
          <div class="flex flex-wrap items-start justify-between gap-2">
            {#if renamingList}
              <form
                class="flex min-w-0 flex-1 flex-wrap items-center gap-2"
                onsubmit={submitListRename}
                novalidate
              >
                <label class="sr-only" for="rename-list-name">List name</label>
                <input
                  id="rename-list-name"
                  class="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1.5 text-xl font-semibold"
                  bind:value={listName}
                  use:pasteNormalizer={(value) => (listName = value)}
                  onkeydown={(event) => {
                    if (event.key === 'Escape') {
                      event.preventDefault()
                      cancelListRename()
                    }
                  }}
                />
                <Button type="submit" size="sm">Save</Button>
                <Button type="button" size="sm" variant="ghost" onclick={cancelListRename}>
                  Cancel
                </Button>
              </form>
            {:else}
              <h2 class="min-w-0 text-xl font-semibold wrap-anywhere">
                {currentList?.name ?? 'List'}
              </h2>
              <div class="flex shrink-0 gap-1">
                <Button type="button" size="sm" variant="ghost" onclick={beginListRename}>
                  Rename
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant="ghost"
                  loading={deleting}
                  onclick={openDelete}
                >
                  Delete list
                </Button>
              </div>
            {/if}
          </div>
          {#if listRenameError}
            <p role="alert" class="text-xs text-destructive">{listRenameError}</p>
          {/if}

          {#if detail.refreshFailed}
            <div role="alert" class="flex items-center gap-2 text-sm text-muted-foreground">
              <p>Couldn't refresh this list. It may be out of date.</p>
              <Button type="button" size="sm" variant="outline" loading={retrying} onclick={retry}>
                Retry
              </Button>
            </div>
          {/if}

          <form class="flex gap-2" onsubmit={submitTodo} novalidate>
            <label class="sr-only" for="new-todo-title">New todo title</label>
            <input
              id="new-todo-title"
              class="min-w-0 flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm"
              bind:value={addTitle}
              aria-invalid={addError ? 'true' : undefined}
              aria-describedby={addError ? 'new-todo-error' : undefined}
              use:pasteNormalizer={(value) => (addTitle = value)}
              placeholder="Add a todo..."
            />
            <Button type="submit" size="sm">Add</Button>
          </form>
          {#if addError}
            <p id="new-todo-error" role="alert" class="text-xs text-destructive">{addError}</p>
          {/if}

          {@render unsavedTexts(unsavedNewTodos, () => true)}
          {@render unsavedTexts(unsavedTodoRenames, canResubmit)}

          {#if todos.length === 0}
            <div
              class="rounded-md border border-dashed border-border p-6 text-sm text-muted-foreground"
            >
              No todos yet. Add your first one.
            </div>
          {:else}
            {#if openTodos.length > 0}
              <ul class="divide-y divide-border rounded-md border border-border" aria-label="Todos">
                {#each openTodos as todo (todo.key ?? todo.id)}
                  {@render todoRow(todo, false)}
                {/each}
              </ul>
            {/if}

            {#if doneTodos.length > 0}
              <div class="rounded-md border border-border">
                <button
                  type="button"
                  class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm font-medium"
                  aria-expanded={completedOpen}
                  onclick={() => (completedOpen = !completedOpen)}
                >
                  <span aria-hidden="true">{completedOpen ? '▾' : '▸'}</span>
                  Completed ({doneTodos.length})
                </button>
                {#if completedOpen}
                  <ul
                    class="divide-y divide-border border-t border-border"
                    aria-label="Completed todos"
                  >
                    {#each doneTodos as todo (todo.key ?? todo.id)}
                      {@render todoRow(todo, true)}
                    {/each}
                  </ul>
                {/if}
              </div>
            {/if}
          {/if}
        </div>
      {/if}
    </section>
  </div>

  {#if deleteOpen && currentList}
    <!-- Modal, so nothing behind it can be changed while the list is deleted. -->
    <dialog
      use:modal
      aria-labelledby="delete-list-title"
      aria-describedby="delete-list-detail"
      class="m-auto w-[calc(100%-2rem)] max-w-md rounded-lg border border-border bg-background p-4 text-sm text-foreground shadow-lg backdrop:bg-black/40"
      oncancel={(event) => {
        event.preventDefault()
        cancelDelete()
      }}
      onclose={closeDelete}
    >
      <form class="space-y-3" onsubmit={confirmDelete}>
        <h2 id="delete-list-title" class="text-lg font-semibold wrap-anywhere">
          Delete "{currentList.name}"?
        </h2>
        <p id="delete-list-detail" class="text-muted-foreground">
          It holds {currentList.todo_count}
          {currentList.todo_count === 1 ? 'todo' : 'todos'}, open and done. Deleting a list is
          permanent.
        </p>
        {#if snapshot.queueLength > 0}
          <p class="text-muted-foreground">Waiting for your changes to save...</p>
        {/if}
        {#if deleteError}
          <p role="alert" class="text-destructive">{deleteError}</p>
        {/if}
        <div class="flex justify-end gap-2">
          <Button
            id="cancel-delete-list"
            type="button"
            size="sm"
            variant="outline"
            disabled={deleting}
            onclick={cancelDelete}
          >
            Cancel
          </Button>
          <Button
            type="submit"
            size="sm"
            variant="destructive"
            loading={deleting}
            disabled={snapshot.queueLength > 0}
          >
            Delete list
          </Button>
        </div>
      </form>
    </dialog>
  {/if}
</TelemetryBoundary>
