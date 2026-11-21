<script lang="ts">
  import TelemetryBoundary from '$lib/telemetry/TelemetryBoundary.svelte'
  import { createIslandTiming } from '$lib/telemetry/island-timing'
  import { onMount, tick } from 'svelte'
  import CircleAlertIcon from '@lucide/svelte/icons/circle-alert'
  import { Alert, AlertDescription } from '$lib/components/ui/alert'
  import { Button } from '$lib/components/ui/button'
  import {
    Card,
    CardContent,
    CardDescription,
    CardHeader,
    CardTitle,
  } from '$lib/components/ui/card'
  import { Input } from '$lib/components/ui/input'
  import { Label } from '$lib/components/ui/label'
  import * as Pagination from '$lib/components/ui/pagination'
  import { api } from '../../lib/api'
  import { isApiStatus } from '../../lib/api/errors'
  import {
    AUDIT_KIND_FILTERS,
    AUDIT_KIND_LABELS,
    auditAccountLabel,
    auditActorLabel,
    auditDetails,
  } from '../../lib/audit'
  import { session } from '../../lib/auth/session.svelte'
  import { formatDateTime } from '../../lib/format'
  import { navigateReplace } from '../../lib/navigation'
  import { isSuperAdminRole } from '../../lib/types'
  import type { AuditEvent, AuditKindFilter } from '../../lib/types'

  const PAGE_SIZE = 50
  const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i

  // Ready once the first page, the empty state or the error is shown.
  const timing = createIslandTiming()

  let loading = $state(true)
  let error = $state<string | null>(null)
  let events = $state<AuditEvent[]>([])

  // Filters: what's typed, and what the shown page was loaded with.
  let accountInput = $state('')
  let kindInput = $state<AuditKindFilter | ''>('')
  let accountError = $state<string | null>(null)
  let applied = $state<{ account: string; kind: AuditKindFilter | '' }>({ account: '', kind: '' })
  const filtered = $derived(applied.account !== '' || applied.kind !== '')

  // Pagination (1-based page; the API takes an offset).
  let page = $state(1)
  let total = $state(0)
  let pageLoading = $state(false)
  // Only the latest request may apply its result (as in AdminPanel).
  let latestRequest = 0
  const firstShown = $derived(total === 0 ? 0 : (page - 1) * PAGE_SIZE + 1)
  const lastShown = $derived(Math.min(page * PAGE_SIZE, total))

  // Only a confirmed role sends someone away: a stale or missing cache is not
  // enough (dashboard rules). The server refuses everyone else regardless.
  const confirmedNotSuperAdmin = $derived(
    session.status === 'confirmed' && !isSuperAdminRole(session.user?.role)
  )
  let leaving = $state(false)

  function leave() {
    if (leaving) return
    leaving = true
    navigateReplace('/dashboard')
  }

  $effect(() => {
    if (confirmedNotSuperAdmin) leave()
  })

  async function loadPage(target: number) {
    const request = ++latestRequest
    pageLoading = true
    error = null
    try {
      const list = await api.admin.listAuditEvents({
        limit: PAGE_SIZE,
        offset: (target - 1) * PAGE_SIZE,
        account_id: applied.account || undefined,
        kind: applied.kind || undefined,
      })
      if (request !== latestRequest) return
      events = list.events
      total = list.total
      page = target
    } catch (loadError: unknown) {
      if (request !== latestRequest) return
      if (isApiStatus(loadError, 403)) {
        leave()
        return
      }
      error = loadError instanceof Error ? loadError.message : 'Failed to load the audit log'
    } finally {
      if (request === latestRequest) {
        if (loading && !leaving) void tick().then(timing.markReady)
        loading = false
        pageLoading = false
      }
    }
  }

  /** Mirror the applied filters in the address, so a filtered view can be shared or reloaded. */
  function syncUrl() {
    const url = new URL(window.location.href)
    for (const [key, value] of [
      ['account', applied.account],
      ['kind', applied.kind],
    ]) {
      if (value) url.searchParams.set(key, value)
      else url.searchParams.delete(key)
    }
    history.replaceState(history.state, '', url)
  }

  function applyFilters(event?: SubmitEvent) {
    event?.preventDefault()
    const account = accountInput.trim()
    if (account !== '' && !UUID.test(account)) {
      accountError = 'Enter an account ID (from the Admin user list).'
      return
    }
    accountError = null
    applied = { account, kind: kindInput }
    syncUrl()
    void loadPage(1)
  }

  function clearFilters() {
    accountInput = ''
    kindInput = ''
    applyFilters()
  }

  // Load at once without checking the role first: the server decides, and a
  // 403 leaves before any event could be shown. A link from the user list
  // arrives with `?account=<id>`.
  onMount(() => {
    const params = new URLSearchParams(window.location.search)
    const account = params.get('account') ?? ''
    const kind = params.get('kind') ?? ''
    accountInput = UUID.test(account) ? account : ''
    kindInput = kind in AUDIT_KIND_FILTERS ? (kind as AuditKindFilter) : ''
    applied = { account: accountInput, kind: kindInput }
    void loadPage(1)
  })
</script>

