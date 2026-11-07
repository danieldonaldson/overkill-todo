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
    vi.clearAllMocks()
  })

  afterEach(() => {
    sessionStorage.clear()
  })

  it('renders children and calls callback when authenticated', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(verifiedUser)
    const onAuthenticated = vi.fn()

    const screen = render(AuthGuardWrapper, {
      props: { childText: 'Secret content', onAuthenticated },
    })

    await expect.element(screen.getByText('Secret content')).toBeInTheDocument()
    expect(onAuthenticated).toHaveBeenCalledWith(verifiedUser)
  })

  it('revalidates cached users before rendering protected content', async () => {
    sessionStorage.setItem('app-auth-user', JSON.stringify(verifiedUser))
    vi.mocked(api.auth.me).mockRejectedValue(new Error('Unauthorized'))

    render(AuthGuardWrapper)

    await vi.waitFor(() => {
      expect(api.auth.me).toHaveBeenCalled()
      expect(navigateReplace).toHaveBeenCalledWith('/login')
    })
  })

  it('redirects unauthenticated users to login', async () => {
    vi.mocked(api.auth.me).mockRejectedValue(new Error('Unauthorized'))

    render(AuthGuardWrapper)

    await vi.waitFor(() => {
      expect(navigateReplace).toHaveBeenCalledWith('/login')
    })
  })

  it('redirects unverified users to verify-email unless skipped', async () => {
    vi.mocked(api.auth.me).mockResolvedValue({
      ...verifiedUser,
      email_verified: false,
    })

    render(AuthGuardWrapper)

    await vi.waitFor(() => {
      expect(navigateReplace).toHaveBeenCalledWith('/verify-email?email=test%40example.com')
    })
  })

  it('allows unverified users when verification check is skipped', async () => {
    vi.mocked(api.auth.me).mockResolvedValue({
      ...verifiedUser,
      email_verified: false,
    })

    const screen = render(AuthGuardWrapper, { props: { skipVerificationCheck: true } })

    await expect.element(screen.getByTestId('protected-content')).toBeInTheDocument()
    expect(navigateReplace).not.toHaveBeenCalled()
  })
})
