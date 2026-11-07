import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import SetPasswordForm from './SetPasswordForm.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      completeSignup: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
}))

import { api } from '../../lib/api'
import { navigate } from '../../lib/navigation'

describe('SetPasswordForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders account completion fields', async () => {
    const screen = render(SetPasswordForm, {
      props: { token: 'signup-token', email: 'test@example.com' },
    })

    await expect.element(screen.getByRole('textbox', { name: /email/i })).toHaveValue('test@example.com')
    await expect.element(screen.getByRole('textbox', { name: /^password/i })).toBeInTheDocument()
    await expect.element(screen.getByRole('textbox', { name: /^confirm password/i })).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /create account/i })).toBeInTheDocument()
  })

  it('validates matching passwords before submitting', async () => {
    const screen = render(SetPasswordForm, {
      props: { token: 'signup-token', email: 'test@example.com' },
    })

    await screen.getByRole('textbox', { name: /^password/i }).fill('short')
    await screen.getByRole('textbox', { name: /^confirm password/i }).fill('different')
    await screen.getByRole('button', { name: /create account/i }).click()

    await expect.element(screen.getByText(/password must be at least 8 characters/i)).toBeInTheDocument()
    expect(api.auth.completeSignup).not.toHaveBeenCalled()
  })

  it('completes signup and navigates to dashboard', async () => {
    vi.mocked(api.auth.completeSignup).mockResolvedValue({
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
    const screen = render(SetPasswordForm, {
      props: { token: 'signup-token', email: 'test@example.com' },
    })

    await screen.getByRole('textbox', { name: /^password/i }).fill('password123')
    await screen.getByRole('textbox', { name: /^confirm password/i }).fill('password123')
    await screen.getByRole('button', { name: /create account/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.completeSignup).toHaveBeenCalledWith({
        signup_token: 'signup-token',
        email: 'test@example.com',
        password: 'password123',
      })
    })
    expect(navigate).toHaveBeenCalledWith('/dashboard')
  })

  it('explains an expired signup token and does not navigate', async () => {
    vi.mocked(api.auth.completeSignup).mockRejectedValue({ error_code: 'TOKEN_EXPIRED' })
    const screen = render(SetPasswordForm, {
      props: { token: 'signup-token', email: 'test@example.com' },
    })

    await screen.getByRole('textbox', { name: /^password/i }).fill('password123')
    await screen.getByRole('textbox', { name: /^confirm password/i }).fill('password123')
    await screen.getByRole('button', { name: /create account/i }).click()

    await expect
      .element(screen.getByText(/your signup session has expired/i))
      .toBeInTheDocument()
    expect(navigate).not.toHaveBeenCalled()
  })

  it('blocks submission when the signup token is missing', async () => {
    const screen = render(SetPasswordForm, { props: { token: '', email: 'test@example.com' } })

    await screen.getByRole('textbox', { name: /^password/i }).fill('password123')
    await screen.getByRole('textbox', { name: /^confirm password/i }).fill('password123')
    await screen.getByRole('button', { name: /create account/i }).click()

    await expect.element(screen.getByText(/missing signup token/i)).toBeInTheDocument()
    expect(api.auth.completeSignup).not.toHaveBeenCalled()
  })
})
