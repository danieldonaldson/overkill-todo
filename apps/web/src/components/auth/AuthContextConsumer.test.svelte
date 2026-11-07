<script lang="ts">
  import { getContext } from 'svelte'
  import { AUTH_CONTEXT_KEY, type AuthContext } from '../../lib/auth/cookie'

  const auth = getContext<AuthContext>(AUTH_CONTEXT_KEY)
</script>

<div data-testid="context-consumer">
  <span data-testid="is-authenticated">{auth.isAuthenticated}</span>
  <span data-testid="is-loading">{auth.isLoading}</span>
  <span data-testid="user-email">{auth.user?.email ?? 'none'}</span>
  <button type="button" data-testid="logout-btn" onclick={() => auth.logout()}>Logout</button>
</div>
