import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import LoginForm from './LoginForm.svelte'

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
  redirect: vi.fn(),
}))

import { navigate } from '../../lib/navigation'

describe('LoginForm', () => {
  const mockFetch = vi.fn()
  const getPasswordInput = (screen: ReturnType<typeof render>) =>
    screen.getByRole('textbox', { name: /password/i })

  beforeEach(() => {
    vi.stubGlobal('fetch', mockFetch)
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    mockFetch.mockReset()
  })

  it('renders email, password, OAuth, and password-recovery affordances', async () => {
    const screen = render(LoginForm, { turnstileToken: 'dev-mode-skip' })

    await expect.element(screen.getByLabelText(/email/i)).toBeInTheDocument()
    await expect.element(getPasswordInput(screen)).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /sign in/i })).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /google/i })).toBeInTheDocument()
    await expect
      .element(screen.getByRole('link', { name: /forgot your password/i }))
      .toHaveAttribute('href', '/forgot-password')
  })

  it('validates fields before submitting', async () => {
    const screen = render(LoginForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('notanemail')
    await getPasswordInput(screen).fill('short')
    await screen.getByRole('button', { name: /sign in/i }).click()

    await expect.element(screen.getByText('Please enter a valid email address')).toBeInTheDocument()
    await expect.element(screen.getByText('Must be at least 8 characters')).toBeInTheDocument()
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('submits valid credentials with a Turnstile token and redirects to dashboard', async () => {
    mockFetch.mockResolvedValue({
      ok: true,
      headers: { get: vi.fn().mockReturnValue(null) },
      json: () =>
        Promise.resolve({
          token: 'jwt-token',
          expires_at: Math.floor(Date.now() / 1000) + 3600,
          user: {
            id: 'user-1',
            email: 'test@example.com',
            first_name: '',
            last_name: '',
            email_verified: true,
            has_password: true,
            oauth_providers: [],
          },
        }),
    })

    const screen = render(LoginForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('test@example.com')
    await getPasswordInput(screen).fill('password123')
    await screen.getByRole('button', { name: /sign in/i }).click()

    await vi.waitFor(() => {
      expect(mockFetch).toHaveBeenCalledWith(
        '/api/v1/auth/login',
        expect.objectContaining({
          method: 'POST',
          body: JSON.stringify({
            email: 'test@example.com',
            password: 'password123',
            turnstile_token: 'dev-mode-skip',
          }),
        })
      )
    })
    expect(navigate).toHaveBeenCalledWith('/dashboard')
  })

  it('surfaces a structured credential error and clears the Turnstile token', async () => {
    mockFetch.mockResolvedValue({
      ok: false,
      headers: { get: vi.fn().mockReturnValue(null) },
      json: () =>
        Promise.resolve({
          error_code: 'invalid_credentials',
          error: 'Invalid email or password',
          attempts_remaining: 2,
        }),
    })

    const screen = render(LoginForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('test@example.com')
    await getPasswordInput(screen).fill('password123')
    await screen.getByRole('button', { name: /sign in/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent('Invalid email or password')
    expect(navigate).not.toHaveBeenCalled()
  })

  it('falls back to a generic message when the failure is unstructured', async () => {
    mockFetch.mockResolvedValue({
      ok: false,
      headers: { get: vi.fn().mockReturnValue(null) },
      json: () => Promise.resolve({ message: 'Service unavailable' }),
    })

    const screen = render(LoginForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('test@example.com')
    await getPasswordInput(screen).fill('password123')
    await screen.getByRole('button', { name: /sign in/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent('Service unavailable')
  })

  it('routes incomplete signups back to email verification', async () => {
    mockFetch.mockResolvedValue({
      ok: false,
      headers: { get: vi.fn().mockReturnValue(null) },
      json: () =>
        Promise.resolve({
          error_code: 'signup_incomplete',
          message: 'Please complete your registration',
          email: 'incomplete@example.com',
        }),
    })

    const screen = render(LoginForm, { turnstileToken: 'dev-mode-skip' })

    await screen.getByLabelText(/email/i).fill('incomplete@example.com')
    await getPasswordInput(screen).fill('password123')
    await screen.getByRole('button', { name: /sign in/i }).click()

    await vi.waitFor(() => {
      expect(navigate).toHaveBeenCalledWith('/verify-email?email=incomplete%40example.com')
    })
  })
})
