import { beforeEach, describe, expect, it, vi, type Mock } from 'vitest'
import { render } from 'vitest-browser-svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: { me: vi.fn(), logout: vi.fn() },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
  redirect: vi.fn(),
}))

import DashboardHeader from './DashboardHeader.svelte'
import { api } from '../../lib/api'
import { navigateReplace } from '../../lib/navigation'
import { resetSession } from '../../lib/auth/session.svelte'
import { setTokenExpiry } from '../../lib/auth/token'
import type { User } from '../../lib/types'

const me = api.auth.me as Mock

const user: User = {
  id: 'user-1',
  email: 'member@example.com',
  first_name: '',
  last_name: '',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
  created_at: '2026-01-01T00:00:00Z',
  role: 'user',
}

describe('DashboardHeader', () => {
  beforeEach(() => {
    setTokenExpiry(Math.floor(Date.now() / 1000) + 3600)
    resetSession()
    me.mockResolvedValue(user)
  })

  it('renders the brand link and the dashboard nav', async () => {
    const screen = await render(DashboardHeader)

    await expect
      .element(screen.getByRole('link', { name: /overkill todo/i }))
      .toHaveAttribute('href', '/dashboard')
    await expect.element(screen.getByRole('banner')).toBeInTheDocument()
    await expect.element(screen.getByRole('navigation', { name: /dashboard/i })).toBeInTheDocument()
  })

  it('starts the background account check on mount', async () => {
    await render(DashboardHeader)

    await vi.waitFor(() => expect(me).toHaveBeenCalledTimes(1))
    expect(navigateReplace).not.toHaveBeenCalled()
  })

  it('sends a tab with no session hint to /login', async () => {
    localStorage.clear()

    await render(DashboardHeader)

    await vi.waitFor(() => expect(navigateReplace).toHaveBeenCalledWith('/login'))
    expect(me).not.toHaveBeenCalled()
  })

  it('shows no notice while the server is reachable', async () => {
    const screen = await render(DashboardHeader)

    await vi.waitFor(() => expect(me).toHaveBeenCalled())
    await expect.element(screen.getByRole('banner')).toBeInTheDocument()
    expect(screen.container.textContent).not.toMatch(/couldn't reach the server/i)
  })

  it('shows a notice with Retry when the server cannot be reached, and clears it on success', async () => {
    me.mockRejectedValueOnce(new TypeError('Failed to fetch'))

    const screen = await render(DashboardHeader)

    await expect.element(screen.getByText(/couldn't reach the server/i)).toBeInTheDocument()
    // Non-blocking: the header and nav stay usable.
    await expect.element(screen.getByRole('navigation', { name: /dashboard/i })).toBeInTheDocument()

    await screen.getByRole('button', { name: /retry/i }).click()

    await vi.waitFor(() => expect(me).toHaveBeenCalledTimes(2))
    await expect.element(screen.getByText(/couldn't reach the server/i)).not.toBeInTheDocument()
  })

  it('keeps the notice when the retry also fails', async () => {
    me.mockRejectedValue(new TypeError('Failed to fetch'))

    const screen = await render(DashboardHeader)
    await expect.element(screen.getByText(/couldn't reach the server/i)).toBeInTheDocument()

    await screen.getByRole('button', { name: /retry/i }).click()

    await vi.waitFor(() => expect(me).toHaveBeenCalledTimes(2))
    await expect.element(screen.getByRole('button', { name: /retry/i })).toBeEnabled()
    await expect.element(screen.getByText(/couldn't reach the server/i)).toBeInTheDocument()
  })
})
