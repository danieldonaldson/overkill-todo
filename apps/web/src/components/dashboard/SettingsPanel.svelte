<script lang="ts">
  import { onMount } from 'svelte'
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import CircleCheckIcon from '@lucide/svelte/icons/circle-check'
  import MailIcon from '@lucide/svelte/icons/mail'
  import KeyRoundIcon from '@lucide/svelte/icons/key-round'
  import LinkIcon from '@lucide/svelte/icons/link'
  import DownloadIcon from '@lucide/svelte/icons/download'
  import Trash2Icon from '@lucide/svelte/icons/trash-2'
  import LogOutIcon from '@lucide/svelte/icons/log-out'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import { FormInput } from '$lib/components/ui/form-input'
  import { api } from '../../lib/api'
  import { clearSessionUser, getSessionUser, setSessionUser } from '../../lib/auth/session'
  import { formatDateTime } from '../../lib/format'
  import { navigateReplace } from '../../lib/navigation'
  import type { User } from '../../lib/types'
  import { minLength, required, validate } from '../../lib/utils/validation'

  let user = $state<User | null>(null)
  let loading = $state(true)
  let savingPassword = $state(false)
  let unlinkingGoogle = $state(false)
  let currentPassword = $state('')
  let newPassword = $state('')
  let currentPasswordError = $state<string | null>(null)
  let newPasswordError = $state<string | null>(null)
  let message = $state<string | null>(null)
  let error = $state<string | null>(null)
  let exporting = $state(false)
  let deleteConfirmation = $state('')
  let deletePassword = $state('')
  let requestingDeletion = $state(false)
  let cancelingDeletion = $state(false)

  const googleLinked = $derived(user?.oauth_providers?.includes('google') ?? false)
  const deletionScheduledAt = $derived(user?.deletion_scheduled_at ?? null)


  onMount(async () => {
    // Paint instantly from cache, but always revalidate: this page renders
    // has_password / oauth_providers, which the cache can have stale.
    const cached = getSessionUser()
    if (cached) user = cached
    try {
      const fresh = await api.auth.me()
      user = fresh
      setSessionUser(fresh)
    } catch (loadError: unknown) {
      // Only surface the error if we have nothing cached to show.
      if (!cached) {
        error = loadError instanceof Error ? loadError.message : 'Failed to load account settings'
      }
    } finally {
      loading = false
    }
  })

  function validatePasswordForm() {
    currentPasswordError = user?.has_password ? validate(currentPassword, [required()]) : null
    newPasswordError = validate(newPassword, [
      required(),
      minLength(8, 'Password must be at least 8 characters'),
    ])
  }

  async function handlePasswordSubmit(event: SubmitEvent) {
    event.preventDefault()
    if (!user) return

    validatePasswordForm()
    if (currentPasswordError || newPasswordError) return

    savingPassword = true
    message = null
    error = null

    try {
      if (user.has_password) {
        await api.users.changePassword({
          current_password: currentPassword,
          new_password: newPassword,
        })
        message = 'Password changed.'
      } else {
        await api.users.setPassword({ new_password: newPassword })
        user = { ...user, has_password: true }
        setSessionUser(user)
        message = 'Password set.'
      }
      currentPassword = ''
      newPassword = ''
    } catch (saveError: unknown) {
      error = saveError instanceof Error ? saveError.message : 'Password update failed'
    } finally {
      savingPassword = false
    }
  }

  async function handleLinkGoogle() {
    error = null
    message = null
    try {
      await api.oauth.startLink('google')
    } catch (linkError: unknown) {
      error = linkError instanceof Error ? linkError.message : 'Failed to start Google linking'
    }
  }

  async function handleUnlinkGoogle() {
    if (!user) return

    unlinkingGoogle = true
    error = null
    message = null

    try {
      await api.oauth.unlink('google')
      user = {
        ...user,
        oauth_providers: user.oauth_providers.filter((provider) => provider !== 'google'),
      }
      setSessionUser(user)
      message = 'Google account unlinked.'
    } catch (unlinkError: unknown) {
      error = unlinkError instanceof Error ? unlinkError.message : 'Failed to unlink Google'
    } finally {
      unlinkingGoogle = false
    }
  }

  async function handleExportData() {
    exporting = true
    error = null
    message = null
    try {
      const data = await api.users.exportData()
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' })
      const url = URL.createObjectURL(blob)
      const anchor = document.createElement('a')
      anchor.href = url
      anchor.download = `app-data-export-${data.id}.json`
      document.body.appendChild(anchor)
      anchor.click()
      anchor.remove()
      URL.revokeObjectURL(url)
      message = 'Your data export has been downloaded.'
    } catch (exportError: unknown) {
      error = exportError instanceof Error ? exportError.message : 'Failed to export your data'
    } finally {
      exporting = false
    }
  }

  async function handleRequestDeletion() {
    if (!user) return
    error = null
    message = null

    if (deleteConfirmation !== 'DELETE') {
      error = 'Type DELETE to confirm account deletion.'
      return
    }
    if (user.has_password && !deletePassword) {
      error = 'Enter your password to confirm account deletion.'
      return
    }

    requestingDeletion = true
    try {
      const result = await api.users.requestDeletion({
        confirmation: deleteConfirmation,
        ...(user.has_password ? { password: deletePassword } : {}),
      })
      user = { ...user, deletion_scheduled_at: result.deletion_scheduled_at ?? null }
      setSessionUser(user)
      deleteConfirmation = ''
      deletePassword = ''
      message = 'Account deletion scheduled. You can cancel during the grace period.'
    } catch (deletionError: unknown) {
      error = deletionError instanceof Error ? deletionError.message : 'Failed to schedule deletion'
    } finally {
      requestingDeletion = false
    }
  }

  async function handleCancelDeletion() {
    if (!user) return
    cancelingDeletion = true
    error = null
    message = null
    try {
      await api.users.cancelDeletion()
      user = { ...user, deletion_scheduled_at: null }
      setSessionUser(user)
      message = 'Account deletion cancelled.'
    } catch (cancelError: unknown) {
      error = cancelError instanceof Error ? cancelError.message : 'Failed to cancel deletion'
    } finally {
      cancelingDeletion = false
    }
  }

  async function handleLogout() {
    try {
      await api.auth.logout()
    } catch {
      // Always clear local state on logout.
    }
    clearSessionUser()
    navigateReplace('/login')
  }
