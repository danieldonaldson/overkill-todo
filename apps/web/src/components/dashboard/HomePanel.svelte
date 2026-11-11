<script lang="ts">
  import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
  } from '$lib/components/ui/card'
  import { getCurrentUser } from '../../lib/auth/current-user'

  // Placeholder home: replace with the product's first real panel. The user
  // comes from AuthGuard, which already fetched /auth/me for this page load.
  const user = getCurrentUser()
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
