import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import VerifyEmailForm from './VerifyEmailForm.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      verifyEmail: vi.fn(),
      resendVerification: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
}))

import { api } from '../../lib/api'
import { navigate } from '../../lib/navigation'

describe('VerifyEmailForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  const setCode = (screen: ReturnType<typeof render>, value: string) => {
    const input = screen.container.querySelector('input[aria-label="Verification code"]') as HTMLInputElement
    input.value = value
    input.dispatchEvent(new Event('input', { bubbles: true }))
  }

  it('renders code entry and resend controls', async () => {
    const screen = render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    expect(screen.container.querySelector('input[aria-label="Verification code"]')).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /verify email/i })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: /resend code/i })).toBeInTheDocument()
    await expect.element(screen.getByText(/test@example.com/)).toBeInTheDocument()
  })

  it('navigates existing verified users to dashboard', async () => {
    vi.mocked(api.auth.verifyEmail).mockResolvedValue({
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
    const screen = render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await vi.waitFor(() => {
      expect(api.auth.verifyEmail).toHaveBeenCalledWith({ email: 'test@example.com', code: '123456' })
      expect(navigate).toHaveBeenCalledWith('/dashboard')
    })
  })

  it('navigates new users with signup token to set-password', async () => {
    vi.mocked(api.auth.verifyEmail).mockResolvedValue({ signup_token: 'signup-token' })
    const screen = render(VerifyEmailForm, { props: { email: 'new@example.com' } })

    setCode(screen, '123456')

    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledWith('/set-password?token=signup-token&email=new%40example.com')
    })
  })

  it('resends verification with Turnstile token', async () => {
    vi.mocked(api.auth.resendVerification).mockResolvedValue({ cooldown_seconds: 300 })
    const screen = render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    await screen.getByRole('button', { name: /resend code/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.resendVerification).toHaveBeenCalledWith({
        email: 'test@example.com',
        turnstile_token: 'dev-mode-skip',
      })
    })
    await vi.waitFor(() => {
      expect(api.auth.resendVerification).toHaveBeenCalled()
    })
    await expect.element(screen.getByRole('alert')).toHaveTextContent(/new verification code has been sent/i)
  })

  it('shows the remaining-attempts hint on an invalid code', async () => {
    vi.mocked(api.auth.verifyEmail).mockRejectedValue({
      error_code: 'CODE_INVALID',
      attempts_remaining: 2,
    })
    const screen = render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await expect.element(screen.getByText(/incorrect code\. 2 attempts remaining/i)).toBeInTheDocument()
  })

  it('prompts a fresh code when the entered code has expired', async () => {
    vi.mocked(api.auth.verifyEmail).mockRejectedValue({ error_code: 'CODE_EXPIRED' })
    const screen = render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent(/this code has expired/i)
  })

  it('starts a cooldown when a resend is rate-limited', async () => {
    vi.mocked(api.auth.resendVerification).mockRejectedValue({
      error_code: 'COOLDOWN_ACTIVE',
      cooldown_seconds: 120,
    })
    const screen = render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    await screen.getByRole('button', { name: /resend code/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent(/please wait before requesting/i)
    // The cooldown swaps the resend button for the countdown timer.
    await expect.element(screen.getByText(/resend in/i)).toBeInTheDocument()
  })

  it('redirects to the dashboard when resending reports the email is already verified', async () => {
    vi.mocked(api.auth.resendVerification).mockRejectedValue({ error_code: 'ALREADY_VERIFIED' })
    const screen = render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    await screen.getByRole('button', { name: /resend code/i }).click()

    await vi.waitFor(() => expect(navigate).toHaveBeenCalledWith('/dashboard'))
  })
})
