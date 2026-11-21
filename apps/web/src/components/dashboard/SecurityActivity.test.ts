import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import SecurityActivity from './SecurityActivity.svelte'

vi.mock('../../lib/api', () => ({
  api: { users: { securityActivity: vi.fn() } },
}))

import { api } from '../../lib/api'
import type { SecurityActivityEvent, SecurityActivityPage } from '../../lib/types'

function event(id: number, overrides: Partial<SecurityActivityEvent> = {}): SecurityActivityEvent {
  return {
    id,
    occurred_at: '2026-11-21T10:00:00Z',
    kind: 'password_changed',
    actor: 'you',
    details: {},
    ...overrides,
  }
}

function texts(screen: Awaited<ReturnType<typeof render>>): string[] {
  return [...screen.container.querySelectorAll('[data-testid="activity-event"]')].map(
    (item) => item.textContent?.replace(/\s+/g, ' ').trim() ?? ''
  )
}

describe('SecurityActivity', () => {
  beforeEach(() => {
    vi.mocked(api.users.securityActivity).mockResolvedValue({ events: [] })
  })

  it("lists the Member's events with who acted, never naming staff", async () => {
    vi.mocked(api.users.securityActivity).mockResolvedValue({
      events: [
        event(3),
        event(2, {
          kind: 'role_changed',
          actor: 'administrator',
          details: { from_role: 'user', to_role: 'admin' },
        }),
        event(1, { kind: 'sessions_revoked_after_reuse', actor: 'system' }),
      ],
    })
    const screen = await render(SecurityActivity)

    await expect.element(screen.getByText('Password changed')).toBeInTheDocument()
    const lines = texts(screen)
    expect(lines[0]).toMatch(/^Password changed By you ·/)
    expect(lines[1]).toMatch(/^Role changed from Member to Admin By an administrator ·/)
    expect(lines[2]).toMatch(/^All sessions signed out Automatically ·/)
    expect(api.users.securityActivity).toHaveBeenCalledWith({ limit: 20, before_id: undefined })
  })

  it('says there is no activity yet for a new account', async () => {
    const screen = await render(SecurityActivity)
    await expect.element(screen.getByText('No activity yet.')).toBeInTheDocument()
    expect(screen.container.querySelector('[role="alert"]')).toBeNull()
  })

  it('shows an error with a retry that reloads', async () => {
    vi.mocked(api.users.securityActivity).mockRejectedValueOnce(new Error('Service unavailable'))
    const screen = await render(SecurityActivity)

    await expect.element(screen.getByText(/Service unavailable/)).toBeInTheDocument()
    vi.mocked(api.users.securityActivity).mockResolvedValue({ events: [event(1)] })
    await screen.getByRole('button', { name: 'Retry' }).click()

    await expect.element(screen.getByText('Password changed')).toBeInTheDocument()
    expect(screen.container.querySelector('[role="alert"]')).toBeNull()
  })

  it('loads more from where the page ended', async () => {
    const first: SecurityActivityPage = {
      events: [event(5), event(4)],
      next_before_id: 4,
    }
    vi.mocked(api.users.securityActivity).mockImplementation(async (params) =>
      params?.before_id === 4 ? { events: [event(3, { kind: 'data_exported' })] } : first
    )
    const screen = await render(SecurityActivity)
    await expect.element(screen.getByRole('button', { name: 'Load more' })).toBeVisible()

    await screen.getByRole('button', { name: 'Load more' }).click()

    await expect.element(screen.getByText('Data exported')).toBeInTheDocument()
    expect(texts(screen)).toHaveLength(3)
    expect(api.users.securityActivity).toHaveBeenLastCalledWith({ limit: 20, before_id: 4 })
    await expect.element(screen.getByRole('button', { name: 'Load more' })).not.toBeInTheDocument()
  })
})
