<script lang="ts">
  import { onMount } from 'svelte'
  import { api } from '../../lib/api'
  import { clearSessionUser, setSessionUser } from '../../lib/auth/session'
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

  onMount(async () => {
    try {
      const user = await api.auth.me()
      setSessionUser(user)

      if (!skipVerificationCheck && !user.email_verified) {
        navigateReplace(`/verify-email?email=${encodeURIComponent(user.email)}`)
        return
      }

      authenticated = true
      onAuthenticated?.(user)
    } catch {
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
