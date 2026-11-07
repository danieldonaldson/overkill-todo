import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import ForgotPasswordForm from './ForgotPasswordForm.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      forgotPassword: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
}))

import { api } from '../../lib/api'
import { navigate } from '../../lib/navigation'

describe('ForgotPasswordForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders the reset request form', async () => {
    const screen = render(ForgotPasswordForm)

    await expect.element(screen.getByLabelText(/email/i)).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /send reset code/i })).toBeInTheDocument()
    await expect.element(screen.getByRole('link', { name: /back to sign in/i })).toHaveAttribute('href', '/login')
  })

  it('validates email before submitting', async () => {
    const screen = render(ForgotPasswordForm)

    await screen.getByLabelText(/email/i).fill('invalid')
    await screen.getByRole('button', { name: /send reset code/i }).click()

    await expect.element(screen.getByText('Please enter a valid email address')).toBeInTheDocument()
    expect(api.auth.forgotPassword).not.toHaveBeenCalled()
  })

  it('requests a reset code with Turnstile token and navigates to reset-password', async () => {
    vi.mocked(api.auth.forgotPassword).mockResolvedValue(undefined)
    const screen = render(ForgotPasswordForm)

    await screen.getByLabelText(/email/i).fill('test@example.com')
    await screen.getByRole('button', { name: /send reset code/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.forgotPassword).toHaveBeenCalledWith({
        email: 'test@example.com',
        turnstile_token: 'dev-mode-skip',
      })
    })
    expect(navigate).toHaveBeenCalledWith('/reset-password?email=test%40example.com')
  })

  it('surfaces an error and stays put when the request fails', async () => {
    vi.mocked(api.auth.forgotPassword).mockRejectedValue(new Error('Email service down'))
    const screen = render(ForgotPasswordForm)

    await screen.getByLabelText(/email/i).fill('test@example.com')
    await screen.getByRole('button', { name: /send reset code/i }).click()

    await expect.element(screen.getByText('Email service down')).toBeInTheDocument()
    expect(navigate).not.toHaveBeenCalled()
  })
})
