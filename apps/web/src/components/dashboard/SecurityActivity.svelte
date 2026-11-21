<script lang="ts">
  // A Member's own Security activity (OT-008 slice 3). Loaded by its own
  // request with its own error and retry, so a failure here never blocks the
  // rest of Settings (dashboard rules).
  import { onMount } from 'svelte'
  import ShieldCheckIcon from '@lucide/svelte/icons/shield-check'
  import { Button } from '$lib/components/ui/button'
  import { api } from '../../lib/api'
  import { activityLine } from '../../lib/audit'
  import { formatDateTime } from '../../lib/format'
  import type { SecurityActivityEvent } from '../../lib/types'

  const PAGE_SIZE = 20

  // Bump to reload from the top (for example after a password change).
  let { refreshKey = 0 }: { refreshKey?: number } = $props()

  let events = $state<SecurityActivityEvent[]>([])
  let nextBeforeId = $state<number | null>(null)
  let loading = $state(true)
  let loadingMore = $state(false)
  let error = $state<string | null>(null)
  // Only the latest request may apply its result.
  let latestRequest = 0
  // What the failed request asked for, so Retry repeats exactly that.
  let failedBeforeId: number | null = null

  async function load(beforeId: number | null) {
    const request = ++latestRequest
    error = null
    if (beforeId === null) loading = true
    else loadingMore = true
    try {
      const page = await api.users.securityActivity({
        limit: PAGE_SIZE,
        before_id: beforeId ?? undefined,
      })
      if (request !== latestRequest) return
      events = beforeId === null ? page.events : [...events, ...page.events]
      nextBeforeId = page.next_before_id ?? null
    } catch (loadError: unknown) {
      if (request !== latestRequest) return
      failedBeforeId = beforeId
      error = loadError instanceof Error ? loadError.message : 'Could not load security activity'
    } finally {
      if (request === latestRequest) {
        loading = false
        loadingMore = false
      }
    }
  }

  onMount(() => {
    void load(null)
  })

  // Reload when the parent reports a change (skipping the first run, which
  // onMount already covers).
  let seenKey = refreshKey
  $effect(() => {
    if (refreshKey !== seenKey) {
      seenKey = refreshKey
      void load(null)
    }
  })
</script>

<section
  aria-labelledby="settings-activity"
  class="overflow-hidden rounded-[var(--radius)] border border-border bg-card"
  data-testid="security-activity"
>
  <h2
    id="settings-activity"
    class="border-b border-border px-5 py-3 text-xs font-semibold tracking-wide text-muted-foreground uppercase"
  >
    Security activity
  </h2>

  {#if loading}
    <div aria-hidden="true" data-testid="activity-skeleton">
      {#each [1, 2, 3] as row (row)}
        <div class="flex flex-col gap-2 border-b border-border px-5 py-4 last:border-b-0">
          <div class="h-3.5 w-40 animate-pulse rounded bg-muted"></div>
          <div class="h-3.5 w-56 max-w-full animate-pulse rounded bg-muted"></div>
        </div>
      {/each}
    </div>
  {:else if error && events.length === 0}
    <div class="flex flex-wrap items-center justify-between gap-3 px-5 py-4" role="alert">
      <p class="text-sm text-destructive">Couldn't load your security activity. {error}</p>
      <Button type="button" variant="outline" size="sm" onclick={() => load(null)}>Retry</Button>
    </div>
  {:else if events.length === 0}
    <div class="flex items-center gap-3 px-5 py-4">
      <span
        class="flex size-9 shrink-0 items-center justify-center rounded-full bg-cloud-2 text-muted-foreground"
        aria-hidden="true"
      >
        <ShieldCheckIcon class="size-4" />
      </span>
      <p class="text-sm text-muted-foreground">No activity yet.</p>
    </div>
  {:else}
    <ol>
      {#each events as event (event.id)}
        {@const line = activityLine(event)}
        <li
          class="flex flex-col gap-0.5 border-b border-border px-5 py-3 last:border-b-0"
          data-testid="activity-event"
        >
          <p class="text-sm font-medium">{line.title}</p>
          <p class="text-sm text-muted-foreground">
            {line.by} ·
            <time datetime={event.occurred_at}>{formatDateTime(event.occurred_at)}</time>
          </p>
        </li>
      {/each}
    </ol>
    {#if error}
      <div class="flex flex-wrap items-center gap-3 px-5 py-3" role="alert">
        <p class="text-sm text-destructive">{error}</p>
        <Button type="button" variant="outline" size="sm" onclick={() => load(failedBeforeId)}>
          Retry
        </Button>
      </div>
    {:else if nextBeforeId !== null}
      <div class="px-5 py-3">
        <Button
          type="button"
          variant="outline"
          size="sm"
          loading={loadingMore}
          onclick={() => load(nextBeforeId)}
        >
          Load more
        </Button>
      </div>
    {/if}
  {/if}
</section>
