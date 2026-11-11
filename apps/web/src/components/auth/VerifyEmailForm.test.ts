import { beforeEach, describe, expect, it, vi } from 'vitest'

import { render } from 'vitest-browser-svelte'
import { readSignupHandoff } from '../../lib/auth/signup-handoff'
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

  const setCode = (screen: Awaited<ReturnType<typeof render>>, value: string) => {
    const input = screen.container.querySelector(
      'input[aria-label="Verification code"]'
    ) as HTMLInputElement
    input.value = value
    input.dispatchEvent(new Event('input', { bubbles: true }))
  }

  it('renders code entry and resend controls', async () => {
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    expect(
      screen.container.querySelector('input[aria-label="Verification code"]')
    ).toBeInTheDocument()
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
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await vi.waitFor(() => {
      expect(api.auth.verifyEmail).toHaveBeenCalledWith({
        email: 'test@example.com',
        code: '123456',
      })
      expect(navigate).toHaveBeenCalledWith('/dashboard')
    })
  })

  it('blocks resend while a verification is in flight', async () => {
    let finishVerify: (value: { signup_token: string }) => void = () => {}
    vi.mocked(api.auth.verifyEmail).mockReturnValue(
      new Promise((resolve) => {
        finishVerify = resolve
      })
    )
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await expect.element(screen.getByRole('button', { name: /verifying/i })).toBeDisabled()
    await expect.element(screen.getByRole('button', { name: /resend code/i })).toBeDisabled()

    finishVerify({ signup_token: 'signup-token' })
    await expect.element(screen.getByRole('button', { name: /resend code/i })).toBeEnabled()
    expect(api.auth.resendVerification).not.toHaveBeenCalled()
  })

  it('navigates new users with signup token to set-password', async () => {
    vi.mocked(api.auth.verifyEmail).mockResolvedValue({ signup_token: 'signup-token' })
    const screen = await render(VerifyEmailForm, { props: { email: 'new@example.com' } })

    setCode(screen, '123456')

    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledWith('/set-password')
    })
    // The token travels via sessionStorage, never the URL.
    expect(readSignupHandoff()).toEqual({ token: 'signup-token', email: 'new@example.com' })
    sessionStorage.clear()
  })

  it('resends verification with Turnstile token', async () => {
    vi.mocked(api.auth.resendVerification).mockResolvedValue({ cooldown_seconds: 300 })
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

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
    await expect.element(screen.getByRole('alert')).toMatchTextContent(/a new code is on its way/i)
  })

  it('shows the remaining-attempts hint on an invalid code', async () => {
    vi.mocked(api.auth.verifyEmail).mockRejectedValue({
      error_code: 'CODE_INVALID',
      attempts_remaining: 2,
    })
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await expect
      .element(screen.getByText(/incorrect code\. 2 attempts remaining/i))
      .toBeInTheDocument()
  })

  it('tells already-verified users to log in instead of signing them in', async () => {
    // The backend answers an already-verified email with an error and no
    // credentials; it no longer checks the code (or hands out a session) here.
    vi.mocked(api.auth.verifyEmail).mockRejectedValue({
      error: 'This email is already verified. Please log in.',
      error_code: 'ALREADY_VERIFIED',
    })
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent('This email is already verified. Please log in.')
    expect(navigate).not.toHaveBeenCalled()
  })

  it('prompts a fresh code when the entered code has expired', async () => {
    vi.mocked(api.auth.verifyEmail).mockRejectedValue({ error_code: 'CODE_EXPIRED' })
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    setCode(screen, '123456')

    await expect.element(screen.getByRole('alert')).toMatchTextContent(/this code has expired/i)
  })

  it('starts the cooldown timer from a resend response', async () => {
    vi.mocked(api.auth.resendVerification).mockResolvedValue({ cooldown_seconds: 300 })
    const screen = await render(VerifyEmailForm, { props: { email: 'test@example.com' } })

    await screen.getByRole('button', { name: /resend code/i }).click()

    // The cooldown swaps the resend button for the countdown timer.
    await expect.element(screen.getByText(/resend in/i)).toBeInTheDocument()
  })
})
