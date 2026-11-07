<script lang="ts">
  import { onMount } from 'svelte'
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
  } from '$lib/components/ui/card'
  import { api } from '../../lib/api'
  import { getSessionUser, setSessionUser } from '../../lib/auth/session'
  import { formatDate } from '../../lib/format'
  import { navigateReplace } from '../../lib/navigation'
  import { isAdminRole } from '../../lib/types'
  import type { AdminUserRow, User, UserRole } from '../../lib/types'

  const ROLES: UserRole[] = ['user', 'admin', 'super_admin']

  let currentUser = $state<User | null>(null)
  let loading = $state(true)
  let message = $state<string | null>(null)
  let error = $state<string | null>(null)

  // Users section
  let users = $state<AdminUserRow[]>([])
  let busyUserId = $state<string | null>(null)

  const isSuperAdmin = $derived(currentUser?.role === 'super_admin')

  onMount(async () => {
    // Confirm the viewer is an admin before loading anything; the backend
    // enforces this too, but redirecting avoids rendering a dead page.
    const cached = getSessionUser()
    if (cached) currentUser = cached

    // Fire the admin loads now, concurrently with the identity refresh; their
    // results are only consumed after we've confirmed the viewer is an admin.
    const adminData = api.admin.listUsers()
    // Swallow rejection here so a non-admin redirect can't leave an unhandled one.
    adminData.catch(() => {})

    try {
      const fresh = await api.auth.me()
      currentUser = fresh
      setSessionUser(fresh)
    } catch {
      // fall through to the admin check below
    }

    if (!isAdminRole(currentUser?.role)) {
      navigateReplace('/dashboard')
      return
    }

    try {
      const list = await adminData
      users = list.users
    } catch (loadError: unknown) {
      error = loadError instanceof Error ? loadError.message : 'Failed to load admin data'
    } finally {
      loading = false
    }

  })

  async function handleRoleChange(user: AdminUserRow, event: Event) {
    const role = (event.target as HTMLSelectElement).value as UserRole
    if (role === user.role) return
    busyUserId = user.id
    message = null
    error = null
    try {
      const updated = await api.admin.setRole(user.id, role)
      users = users.map((u) => (u.id === updated.id ? updated : u))
      message = `Updated ${updated.email} to ${updated.role}.`
    } catch (e: unknown) {
      error = e instanceof Error ? e.message : 'Failed to change role'
      // Reset the <select> back to the unchanged role. Replacing the row object
      // (not just the array) changes its identity so the one-way `value` binding
      // actually re-applies — spreading the array alone leaves the DOM showing
      // the failed selection.
      users = users.map((u) => (u.id === user.id ? { ...u } : u))
    } finally {
      busyUserId = null
    }
  }
</script>

<div class="space-y-6">
  <div>
    <h1 class="text-2xl font-semibold">Admin</h1>
    <p class="text-sm text-muted-foreground">User management.</p>
  </div>

  {#if message}
    <Alert>
      <CircleCheckIcon class="size-4" />
      <AlertDescription>{message}</AlertDescription>
    </Alert>
  {/if}
  {#if error}
    <Alert variant="destructive">
      <CircleAlertIcon class="size-4" />
      <AlertDescription>{error}</AlertDescription>
    </Alert>
  {/if}

  {#if loading}
    <p class="text-sm text-muted-foreground">Loading…</p>
  {:else}
    <Card>
      <CardHeader>
        <CardTitle>Users</CardTitle>
        <CardDescription>
          {isSuperAdmin ? 'Manage user roles.' : 'All registered users.'}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <div class="overflow-x-auto">
          <table class="w-full text-left text-sm">
            <thead class="text-muted-foreground">
              <tr class="border-b border-border">
                <th class="py-2 pr-4 font-medium">Email</th>
                <th class="py-2 pr-4 font-medium">Role</th>
                <th class="py-2 font-medium">Joined</th>
              </tr>
            </thead>
            <tbody>
              {#each users as user (user.id)}
                <tr class="border-b border-border/60">
                  <td class="py-2 pr-4">{user.email}</td>
                  <td class="py-2 pr-4">
                    {#if isSuperAdmin}
                      <select
                        class="rounded-md border border-input bg-background px-2 py-1"
                        value={user.role}
                        disabled={busyUserId === user.id}
                        onchange={(e) => handleRoleChange(user, e)}
                      >
                        {#each ROLES as role (role)}
                          <option value={role}>{role}</option>
                        {/each}
                      </select>
                    {:else}
                      {user.role}
                    {/if}
                  </td>
                  <td class="py-2 text-muted-foreground">
                    {user.created_at ? formatDate(user.created_at) : '—'}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
          {#if users.length === 0}
            <p class="py-4 text-sm text-muted-foreground">No users.</p>
          {/if}
        </div>
      </CardContent>
    </Card>
  {/if}
</div>
