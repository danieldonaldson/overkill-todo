import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import ResetPasswordForm from './ResetPasswordForm.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      verifyResetCode: vi.fn(),
      resetPassword: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
}))

import { api } from '../../lib/api'
import { navigate } from '../../lib/navigation'

describe('ResetPasswordForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  const setCode = (screen: ReturnType<typeof render>, value: string) => {
    const input = screen.container.querySelector('input[aria-label="Verification code"]') as HTMLInputElement
    input.value = value
    input.dispatchEvent(new Event('input', { bubbles: true }))
  }

  const fillPassword = async (screen: ReturnType<typeof render>, password: string, confirm = password) => {
    await screen.getByRole('textbox', { name: /^new password/i }).fill(password)
    await screen.getByRole('textbox', { name: /^confirm new password/i }).fill(confirm)
  }

  it('starts with reset code verification', async () => {
    const screen = render(ResetPasswordForm, { props: { email: 'test@example.com' } })

    expect(screen.container.querySelector('input[aria-label="Verification code"]')).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /verify code/i })).toBeDisabled()
    await expect.element(screen.getByText(/test@example.com/)).toBeInTheDocument()
  })

  it('verifies the reset code before showing password fields', async () => {
    vi.mocked(api.auth.verifyResetCode).mockResolvedValue({ reset_token: 'reset-token' })
    const screen = render(ResetPasswordForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await vi.waitFor(() => {
      expect(api.auth.verifyResetCode).toHaveBeenCalledWith({ email: 'test@example.com', code: '123456' })
    })
    await expect.element(screen.getByText(/code verified/i)).toBeInTheDocument()
  })

  it('resets password with verified token and navigates to dashboard', async () => {
    vi.mocked(api.auth.verifyResetCode).mockResolvedValue({ reset_token: 'reset-token' })
    vi.mocked(api.auth.resetPassword).mockResolvedValue({
      expires_at: Math.floor(Date.now() / 1000) + 3600,
      user: {
        id: '1',
        email: 'test@example.com',
        first_name: '',
        last_name: '',
        email_verified: true,
        has_password: true,
        oauth_providers: [],
      },
    })
    const screen = render(ResetPasswordForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')
    await vi.waitFor(() => expect(api.auth.verifyResetCode).toHaveBeenCalled())
    await fillPassword(screen, 'newpassword123')
    await screen.getByRole('button', { name: /reset password/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.resetPassword).toHaveBeenCalledWith({
        reset_token: 'reset-token',
        new_password: 'newpassword123',
      })
    })
    expect(navigate).toHaveBeenCalledWith('/dashboard')
  })

  it('shows the remaining-attempts hint on an invalid reset code', async () => {
    vi.mocked(api.auth.verifyResetCode).mockRejectedValue({
      error_code: 'CODE_INVALID',
      attempts_remaining: 1,
    })
    const screen = render(ResetPasswordForm, { props: { email: 'test@example.com' } })

    setCode(screen, '000000')

    await expect.element(screen.getByText(/incorrect code\. 1 attempts remaining/i)).toBeInTheDocument()
    // Still on the code step — password fields not shown.
    await expect.element(screen.getByRole('button', { name: /verify code/i })).toBeInTheDocument()
  })

  it('prompts for a new code when the reset code has expired', async () => {
    vi.mocked(api.auth.verifyResetCode).mockRejectedValue({ error_code: 'CODE_EXPIRED' })
    const screen = render(ResetPasswordForm, { props: { email: 'test@example.com' } })

    setCode(screen, '000000')

    await expect.element(screen.getByRole('alert')).toHaveTextContent(/this code has expired/i)
  })

  it('returns to the code step when the reset token expires mid-flow', async () => {
    vi.mocked(api.auth.verifyResetCode).mockResolvedValue({ reset_token: 'reset-token' })
    vi.mocked(api.auth.resetPassword).mockRejectedValue({ error_code: 'TOKEN_EXPIRED' })
    const screen = render(ResetPasswordForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')
    await vi.waitFor(() => expect(api.auth.verifyResetCode).toHaveBeenCalled())
    await fillPassword(screen, 'newpassword123')
    await screen.getByRole('button', { name: /reset password/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent(/your session expired/i)
    // Bounced back to the code-entry step.
    await expect.element(screen.getByRole('button', { name: /verify code/i })).toBeInTheDocument()
  })
})
