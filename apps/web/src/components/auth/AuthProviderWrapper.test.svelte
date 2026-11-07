<script lang="ts">
  import AuthProvider from './AuthProvider.svelte'
  import AuthContextConsumer from './AuthContextConsumer.test.svelte'
  import type { User } from '../../lib/types'

  let { initialUser = null }: { initialUser?: User | null } = $props()
</script>

<AuthProvider {initialUser}>
  <AuthContextConsumer />
</AuthProvider>
