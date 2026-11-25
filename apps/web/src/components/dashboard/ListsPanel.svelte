<script lang="ts">
  import TelemetryBoundary from '$lib/telemetry/TelemetryBoundary.svelte'
  import { createIslandTiming } from '$lib/telemetry/island-timing'
  import { Button } from '$lib/components/ui/button'
  import { navigate } from '$lib/navigation'
  import { onMount } from 'svelte'
  import { lists } from '../../lib/lists/store.svelte'
  import type { TodoView } from '../../lib/lists/engine'

  // One page for every list address: the shell is built once with a
  // placeholder id and the web server rewrites real addresses to it (design
  // D8). The panel reads the list id from `location.pathname`; an id that isn't
  // a UUID shows "list not found" without a request.
  const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

  function idFromPath(): string | null {
    if (typeof location === 'undefined') return null
    const match = location.pathname.replace(/\/+$/, '').match(/^\/dashboard\/lists\/([^/]+)$/)
    if (!match) return null
    try {
      return decodeURIComponent(match[1])
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

  const snapshot = $derived(lists.snapshot)
  const currentList = $derived(snapshot.lists.find((list) => list.id === selectedId))
  const detail = $derived(selectedId ? snapshot.details[selectedId] : undefined)
  const todos = $derived(detail?.todos ?? [])
  const openTodos = $derived(todos.filter((todo) => !todo.done))
  const doneTodos = $derived(todos.filter((todo) => todo.done))

  const timing = createIslandTiming()
  const ready = $derived(
    snapshot.listsStatus === 'ready' ||
      snapshot.listsStatus === 'error' ||
      detail?.status === 'ready' ||
      detail?.status === 'not_found' ||
      detail?.status === 'error' ||
      malformedId
  )
  $effect(() => {
    if (ready) timing.markReady()
  })

  $effect(() => {
    const id = snapshot.lastCreatedListId
    if (id && selectedId !== id && location.pathname === '/dashboard/lists') {
      // Keep the router's own `history.state`: only the address changes, so the
      // queue and the persisted header survive (design D3).
      history.replaceState(history.state, '', `/dashboard/lists/${id}`)
      selectedId = id
      malformedId = false
      void lists.loadList(id)
    }
  })

  $effect(() => {
    const restored = lists.restoreListName
    if (restored !== null) {
      creatingName = restored
      lists.consumeListRestore()
    }
  })

  $effect(() => {
    const restored = lists.restoreTodo
    if (restored !== null && restored.listId === selectedId) {
      addTitle = restored.title
      lists.consumeTodoRestore()
    }
  })

  onMount(() => {
    void lists.loadLists()
    if (selectedId) void lists.loadList(selectedId)
  })

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
    lists.createList(name)
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
  function pasteNormalizer(node: HTMLInputElement) {
    const handler = (event: Event) => {
      const paste = event as ClipboardEvent
      // A single-line input strips line breaks on paste; put spaces in instead.
      const text = paste.clipboardData?.getData('text/plain')
      if (!text || !/[\r\n]/.test(text)) return
      paste.preventDefault()
      const start = node.selectionStart ?? node.value.length
      const end = node.selectionEnd ?? node.value.length
      node.setRangeText(text.replace(/[ \t]*[\r\n]+[ \t]*/g, ' '), start, end, 'end')
      addTitle = node.value
    }
    node.addEventListener('paste', handler)
    return { destroy: () => node.removeEventListener('paste', handler) }
  }

  function toggleTodo(todo: TodoView) {
    if (selectedId) lists.markTodo(selectedId, todo, !todo.done)
  }

  function chooseList(id: string) {
    if (id === selectedId) return
    navigate(`/dashboard/lists/${id}`)
  }

  async function retry() {
    retrying = true
    try {
      await lists.loadLists()
      if (selectedId) await lists.loadList(selectedId)
      lists.retry()
    } finally {
      retrying = false
    }
  }
</script>

<TelemetryBoundary name="ListsPanel" {timing}>
  <div class="grid gap-6 md:grid-cols-[16rem_1fr]">
    <aside aria-label="Your lists" class="space-y-3">
      <div class="flex items-center justify-between gap-2">
        <h1 class="text-lg font-semibold">Lists</h1>
        <span class="text-xs text-muted-foreground">{snapshot.lists.length} / 100</span>
      </div>

      <form class="flex gap-2" onsubmit={submitList} novalidate>
        <label class="sr-only" for="new-list-name">New list name</label>
        <input
          id="new-list-name"
          class="min-w-0 flex-1 rounded-md border border-input bg-background px-2 py-1.5 text-sm"
          bind:value={creatingName}
          aria-invalid={createError ? 'true' : undefined}
          aria-describedby={createError ? 'new-list-error' : undefined}
          placeholder="New list"
        />
        <Button type="submit" size="sm" variant="outline">Create</Button>
      </form>
      {#if createError}
        <p id="new-list-error" role="alert" class="text-xs text-destructive">{createError}</p>
      {/if}

      {#if snapshot.listsStatus === 'loading' && snapshot.lists.length === 0}
        <div class="space-y-2" aria-label="Loading lists" data-testid="lists-skeleton">
          {#each [0, 1, 2] as index (index)}
            <div class="h-8 animate-pulse rounded bg-muted"></div>
          {/each}
        </div>
      {:else if snapshot.listsStatus === 'error' && snapshot.lists.length === 0}
        <div role="alert" class="rounded-md border border-border p-3 text-sm">
          <p>Couldn't load your lists.</p>
          <Button type="button" size="sm" variant="outline" loading={retrying} onclick={retry}>
            Retry
          </Button>
        </div>
      {:else if snapshot.lists.length === 0}
        <p class="text-sm text-muted-foreground">No lists yet. Create one to get started.</p>
      {:else}
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
                <span class="truncate">{list.name}</span>
                <span class="text-xs text-muted-foreground">{list.open_count}</span>
              </button>
            </li>
          {/each}
        </ul>
      {/if}
    </aside>

    <section aria-label="List" class="min-w-0">
      {#if malformedId || detail?.status === 'not_found'}
        <div role="alert" class="rounded-md border border-border p-6 text-sm">
          <h2 class="text-lg font-semibold">List not found</h2>
          <p class="mt-1 text-muted-foreground">This list doesn't exist or isn't yours.</p>
          <a class="mt-3 inline-block underline" href="/dashboard/lists">Back to your lists</a>
        </div>
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
          <h2 class="text-xl font-semibold">{currentList?.name ?? 'List'}</h2>

          <form class="flex gap-2" onsubmit={submitTodo} novalidate>
            <label class="sr-only" for="new-todo-title">New todo title</label>
            <input
              id="new-todo-title"
              class="min-w-0 flex-1 rounded-md border border-input bg-background px-3 py-2 text-sm"
              bind:value={addTitle}
              aria-invalid={addError ? 'true' : undefined}
              aria-describedby={addError ? 'new-todo-error' : undefined}
              use:pasteNormalizer
              placeholder="Add a todo..."
            />
            <Button type="submit" size="sm">Add</Button>
          </form>
          {#if addError}
            <p id="new-todo-error" role="alert" class="text-xs text-destructive">{addError}</p>
          {/if}

          {#if todos.length === 0}
            <div
              class="rounded-md border border-dashed border-border p-6 text-sm text-muted-foreground"
            >
              No todos yet. Add your first one.
            </div>
          {:else}
            <ul class="divide-y divide-border rounded-md border border-border" aria-label="Todos">
              {#each openTodos as todo (todo.id ?? todo.key)}
                <li class="flex items-center gap-3 px-3 py-2">
                  <input
                    type="checkbox"
                    class="size-4"
                    checked={todo.done}
                    aria-label={`Mark "${todo.title}" done`}
                    onchange={() => toggleTodo(todo)}
                  />
                  <span class="min-w-0 flex-1 break-words">{todo.title}</span>
                </li>
              {/each}
            </ul>

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
                    {#each doneTodos as todo (todo.id ?? todo.key)}
                      <li class="flex items-center gap-3 px-3 py-2">
                        <input
                          type="checkbox"
                          class="size-4"
                          checked={todo.done}
                          aria-label={`Mark "${todo.title}" not done`}
                          onchange={() => toggleTodo(todo)}
                        />
                        <span class="min-w-0 flex-1 break-words text-muted-foreground line-through">
                          {todo.title}
                        </span>
                      </li>
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
</TelemetryBoundary>