</script>

<section class="mx-auto flex w-full max-w-2xl flex-col gap-6">
  <div>
    <h1 class="font-display text-2xl font-semibold tracking-tight">Settings</h1>
    <p class="mt-1 text-sm text-muted-foreground">Manage your dashboard account.</p>
  </div>

  {#if error}
    <Alert variant="destructive" role="alert">
      <CircleAlertIcon class="size-4" />
      <AlertDescription>{error}</AlertDescription>
    </Alert>
  {/if}

  {#if message}
    <Alert role="status" aria-live="polite">
      <CircleCheckIcon class="size-4 text-primary" />
      <AlertDescription>{message}</AlertDescription>
    </Alert>
  {/if}

  {#if loading}
    <!-- Skeleton rows: no layout shift when the real groups swap in. -->
    <div class="flex flex-col gap-6" aria-hidden="true">
      {#each [0, 1, 2] as i (i)}
        <div class="rounded-[var(--radius)] border border-border bg-card">
          <div class="border-b border-border px-5 py-3">
            <div class="h-3 w-24 animate-pulse rounded-full bg-muted"></div>
          </div>
          <div class="flex items-center justify-between gap-4 px-5 py-4">
            <div class="flex items-center gap-3">
              <div class="size-9 animate-pulse rounded-full bg-muted"></div>
              <div class="h-4 w-40 animate-pulse rounded-full bg-muted"></div>
            </div>
            <div class="h-8 w-24 animate-pulse rounded-full bg-muted"></div>
          </div>
        </div>
      {/each}
    </div>
    <p class="sr-only" role="status" aria-live="polite">Loading settings…</p>
  {:else if user}
    <!-- Account -->
    <section
      aria-labelledby="settings-account"
      class="overflow-hidden rounded-[var(--radius)] border border-border bg-card"
    >
      <h2
        id="settings-account"
        class="border-b border-border px-5 py-3 text-xs font-semibold tracking-wide text-muted-foreground uppercase"
      >
        Account
      </h2>
      <div class="flex items-center gap-3 px-5 py-4">
        <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-cloud-2 text-muted-foreground" aria-hidden="true">
          <MailIcon class="size-4" />
        </span>
        <div class="min-w-0">
          <p class="text-sm font-medium">Email</p>
          <p class="truncate text-sm text-muted-foreground">{user.email}</p>
        </div>
      </div>
    </section>

    <!-- Security -->
    <section
      aria-labelledby="settings-security"
      class="overflow-hidden rounded-[var(--radius)] border border-border bg-card"
    >
      <h2
        id="settings-security"
        class="border-b border-border px-5 py-3 text-xs font-semibold tracking-wide text-muted-foreground uppercase"
      >
        Security
      </h2>

      <div class="flex flex-col gap-4 border-b border-border px-5 py-4">
        <div class="flex items-center gap-3">
          <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-cloud-2 text-muted-foreground" aria-hidden="true">
            <KeyRoundIcon class="size-4" />
          </span>
          <p class="text-sm font-medium">{user.has_password ? 'Change password' : 'Set a password'}</p>
        </div>
        <form class="flex max-w-md flex-col gap-4" onsubmit={handlePasswordSubmit}>
          {#if user.has_password}
            <FormInput
              type="password"
              name="currentPassword"
              label="Current password"
              bind:value={currentPassword}
              error={currentPasswordError}
              required
              autocomplete="current-password"
              showPasswordToggle
            />
          {/if}

          <FormInput
            type="password"
            name="newPassword"
            label="New password"
            bind:value={newPassword}
            error={newPasswordError}
            required
            autocomplete="new-password"
            showPasswordToggle
          />

          <Button type="submit" loading={savingPassword} class="w-fit">
            {user.has_password ? 'Change password' : 'Set password'}
          </Button>
        </form>
      </div>

      <div class="flex items-center justify-between gap-4 px-5 py-4">
        <div class="flex items-center gap-3">
          <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-cloud-2 text-muted-foreground" aria-hidden="true">
            <LinkIcon class="size-4" />
          </span>
          <div>
            <p class="text-sm font-medium">Google</p>
            <p class="text-sm text-muted-foreground">{googleLinked ? 'Linked' : 'Not linked'}</p>
          </div>
        </div>
        {#if googleLinked}
          <Button type="button" variant="outline" size="sm" loading={unlinkingGoogle} onclick={handleUnlinkGoogle}>
            Unlink Google
          </Button>
        {:else}
          <Button type="button" variant="outline" size="sm" onclick={handleLinkGoogle}>
            Link Google
          </Button>
        {/if}
      </div>
    </section>

    <!-- Privacy & data -->
    <section
      aria-labelledby="settings-data"
      class="overflow-hidden rounded-[var(--radius)] border border-border bg-card"
    >
      <h2
        id="settings-data"
        class="border-b border-border px-5 py-3 text-xs font-semibold tracking-wide text-muted-foreground uppercase"
      >
        Privacy &amp; data
      </h2>
      <div class="flex items-center justify-between gap-4 border-b border-border px-5 py-4">
        <div class="flex items-center gap-3">
          <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-cloud-2 text-muted-foreground" aria-hidden="true">
            <DownloadIcon class="size-4" />
          </span>
          <div>
            <p class="text-sm font-medium">Export my data</p>
            <p class="text-sm text-muted-foreground">A machine-readable copy of your account data.</p>
          </div>
        </div>
        <Button type="button" variant="outline" size="sm" loading={exporting} onclick={handleExportData}>
          {exporting ? 'Preparing export…' : 'Export my data'}
        </Button>
      </div>

      <div class="flex flex-col gap-4 px-5 py-4">
        <div class="flex items-center gap-3">
          <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-destructive/10 text-destructive" aria-hidden="true">
            <Trash2Icon class="size-4" />
          </span>
          <p class="text-sm font-semibold text-destructive">Delete account</p>
        </div>
        {#if deletionScheduledAt}
          <p class="text-sm text-muted-foreground">
            Your account is scheduled for deletion on
            <span class="font-medium">{formatDateTime(deletionScheduledAt)}</span>. You can cancel any
            time before then.
          </p>
          <Button
            type="button"
            variant="outline"
            size="sm"
            class="w-fit"
            loading={cancelingDeletion}
            onclick={handleCancelDeletion}
          >
            {cancelingDeletion ? 'Cancelling…' : 'Cancel deletion'}
          </Button>
        {:else}
          <p class="text-sm text-muted-foreground">
            This schedules permanent deletion of your account and all associated data after a
            14-day grace period.
          </p>
          <div class="flex max-w-md flex-col gap-4">
            {#if user.has_password}
              <FormInput
                type="password"
                name="deletePassword"
                label="Confirm your password"
                bind:value={deletePassword}
                autocomplete="current-password"
                showPasswordToggle
              />
            {/if}
            <FormInput
              type="text"
              name="deleteConfirmation"
              label="Type DELETE to confirm"
              bind:value={deleteConfirmation}
            />
            <Button
              type="button"
              variant="destructive"
              class="w-fit"
              loading={requestingDeletion}
              disabled={deleteConfirmation !== 'DELETE'}
              onclick={handleRequestDeletion}
            >
              {requestingDeletion ? 'Scheduling…' : 'Delete my account'}
            </Button>
          </div>
        {/if}
      </div>
    </section>

    <!-- Session -->
    <section
      aria-labelledby="settings-session"
      class="overflow-hidden rounded-[var(--radius)] border border-border bg-card"
    >
      <h2
        id="settings-session"
        class="border-b border-border px-5 py-3 text-xs font-semibold tracking-wide text-muted-foreground uppercase"
      >
        Session
      </h2>
      <div class="flex items-center justify-between gap-4 px-5 py-4">
        <div class="flex items-center gap-3">
          <span class="flex size-9 shrink-0 items-center justify-center rounded-full bg-cloud-2 text-muted-foreground" aria-hidden="true">
            <LogOutIcon class="size-4" />
          </span>
          <p class="text-sm font-medium">Sign out of this device</p>
        </div>
        <Button type="button" variant="outline" size="sm" onclick={handleLogout}>Log out</Button>
      </div>
    </section>
  {/if}
</section>
