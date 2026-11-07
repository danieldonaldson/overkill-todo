<script lang="ts">
  import GuestGuard from './GuestGuard.svelte'

  let { childText = 'Sign in form' }: { childText?: string } = $props()
</script>

<GuestGuard>
  <div data-testid="guest-content">{childText}</div>
</GuestGuard>
