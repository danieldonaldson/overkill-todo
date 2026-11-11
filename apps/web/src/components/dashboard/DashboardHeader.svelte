<script lang="ts">
  import { onMount } from 'svelte'
  import CloudOffIcon from '@lucide/svelte/icons/cloud-off'
  import { Button } from '$lib/components/ui/button'
  import { session } from '../../lib/auth/session.svelte'
  import DashboardNav from './DashboardNav.svelte'

  // Persisted across dashboard moves (`transition:persist` in
  // DashboardLayout.astro), so this mounts once per document and must hold no
  // page-specific state. Mounting starts the one background account check
  // (design D3), after the session-hint backstop (D2).
  onMount(() => {
    session.init()
  })

  let retrying = $state(false)

  async function handleRetry() {
    retrying = true
    try {
      await session.retry()
    } finally {
      retrying = false
    }
  }
</script>

<header class="sticky top-0 z-50 border-b border-border bg-background/95 backdrop-blur">
  <div class="mx-auto flex h-16 w-full max-w-6xl items-center gap-6 px-4">
    <a href="/dashboard" class="flex items-center gap-2 text-lg font-semibold text-primary">
      <span
        class="flex size-8 items-center justify-center rounded-md bg-primary text-sm font-bold text-primary-foreground"
        aria-hidden="true"
      >
        t
      </span>
      <span>Overkill Todo</span>
    </a>

    <DashboardNav />
  </div>
</header>

{#if session.status === 'unreachable'}
  <!-- Non-blocking (D4): the page stays usable with what the tab remembers. -->
  <div role="status" aria-live="polite" class="border-b border-border bg-muted/60">
    <div class="mx-auto flex w-full max-w-6xl items-center gap-3 px-4 py-2 text-sm">
      <CloudOffIcon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      <p class="flex-1">Couldn't reach the server. Some details may be out of date.</p>
      <Button type="button" variant="outline" size="sm" loading={retrying} onclick={handleRetry}>
        Retry
      </Button>
    </div>
  </div>
{/if}
