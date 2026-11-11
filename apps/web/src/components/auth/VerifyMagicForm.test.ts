import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { render } from 'vitest-browser-svelte'
import { readSignupHandoff } from '../../lib/auth/signup-handoff'
import VerifyMagicForm from './VerifyMagicForm.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      verifyMagicLink: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
}))

import { api } from '../../lib/api'
import { navigate } from '../../lib/navigation'

describe('VerifyMagicForm', () => {
  const originalUrl = window.location.href

  beforeEach(() => {
    vi.clearAllMocks()
    sessionStorage.clear()
  })

  afterEach(() => {
    window.history.replaceState(null, '', originalUrl)
  })

  it('does nothing until the user confirms, and strips the token from the URL', async () => {
    window.history.replaceState(null, '', '/verify-magic#token=magic-token')
    const screen = await render(VerifyMagicForm)

    await expect.element(screen.getByRole('button', { name: /confirm my email/i })).toBeEnabled()
    expect(api.auth.verifyMagicLink).not.toHaveBeenCalled()
    expect(window.location.hash).toBe('')
  })

  it('hands the signup token to set-password via sessionStorage', async () => {
    vi.mocked(api.auth.verifyMagicLink).mockResolvedValue({
      signup_token: 'signup-token',
      user: {
        id: 'u1',
        email: 'new@example.com',
        email_verified: true,
      } as never,
    })
    window.history.replaceState(null, '', '/verify-magic#token=magic-token')
    const screen = await render(VerifyMagicForm)

    await screen.getByRole('button', { name: /confirm my email/i }).click()

    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith('/set-password'))
    expect(api.auth.verifyMagicLink).toHaveBeenCalledWith('magic-token')
    expect(readSignupHandoff()).toEqual({ token: 'signup-token', email: 'new@example.com' })
  })

  it('sends an already-verified account to login', async () => {
    vi.mocked(api.auth.verifyMagicLink).mockRejectedValue({
      error: 'This email is already verified. Please log in.',
      error_code: 'ALREADY_VERIFIED',
    })
    window.history.replaceState(null, '', '/verify-magic#token=magic-token')
    const screen = await render(VerifyMagicForm)

    await screen.getByRole('button', { name: /confirm my email/i }).click()

    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith('/login?message=email_verified'))
  })

  it('shows the server error for a used or expired link', async () => {
    vi.mocked(api.auth.verifyMagicLink).mockRejectedValue({
      error: 'This link has expired. Request a new code.',
      error_code: 'CODE_EXPIRED',
    })
    window.history.replaceState(null, '', '/verify-magic#token=magic-token')
    const screen = await render(VerifyMagicForm)

    await screen.getByRole('button', { name: /confirm my email/i }).click()

    await expect.element(screen.getByText(/link has expired/i)).toBeInTheDocument()
  })

  it('explains a link without a token', async () => {
    window.history.replaceState(null, '', '/verify-magic')
    const screen = await render(VerifyMagicForm)

    await expect.element(screen.getByText(/link is incomplete/i)).toBeInTheDocument()
  })
})
