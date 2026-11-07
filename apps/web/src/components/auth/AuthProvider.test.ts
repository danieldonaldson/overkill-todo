import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import AuthProviderWrapper from './AuthProviderWrapper.test.svelte'
import type { User } from '../../lib/types'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      logout: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
}))

import { api } from '../../lib/api'
import { navigateReplace } from '../../lib/navigation'

const mockUser: User = {
  id: 'user-123',
  email: 'test@example.com',
  first_name: 'Test',
  last_name: 'User',
  email_verified: true,
  has_password: true,
  oauth_providers: [],
}

describe('AuthProvider', () => {
  beforeEach(() => {
    sessionStorage.clear()
    vi.clearAllMocks()
  })

  afterEach(() => {
    sessionStorage.clear()
  })

  it('provides unauthenticated context without a user', async () => {
    const screen = render(AuthProviderWrapper)

    await expect.element(screen.getByTestId('context-consumer')).toBeInTheDocument()
    await expect.element(screen.getByTestId('is-authenticated')).toHaveTextContent('false')
    await expect.element(screen.getByTestId('user-email')).toHaveTextContent('none')
    await expect.element(screen.getByTestId('is-loading')).toHaveTextContent('false')
  })

  it('provides authenticated context from initial user', async () => {
    const screen = render(AuthProviderWrapper, { props: { initialUser: mockUser } })

    await expect.element(screen.getByTestId('is-authenticated')).toHaveTextContent('true')
    await expect.element(screen.getByTestId('user-email')).toHaveTextContent('test@example.com')
  })

  it('logs out and replaces navigation with login', async () => {
    vi.mocked(api.auth.logout).mockResolvedValue(undefined)
    const screen = render(AuthProviderWrapper, { props: { initialUser: mockUser } })

    await screen.getByTestId('logout-btn').click()

    await vi.waitFor(() => {
      expect(api.auth.logout).toHaveBeenCalled()
      expect(navigateReplace).toHaveBeenCalledWith('/login')
    })
  })
})
