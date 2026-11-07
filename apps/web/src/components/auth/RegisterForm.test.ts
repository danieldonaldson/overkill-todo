import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import RegisterForm from './RegisterForm.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      register: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
}))

import { api } from '../../lib/api'
import { navigate } from '../../lib/navigation'

describe('RegisterForm', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    history.replaceState(null, '', '/register')
  })

  it('renders email-first signup without password fields', async () => {
    const screen = render(RegisterForm, { turnstileToken: 'dev-mode-skip' })

    await expect.element(screen.getByLabelText(/email/i)).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /continue/i })).toBeInTheDocument()
    expect(screen.container.querySelector('input[name="password"]')).toBeNull()
  })

  it('validates email before submitting', async () => {
    const screen = render(RegisterForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('notanemail')
    await screen.getByRole('button', { name: /continue/i }).click()

    await expect.element(screen.getByText('Please enter a valid email address')).toBeInTheDocument()
    expect(api.auth.register).not.toHaveBeenCalled()
  })

  it('starts registration with a Turnstile token and redirects to verify-email', async () => {
    vi.mocked(api.auth.register).mockResolvedValue({
      success: true,
      message: 'Verification code sent',
    })

    const screen = render(RegisterForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('john@example.com')
    await screen.getByRole('button', { name: /continue/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.register).toHaveBeenCalledWith({
        email: 'john@example.com',
        turnstile_token: 'dev-mode-skip',
      })
    })
    expect(navigate).toHaveBeenCalledWith('/verify-email?email=john%40example.com')
  })

  it('surfaces an error and stays put when registration fails', async () => {
    vi.mocked(api.auth.register).mockRejectedValue(new Error('Email already registered'))
    const screen = render(RegisterForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('john@example.com')
    await screen.getByRole('button', { name: /continue/i }).click()

    await expect.element(screen.getByText('Email already registered')).toBeInTheDocument()
    expect(navigate).not.toHaveBeenCalled()
  })
})