<TelemetryBoundary name="AuditLogPanel" {timing}>
  <div class="space-y-6">
    <div>
      <h1 class="text-2xl font-semibold">Audit log</h1>
      <p class="text-sm text-muted-foreground">Who changed what on which account, newest first.</p>
    </div>

    {#if leaving}
      <!-- On the way Home: never show events to anyone but a Super Admin. -->
    {:else}
      <form
        class="grid gap-3 sm:grid-cols-[1fr_auto_auto] sm:items-end"
        onsubmit={applyFilters}
        aria-label="Filter audit events"
      >
        <div class="grid gap-1.5">
          <Label for="audit-account">Account ID</Label>
          <Input
            id="audit-account"
            bind:value={accountInput}
            placeholder="Affected or acting account"
            autocomplete="off"
            aria-invalid={accountError ? true : undefined}
            aria-describedby={accountError ? 'audit-account-error' : undefined}
          />
        </div>
        <div class="grid gap-1.5">
          <Label for="audit-kind">Kind</Label>
          <select
            id="audit-kind"
            class="h-9 rounded-lg border border-input bg-background px-2 text-sm"
            bind:value={kindInput}
          >
            <option value="">All kinds</option>
            {#each Object.entries(AUDIT_KIND_FILTERS) as [value, label] (value)}
              <option {value}>{label}</option>
            {/each}
          </select>
        </div>
        <div class="flex gap-2">
          <Button type="submit">Filter</Button>
          <Button type="button" variant="outline" onclick={clearFilters}>Clear</Button>
        </div>
        {#if accountError}
          <p id="audit-account-error" class="text-sm text-destructive sm:col-span-3">
            {accountError}
          </p>
        {/if}
      </form>

      {#if error}
        <Alert variant="destructive">
          <CircleAlertIcon class="size-4" />
          <AlertDescription class="flex flex-wrap items-center gap-3">
            <span>{error}</span>
            <Button size="sm" variant="outline" onclick={() => loadPage(page)}>Retry</Button>
          </AlertDescription>
        </Alert>
      {/if}

      {#if loading}
        <Card aria-hidden="true" data-testid="audit-skeleton">
          <CardContent>
            {#each Array.from({ length: 5 }, (_, i) => i) as row (row)}
              <div class="space-y-2 border-b border-border/60 py-3">
                <div class="h-3.5 w-40 animate-pulse rounded bg-muted"></div>
                <div class="h-3.5 w-64 max-w-full animate-pulse rounded bg-muted"></div>
              </div>
            {/each}
          </CardContent>
        </Card>
      {:else if !error}
        <Card>
          <CardHeader>
            <CardTitle>Events</CardTitle>
            <CardDescription>Kept for 7 years. Nobody can change or delete them.</CardDescription>
          </CardHeader>
          <CardContent>
            <ol
              class="transition-opacity"
              class:opacity-60={pageLoading}
              aria-busy={pageLoading}
              data-testid="audit-events"
            >
              {#each events as event (event.id)}
                {@const details = auditDetails(event)}
                <li
                  class="grid gap-1 border-b border-border/60 py-3 text-sm last:border-b-0"
                  data-testid="audit-event"
                >
                  <div class="flex flex-wrap items-baseline justify-between gap-x-4">
                    <span class="font-medium">{AUDIT_KIND_LABELS[event.kind]}</span>
                    <time class="text-muted-foreground" datetime={event.occurred_at}>
                      {formatDateTime(event.occurred_at)}
                    </time>
                  </div>
                  <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">
                    <dt class="text-muted-foreground">Account</dt>
                    <dd class="break-all">{auditAccountLabel(event.subject)}</dd>
                    <dt class="text-muted-foreground">By</dt>
                    <dd class="break-all">{auditActorLabel(event.actor)}</dd>
                    {#if details}
                      <dt class="text-muted-foreground">Change</dt>
                      <dd>{details}</dd>
                    {/if}
                  </dl>
                </li>
              {/each}
            </ol>
            {#if events.length === 0}
              <p class="py-4 text-sm text-muted-foreground">
                {filtered ? 'No audit events match these filters.' : 'No audit events yet.'}
              </p>
            {/if}
            {#if total > 0}
              <div class="mt-4 flex flex-col items-center gap-3 sm:flex-row sm:justify-between">
                <p class="text-sm text-muted-foreground" data-testid="audit-range">
                  Showing {firstShown}–{lastShown} of {total}
                </p>
                {#if total > PAGE_SIZE}
                  <Pagination.Root
                    count={total}
                    perPage={PAGE_SIZE}
                    bind:page
                    onPageChange={loadPage}
                    class="mx-0 w-auto"
                  >
                    {#snippet children({ pages, currentPage })}
                      <Pagination.Content>
                        <Pagination.Item>
                          <Pagination.PrevButton />
                        </Pagination.Item>
                        {#each pages as item (item.key)}
                          <Pagination.Item>
                            {#if item.type === 'ellipsis'}
                              <Pagination.Ellipsis />
                            {:else}
                              <Pagination.Link page={item} isActive={currentPage === item.value} />
                            {/if}
                          </Pagination.Item>
                        {/each}
                        <Pagination.Item>
                          <Pagination.NextButton />
                        </Pagination.Item>
                      </Pagination.Content>
                    {/snippet}
                  </Pagination.Root>
                {/if}
              </div>
            {/if}
          </CardContent>
        </Card>
      {/if}
    {/if}
  </div>
</TelemetryBoundary>
