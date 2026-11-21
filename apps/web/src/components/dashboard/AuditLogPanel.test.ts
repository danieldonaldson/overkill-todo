import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import AuditLogPanel from './AuditLogPanel.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: { me: vi.fn() },
    admin: { listAuditEvents: vi.fn() },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
  redirect: vi.fn(),
}))

const markReady = vi.hoisted(() => vi.fn())
vi.mock('../../lib/telemetry/island-timing', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../lib/telemetry/island-timing')>()
  return {
    ...actual,
    createIslandTiming: () => Object.assign(new actual.IslandTimer(), { markReady }),
  }
})

import { api } from '../../lib/api'
import { ApiErrorException } from '../../lib/api/errors'
import { resetSession, session } from '../../lib/auth/session.svelte'
import { navigateReplace } from '../../lib/navigation'
import type { AuditEvent, AuditEventList, User, UserRole } from '../../lib/types'

const ALICE = '0190a0b0-0000-7000-8000-000000000001'
const ROOT = '0190a0b0-0000-7000-8000-000000000002'

function viewer(role: UserRole): User {
  return {
    id: 'viewer',
    email: 'viewer@example.com',
    first_name: '',
    last_name: '',
    email_verified: true,
    has_password: true,
    oauth_providers: [],
    role,
    created_at: '2026-01-01T00:00:00Z',
  }
}

const promotion: AuditEvent = {
  id: 2,
  occurred_at: '2026-11-20T10:00:00Z',
  kind: 'role_changed',
  subject: { id: ALICE, email: 'alice@example.com', deleted: false },
  actor: { kind: 'user', account: { id: ROOT, email: 'root@example.com', deleted: false } },
  details: { from_role: 'user', to_role: 'admin' },
}

const bootstrap: AuditEvent = {
  id: 1,
  occurred_at: '2026-11-19T10:00:00Z',
  kind: 'role_changed',
  subject: { id: ROOT, email: null, deleted: true },
  actor: { kind: 'operator' },
  details: { from_role: 'user', to_role: 'super_admin' },
}

function list(events: AuditEvent[], total = events.length): AuditEventList {
  return { events, total, limit: 50, offset: 0 }
}

