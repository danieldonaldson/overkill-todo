import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import HomePanel from './HomePanel.svelte'

vi.mock('../../lib/api', () => ({ api: { auth: { me: vi.fn() } } }))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
  redirect: vi.fn(),
}))

// The panel's timing handle, with `markReady` observable (OT-006 Decision 6b).
const markReady = vi.hoisted(() => vi.fn())
vi.mock('../../lib/telemetry/island-timing', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../lib/telemetry/island-timing')>()
  return {
    ...actual,
    createIslandTiming: () => Object.assign(new actual.IslandTimer(), { markReady }),
  }
})

import { api } from '../../lib/api'
import { resetSession, session } from '../../lib/auth/session.svelte'
import { setTokenExpiry } from '../../lib/auth/token'
import type { User } from '../../lib/types'

const member: User = {
  id: '1',
  email: 'member@example.com',
  first_name: '',
  last_name: '',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
  created_at: '2026-01-01T00:00:00Z',
  role: 'user',
}

describe('HomePanel timing', () => {
  beforeEach(() => {
    resetSession()
    vi.clearAllMocks()
  })

  it('is ready once, at once, when the Member is already known', async () => {
    session.set(member)
    const screen = await render(HomePanel)
    await expect.element(screen.getByText('Signed in as member@example.com')).toBeInTheDocument()
    await vi.waitFor(() => expect(markReady).toHaveBeenCalledTimes(1))
  })

  it('is ready once, when the background check fills in the Member', async () => {
    const screen = await render(HomePanel)
    await expect.element(screen.getByRole('heading', { name: 'Home' })).toBeInTheDocument()
    expect(markReady).not.toHaveBeenCalled()
    session.set(member)
    await expect.element(screen.getByText('Signed in as member@example.com')).toBeInTheDocument()
    await vi.waitFor(() => expect(markReady).toHaveBeenCalledTimes(1))
    session.patch({ first_name: 'Ann' })
    await new Promise((r) => setTimeout(r, 20))
    expect(markReady).toHaveBeenCalledTimes(1)
  })

  it('is ready once, when the account check fails (the error state)', async () => {
    setTokenExpiry(Date.now() + 60_000)
    vi.mocked(api.auth.me).mockRejectedValue(new TypeError('Failed to fetch'))
    await render(HomePanel)
    await session.revalidate()
    await vi.waitFor(() => expect(session.status).toBe('unreachable'))
    await vi.waitFor(() => expect(markReady).toHaveBeenCalledTimes(1))
  })
})
