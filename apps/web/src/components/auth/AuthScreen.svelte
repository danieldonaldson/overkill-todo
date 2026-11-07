<script lang="ts">
  import AuthTabs from './AuthTabs.svelte'
  import GuestGuard from './GuestGuard.svelte'

  // One island, not two: an Astro page can only hydrate the component it names,
  // so <GuestGuard client:load><AuthTabs client:load /></GuestGuard> in .astro
  // would slot the tabs in as inert HTML. Composing them here keeps both live.

  interface Props {
    initial?: 'login' | 'register'
  }

  let { initial = 'login' }: Props = $props()
</script>

<GuestGuard>
  <AuthTabs {initial} />
</GuestGuard>
