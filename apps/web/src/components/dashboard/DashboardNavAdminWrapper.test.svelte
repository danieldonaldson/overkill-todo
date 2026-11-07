<script lang="ts">
  import AuthProvider from '../auth/AuthProvider.svelte'
  import DashboardNav from './DashboardNav.svelte'
  import type { User } from '../../lib/types'

  const adminUser: User = {
    id: 'admin-1',
    email: 'admin@example.com',
    first_name: 'Ad',
    last_name: 'Min',
    email_verified: true,
    has_password: true,
    oauth_providers: [],
    role: 'super_admin',
  }
</script>

<AuthProvider initialUser={adminUser}>
  <DashboardNav />
</AuthProvider>
