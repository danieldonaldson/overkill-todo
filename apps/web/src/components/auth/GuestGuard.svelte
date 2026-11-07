<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '../../lib/api'
  import { clearSessionUser, getSessionUser, setSessionUser } from '../../lib/auth/session'
  import { getTokenExpiry } from '../../lib/auth/token'
  import { navigateReplace } from '../../lib/navigation'
  import { Spinner } from '$lib/components/ui/spinner'

  interface Props {
    children?: import('svelte').Snippet
  }

  let { children }: Props = $props()

  /**
   * Does this browser look like it holds a session worth verifying?
   *
   * The JWT and refresh token live in HttpOnly cookies we can't read, so we go
   * by the local traces a login leaves behind: the token expiry in localStorage
   * (survives tab close) and the cached user in sessionStorage. Either one being
   * present means "probably signed in" — the expiry may well be *past*, since
   * access tokens are short-lived and the refresh cookie outlives them by days.
   *
   * Gating the /auth/me call on this is not just an optimisation. For a visitor
   * with no session, that call 401s, HttpClient fails the refresh, and it hard
   * redirects to /login (client.ts) — from the login page itself, that is a
   * reload loop. No hint, no call, no loop.
   */
  function hasSessionHint(): boolean {
    if (typeof window === 'undefined') return false
    return getTokenExpiry() !== null || getSessionUser() !== null
  }

  // Computed before first paint, so a signed-out visitor renders the form
  // immediately with no spinner flash.
  let checking = $state(hasSessionHint())

  onMount(async () => {
    if (!checking) return

    try {
      const user = await api.auth.me()
      setSessionUser(user)
      // Deliberately no `checking = false` on this path: keep the spinner up
      // through the navigation so the auth form never flashes into view.
      navigateReplace(
        user.email_verified ? '/dashboard' : `/verify-email?email=${encodeURIComponent(user.email)}`,
      )
    } catch {
      // Stale hint — the cookies are gone or rejected. Show the auth form.
      clearSessionUser()
      checking = false
    }
  })
</script>

{#if checking}
  <div class="flex min-h-64 flex-col items-center justify-center gap-4 text-muted-foreground">
    <Spinner size="lg" />
    <p class="text-sm">Loading...</p>
  </div>
{:else}
  {@render children?.()}
{/if}
