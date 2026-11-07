import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import GuestGuardWrapper from './GuestGuardWrapper.test.svelte'
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

/** Mimics the localStorage trace a completed login leaves behind. */
function seedTokenExpiry(secondsFromNow: number): void {
  localStorage.setItem('jwt_expiry', String(Math.floor(Date.now() / 1000) + secondsFromNow))
}

describe('GuestGuard', () => {
  beforeEach(() => {
    sessionStorage.clear()
    localStorage.clear()
    vi.clearAllMocks()
  })

  afterEach(() => {
    sessionStorage.clear()
    localStorage.clear()
  })

  it('renders the auth form without calling /auth/me when there is no session hint', async () => {
    const screen = render(GuestGuardWrapper)

    await expect.element(screen.getByTestId('guest-content')).toBeInTheDocument()
    expect(api.auth.me).not.toHaveBeenCalled()
    expect(navigateReplace).not.toHaveBeenCalled()
  })

  it('redirects a signed-in visitor to the dashboard', async () => {
    seedTokenExpiry(600)
    vi.mocked(api.auth.me).mockResolvedValue(verifiedUser)

    render(GuestGuardWrapper)

    await vi.waitFor(() => {
      expect(navigateReplace).toHaveBeenCalledWith('/dashboard')
    })
  })

  it('redirects when only the cached session user is present', async () => {
    sessionStorage.setItem('app-auth-user', JSON.stringify(verifiedUser))
    vi.mocked(api.auth.me).mockResolvedValue(verifiedUser)

    render(GuestGuardWrapper)

    await vi.waitFor(() => {
      expect(navigateReplace).toHaveBeenCalledWith('/dashboard')
    })
  })

  it('revalidates an expired access token rather than trusting the stale hint', async () => {
    // The access token expiry is in the past but the refresh cookie may still be
    // good — this is the returning-user case the guard exists for.
    seedTokenExpiry(-3600)
    vi.mocked(api.auth.me).mockResolvedValue(verifiedUser)

    render(GuestGuardWrapper)

    await vi.waitFor(() => {
      expect(api.auth.me).toHaveBeenCalled()
      expect(navigateReplace).toHaveBeenCalledWith('/dashboard')
    })
  })

  it('sends a signed-in but unverified visitor to verify-email', async () => {
    seedTokenExpiry(600)
    vi.mocked(api.auth.me).mockResolvedValue({ ...verifiedUser, email_verified: false })

    render(GuestGuardWrapper)

    await vi.waitFor(() => {
      expect(navigateReplace).toHaveBeenCalledWith('/verify-email?email=test%40example.com')
    })
  })

  it('falls back to the auth form when the hinted session is dead', async () => {
    seedTokenExpiry(600)
    vi.mocked(api.auth.me).mockRejectedValue(new Error('Unauthorized'))

    const screen = render(GuestGuardWrapper)

    await expect.element(screen.getByTestId('guest-content')).toBeInTheDocument()
    expect(navigateReplace).not.toHaveBeenCalled()
    expect(sessionStorage.getItem('app-auth-user')).toBeNull()
  })

  it('keeps the auth form hidden while the session check is in flight', async () => {
    seedTokenExpiry(600)
    let resolveMe: (user: User) => void = () => {}
    vi.mocked(api.auth.me).mockReturnValue(
      new Promise<User>((resolve) => {
        resolveMe = resolve
      }),
    )

    const screen = render(GuestGuardWrapper)

    await expect.element(screen.getByText('Loading...')).toBeInTheDocument()
    expect(screen.getByTestId('guest-content').elements()).toHaveLength(0)

    resolveMe(verifiedUser)
  })
})
