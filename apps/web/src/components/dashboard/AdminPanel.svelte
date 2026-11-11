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
  import { isApiStatus } from '../../lib/api/errors'
  import { session } from '../../lib/auth/session.svelte'
  import { formatDate } from '../../lib/format'
  import { navigateReplace } from '../../lib/navigation'
  import { isAdminRole } from '../../lib/types'
  import type { AdminUserRow, UserRole } from '../../lib/types'

  const ROLES: UserRole[] = ['user', 'admin', 'super_admin']

  let loading = $state(true)
  let message = $state<string | null>(null)
  let error = $state<string | null>(null)

  // Users section
  let users = $state<AdminUserRow[]>([])
  let busyUserId = $state<string | null>(null)

  // Role selects need a known super-admin; until then the table is read-only.
  const isSuperAdmin = $derived(session.user?.role === 'super_admin')
  // Only a confirmed non-admin role redirects: a stale or missing cache is
  // not enough to send an Admin away (design D5).
  const confirmedNonAdmin = $derived(
    session.status === 'confirmed' && !isAdminRole(session.user?.role)
  )
  let leaving = $state(false)

  function leave() {
    if (leaving) return
    leaving = true
    navigateReplace('/dashboard')
  }

  $effect(() => {
    if (confirmedNonAdmin) leave()
  })

  // Load at once without checking the role first (D5): the server decides,
  // and a 403 sends a non-admin Home before any list could be shown.
  onMount(async () => {
    try {
      const list = await api.admin.listUsers()
      users = list.users
    } catch (loadError: unknown) {
      if (isApiStatus(loadError, 403)) {
        leave()
        return
      }
      error = loadError instanceof Error ? loadError.message : 'Failed to load admin data'
    } finally {
      loading = false
    }
  })

  async function handleRoleChange(user: AdminUserRow, event: Event) {
    const select = event.target as HTMLSelectElement
    const role = select.value as UserRole
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
      // Reset the <select> to the unchanged role by hand. Re-rendering can't do
      // it: Svelte remembers the value it last applied (`user.role`), sees the
      // same value again, and leaves the DOM showing the failed selection.
      select.value = user.role
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

  {#if leaving}
    <!-- On the way Home: never show a list to a non-admin. -->
  {:else if loading}
    <Card aria-hidden="true" data-testid="admin-skeleton">
      <CardHeader>
        <div class="h-4 w-16 animate-pulse rounded bg-muted"></div>
        <div class="h-3.5 w-40 animate-pulse rounded bg-muted"></div>
      </CardHeader>
      <CardContent>
        {#each Array.from({ length: 5 }, (_, i) => i) as row (row)}
          <div class="flex items-center gap-4 border-b border-border/60 py-3">
            <div class="h-3.5 w-48 animate-pulse rounded bg-muted"></div>
            <div class="h-3.5 w-16 animate-pulse rounded bg-muted"></div>
            <div class="ml-auto h-3.5 w-20 animate-pulse rounded bg-muted"></div>
          </div>
        {/each}
      </CardContent>
    </Card>
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
