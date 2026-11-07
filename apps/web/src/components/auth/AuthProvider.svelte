<script lang="ts">
  import { setContext } from 'svelte'
  import { api } from '../../lib/api'
  import { AUTH_CONTEXT_KEY, type AuthContext } from '../../lib/auth/cookie'
  import { clearSessionUser, getSessionUser } from '../../lib/auth/session'
  import { navigateReplace } from '../../lib/navigation'
  import type { User } from '../../lib/types'

  interface Props {
    initialUser?: User | null
    children?: import('svelte').Snippet
  }

  let { initialUser = null, children }: Props = $props()

  function getInitialUser(): User | null {
    if (initialUser !== null) return initialUser
    if (typeof window !== 'undefined') return getSessionUser()
    return null
  }

  let user = $state<User | null>(getInitialUser())
  let isLoading = $state(false)

  const isAuthenticated = $derived(user !== null)

  async function logout(): Promise<void> {
    try {
      await api.auth.logout()
    } catch {
      // Logout is best-effort; local client state must still be cleared.
    }

    user = null
    clearSessionUser()
    navigateReplace('/login')
  }

  const authContext: AuthContext = {
    get user() {
      return user
    },
    get isAuthenticated() {
      return isAuthenticated
    },
    get isLoading() {
      return isLoading
    },
    logout,
  }

  setContext(AUTH_CONTEXT_KEY, authContext)

  export function setUser(newUser: User | null) {
    user = newUser
  }

  export function setLoading(loading: boolean) {
    isLoading = loading
  }
</script>

{@render children?.()}
