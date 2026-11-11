<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '../../lib/api'
  import { setCurrentUser } from '../../lib/auth/current-user'
  import { clearSessionUser, getSessionUser, setSessionUser } from '../../lib/auth/session'
  import { getTokenExpiry } from '../../lib/auth/token'
  import { navigateReplace } from '../../lib/navigation'
  import { Spinner } from '$lib/components/ui/spinner'
  import type { User } from '../../lib/types'

  interface Props {
    children?: import('svelte').Snippet
    onAuthenticated?: (user: User) => void
    skipVerificationCheck?: boolean
  }

  let { children, onAuthenticated, skipVerificationCheck = false }: Props = $props()

  let checking = $state(true)
  let authenticated = $state(false)

  /**
   * The tab's cached user, if it can stand in for /auth/me on this page load.
   *
   * Trusted only alongside a stored token expiry (the tab believes it has a
   * session) and only once verified (an unverified user must keep hitting the
   * server so the verify-email redirect clears after they verify). The server
   * still checks every API call: if the session has ended, the next one gets a
   * 401, the client clears this cache and redirects to /login. Until then the
   * page may show a stale name or role; the backend enforces the real role.
   */
  function trustedCachedUser(): User | null {
    const cached = getSessionUser()
    if (!cached || getTokenExpiry() === null) return null
    return cached.email_verified || skipVerificationCheck ? cached : null
  }

  onMount(async () => {
    try {
      const user = trustedCachedUser() ?? (await api.auth.me())
      setSessionUser(user)

      if (!skipVerificationCheck && !user.email_verified) {
        navigateReplace(`/verify-email?email=${encodeURIComponent(user.email)}`)
        return
      }

      // Publish before rendering the slot: the panel islands in it read this
      // on mount instead of fetching /auth/me again.
      setCurrentUser(user)
      authenticated = true
      onAuthenticated?.(user)
    } catch {
      setCurrentUser(null)
      clearSessionUser()
      navigateReplace('/login')
    } finally {
      checking = false
    }
  })
</script>

{#if checking}
  <div class="flex min-h-screen flex-col items-center justify-center gap-4 text-muted-foreground">
    <Spinner size="lg" />
    <p class="text-sm">Loading...</p>
  </div>
{:else if authenticated}
  {@render children?.()}
{/if}
