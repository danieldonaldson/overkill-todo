import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import AuthGuardWrapper from './AuthGuardWrapper.test.svelte'
import type { User } from '../../lib/types'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      me: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
}))

import { api } from '../../lib/api'
import { getCurrentUser, setCurrentUser } from '../../lib/auth/current-user'
import { clearTokenExpiry, setTokenExpiry } from '../../lib/auth/token'
import { navigateReplace } from '../../lib/navigation'

const verifiedUser: User = {
  id: 'user-123',
  email: 'test@example.com',
  first_name: 'Test',
  last_name: 'User',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
}

describe('AuthGuard', () => {
  beforeEach(() => {
    sessionStorage.clear()
    clearTokenExpiry()
    vi.clearAllMocks()
  })

  afterEach(() => {
    sessionStorage.clear()
  })

  it('renders children and calls callback when authenticated', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(verifiedUser)
    const onAuthenticated = vi.fn()

    const screen = await render(AuthGuardWrapper, {
      props: { childText: 'Secret content', onAuthenticated },
    })

    await expect.element(screen.getByText('Secret content')).toBeInTheDocument()
    expect(onAuthenticated).toHaveBeenCalledWith(verifiedUser)
  })

  it('publishes the fetched user for the panels it renders, with one /auth/me call', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(verifiedUser)

    const screen = await render(AuthGuardWrapper)

    await expect.element(screen.getByTestId('protected-content')).toBeInTheDocument()
    expect(getCurrentUser()).toEqual(verifiedUser)
    expect(api.auth.me).toHaveBeenCalledTimes(1)
  })

  it('clears the published user when /auth/me fails', async () => {
    setCurrentUser(verifiedUser)
    vi.mocked(api.auth.me).mockRejectedValue(new Error('Unauthorized'))

    await render(AuthGuardWrapper)

    await vi.waitFor(() => expect(navigateReplace).toHaveBeenCalledWith('/login'))
    expect(getCurrentUser()).toBeNull()
  })

  it('trusts a cached verified user while the tab has a session, without calling /auth/me', async () => {
    sessionStorage.setItem('overkill-todo-auth-user', JSON.stringify(verifiedUser))
    setTokenExpiry(Math.floor(Date.now() / 1000) + 900)
    const onAuthenticated = vi.fn()

    const screen = await render(AuthGuardWrapper, {
      props: { childText: 'Secret content', onAuthenticated },
    })

    await expect.element(screen.getByText('Secret content')).toBeInTheDocument()
    expect(api.auth.me).not.toHaveBeenCalled()
    expect(onAuthenticated).toHaveBeenCalledWith(verifiedUser)
    expect(getCurrentUser()).toEqual(verifiedUser)
  })

  it('still fetches /auth/me for a cached user who is not verified yet', async () => {
    const unverified = { ...verifiedUser, email_verified: false }
    sessionStorage.setItem('overkill-todo-auth-user', JSON.stringify(unverified))
    setTokenExpiry(Math.floor(Date.now() / 1000) + 900)
    vi.mocked(api.auth.me).mockResolvedValue(verifiedUser)

    const screen = await render(AuthGuardWrapper, { props: { childText: 'Secret content' } })

    await expect.element(screen.getByText('Secret content')).toBeInTheDocument()
    expect(api.auth.me).toHaveBeenCalledTimes(1)
  })

  it('revalidates a cached user when the tab has no session hint', async () => {
    sessionStorage.setItem('overkill-todo-auth-user', JSON.stringify(verifiedUser))
    vi.mocked(api.auth.me).mockRejectedValue(new Error('Unauthorized'))

    await render(AuthGuardWrapper)

    await vi.waitFor(() => {
      expect(api.auth.me).toHaveBeenCalled()
      expect(navigateReplace).toHaveBeenCalledWith('/login')
    })
  })

  it('redirects unauthenticated users to login', async () => {
    vi.mocked(api.auth.me).mockRejectedValue(new Error('Unauthorized'))

    await render(AuthGuardWrapper)

    await vi.waitFor(() => {
      expect(navigateReplace).toHaveBeenCalledWith('/login')
    })
  })

  it('redirects unverified users to verify-email unless skipped', async () => {
    vi.mocked(api.auth.me).mockResolvedValue({
      ...verifiedUser,
      email_verified: false,
    })

    await render(AuthGuardWrapper)

    await vi.waitFor(() => {
      expect(navigateReplace).toHaveBeenCalledWith('/verify-email?email=test%40example.com')
    })
  })

  it('allows unverified users when verification check is skipped', async () => {
    vi.mocked(api.auth.me).mockResolvedValue({
      ...verifiedUser,
      email_verified: false,
    })

    const screen = await render(AuthGuardWrapper, { props: { skipVerificationCheck: true } })

    await expect.element(screen.getByTestId('protected-content')).toBeInTheDocument()
    expect(navigateReplace).not.toHaveBeenCalled()
  })
})
