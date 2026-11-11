<script lang="ts">
  import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
  } from '$lib/components/ui/card'
  import { session } from '../../lib/auth/session.svelte'

  // Placeholder home: replace with the product's first real panel. The user
  // comes from the tab's session store; it is null only until the background
  // check answers in a tab with nothing cached (design D4).
</script>

<div class="space-y-6">
  <div>
    <h1 class="text-2xl font-semibold">Home</h1>
    {#if session.user}
      <p class="text-sm text-muted-foreground">Signed in as {session.user.email}</p>
    {:else}
      <div class="flex h-5 items-center" aria-hidden="true">
        <div class="h-3.5 w-56 animate-pulse rounded bg-muted"></div>
      </div>
    {/if}
  </div>

  <Card>
    <CardHeader>
      <CardTitle>Start building</CardTitle>
      <CardDescription>
        This panel is a placeholder. Add an API group in <code>src/lib/api</code>, a use case behind
        a port in the backend, and render it here.
      </CardDescription>
    </CardHeader>
    <CardContent>
      <p class="text-sm text-muted-foreground">
        Auth, settings, account deletion and the admin panel already work.
      </p>
    </CardContent>
  </Card>
</div>
