<script lang="ts">
  import DashboardShell from './DashboardShell.svelte'

  let { childText = 'Dashboard content' }: { childText?: string } = $props()
</script>

<DashboardShell>
  <div data-testid="shell-content">{childText}</div>
</DashboardShell>