describe('AuditLogPanel', () => {
  beforeEach(() => {
    resetSession()
    history.replaceState(null, '', '/dashboard/audit')
    vi.mocked(api.admin.listAuditEvents).mockResolvedValue(list([promotion, bootstrap]))
  })

  it('lists events newest first with kind, accounts, actor and change', async () => {
    session.set(viewer('super_admin'))
    const screen = await render(AuditLogPanel)

    await expect.element(screen.getByText('Showing 1–2 of 2')).toBeInTheDocument()
    expect(api.admin.listAuditEvents).toHaveBeenCalledWith({
      limit: 50,
      offset: 0,
      account_id: undefined,
      kind: undefined,
    })
    await expect.element(screen.getByText('alice@example.com')).toBeInTheDocument()
    const items = [...screen.container.querySelectorAll('[data-testid="audit-event"]')].map(
      (item) => item.textContent ?? ''
    )
    expect(items).toHaveLength(2)
    expect(items[0]).toMatch(/alice@example\.com/)
    expect(items[0]).toMatch(/root@example\.com/)
    expect(items[0]).toMatch(/Member → Admin/)
    expect(items[1]).toMatch(/Deleted account/)
    expect(items[1]).toMatch(/Operator/)
    expect(items[1]).toMatch(/Member → Super Admin/)
    expect(screen.container.textContent).not.toContain('null')
    await vi.waitFor(() => expect(markReady).toHaveBeenCalled())
  })

  it('filters by account and kind, and reflects the filter in the address', async () => {
    session.set(viewer('super_admin'))
    const screen = await render(AuditLogPanel)
    await expect.element(screen.getByText('Showing 1–2 of 2')).toBeInTheDocument()

    vi.mocked(api.admin.listAuditEvents).mockResolvedValue(list([promotion]))
    await screen.getByLabelText('Account ID').fill(ALICE)
    await screen.getByLabelText('Kind').selectOptions('role_changed')
    await screen.getByRole('button', { name: 'Filter' }).click()

    await expect.element(screen.getByText('Showing 1–1 of 1')).toBeInTheDocument()
    expect(api.admin.listAuditEvents).toHaveBeenLastCalledWith({
      limit: 50,
      offset: 0,
      account_id: ALICE,
      kind: 'role_changed',
    })
    expect(new URLSearchParams(window.location.search).get('account')).toBe(ALICE)
  })

  it('starts filtered by the account in the address (from the user list)', async () => {
    history.replaceState(null, '', `/dashboard/audit?account=${ALICE}`)
    const screen = await render(AuditLogPanel)

    await expect.element(screen.getByLabelText('Account ID')).toHaveValue(ALICE)
    expect(api.admin.listAuditEvents).toHaveBeenCalledWith(
      expect.objectContaining({ account_id: ALICE })
    )
  })

  it('refuses an account filter that is not an account ID, without a request', async () => {
    const screen = await render(AuditLogPanel)
    await expect.element(screen.getByText('Showing 1–2 of 2')).toBeInTheDocument()

    await screen.getByLabelText('Account ID').fill('alice@example.com')
    await screen.getByRole('button', { name: 'Filter' }).click()

    await expect.element(screen.getByText(/enter an account id/i)).toBeInTheDocument()
    expect(api.admin.listAuditEvents).toHaveBeenCalledTimes(1)
  })

  it('says there are no audit events, not an error, when none match', async () => {
    vi.mocked(api.admin.listAuditEvents).mockResolvedValue(list([]))
    const screen = await render(AuditLogPanel)

    await expect.element(screen.getByText('No audit events yet.')).toBeInTheDocument()
    expect(screen.container.querySelector('[role="alert"]')).toBeNull()
  })

  it('shows the error with a retry that reloads', async () => {
    vi.mocked(api.admin.listAuditEvents).mockRejectedValueOnce(new Error('Service unavailable'))
    const screen = await render(AuditLogPanel)

    await expect.element(screen.getByText('Service unavailable')).toBeInTheDocument()
    await screen.getByRole('button', { name: 'Retry' }).click()

    await expect.element(screen.getByText('alice@example.com')).toBeInTheDocument()
    await expect.element(screen.getByText('Service unavailable')).not.toBeInTheDocument()
  })

  it('pages through events', async () => {
    vi.mocked(api.admin.listAuditEvents).mockImplementation(async (params) =>
      params?.offset === 50
        ? { events: [bootstrap], total: 120, limit: 50, offset: 50 }
        : { events: [promotion], total: 120, limit: 50, offset: 0 }
    )
    const screen = await render(AuditLogPanel)
    await expect.element(screen.getByText('Showing 1–50 of 120')).toBeInTheDocument()

    await screen.getByRole('button', { name: /next/i }).click()

    await expect.element(screen.getByText('Showing 51–100 of 120')).toBeInTheDocument()
    expect(api.admin.listAuditEvents).toHaveBeenLastCalledWith(
      expect.objectContaining({ offset: 50 })
    )
  })

  it('sends an Admin Home when the server refuses, showing no events', async () => {
    vi.mocked(api.admin.listAuditEvents).mockRejectedValue(
      new ApiErrorException({ code: 'forbidden', message: 'Forbidden' }, { status: 403 })
    )
    const screen = await render(AuditLogPanel)

    await vi.waitFor(() => expect(navigateReplace).toHaveBeenCalledWith('/dashboard'))
    expect(screen.container.textContent).not.toContain('Forbidden')
  })

  it('sends a confirmed Admin Home, but not an unconfirmed cached one', async () => {
    session.set(viewer('admin'))
    await render(AuditLogPanel)
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(navigateReplace).not.toHaveBeenCalled()

    vi.mocked(api.auth.me).mockResolvedValue(viewer('admin'))
    await session.revalidate()
    await vi.waitFor(() => expect(navigateReplace).toHaveBeenCalledWith('/dashboard'))
  })
})
