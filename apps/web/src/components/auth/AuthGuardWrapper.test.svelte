<script lang="ts">
  import AuthGuard from './AuthGuard.svelte'
  import type { User } from '../../lib/types'

  let {
    onAuthenticated,
    childText = 'Protected content',
    skipVerificationCheck = false,
  }: {
    onAuthenticated?: (user: User) => void
    childText?: string
    skipVerificationCheck?: boolean
  } = $props()
</script>

<AuthGuard {onAuthenticated} {skipVerificationCheck}>
  <div data-testid="protected-content">{childText}</div>
</AuthGuard>
