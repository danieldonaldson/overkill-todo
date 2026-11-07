<script lang="ts">
  import AuthProvider from '../auth/AuthProvider.svelte'
  import DashboardNav from './DashboardNav.svelte'
</script>

<AuthProvider>
  <DashboardNav />
</AuthProvider>
