<script lang="ts">
  import AuthGuard from '../auth/AuthGuard.svelte'
  import DashboardShell from './DashboardShell.svelte'

  interface Props {
    children?: import('svelte').Snippet
    skipVerificationCheck?: boolean
  }

  let { children, skipVerificationCheck = false }: Props = $props()
</script>

<AuthGuard {skipVerificationCheck}>
  <DashboardShell>
    {@render children?.()}
  </DashboardShell>
</AuthGuard>
