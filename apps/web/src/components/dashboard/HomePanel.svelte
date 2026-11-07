<script lang="ts">
  import { onMount } from 'svelte'
  import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '$lib/components/ui/card'
  import { api } from '../../lib/api'
  import { getSessionUser, setSessionUser } from '../../lib/auth/session'
  import type { User } from '../../lib/types'

  // Placeholder home: replace with the product's first real panel. It loads the
  // current user through the API client, so it already exercises the
  // authenticated request path (401 → silent refresh → retry).
  let user = $state<User | null>(getSessionUser())
  let error = $state<string | null>(null)

  onMount(async () => {
    try {
      const fresh = await api.auth.me()
      user = fresh
      setSessionUser(fresh)
    } catch (loadError: unknown) {
      error = loadError instanceof Error ? loadError.message : 'Failed to load your account'
    }
  })
</script>

<div class="space-y-6">
  <div>
    <h1 class="text-2xl font-semibold">Home</h1>
    <p class="text-sm text-muted-foreground">
      {user ? `Signed in as ${user.email}` : 'Loading…'}
    </p>
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Start building</CardTitle>
      <CardDescription>
        This panel is a placeholder. Add an API group in <code>src/lib/api</code>, a use case
        behind a port in the backend, and render it here.
      </CardDescription>
    </CardHeader>
    <CardContent>
      {#if error}
        <p class="text-sm text-destructive">{error}</p>
      {:else}
        <p class="text-sm text-muted-foreground">
          Auth, settings, account deletion and the admin panel already work.
        </p>
      {/if}
    </CardContent>
  </Card>
</div>
