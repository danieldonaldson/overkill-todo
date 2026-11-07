import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import AuthTabs from './AuthTabs.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      login: vi.fn(),
      register: vi.fn(),
    },
    oauth: {
      startLogin: vi.fn(),
      startLink: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigate: vi.fn(),
}))

import { api } from '../../lib/api'

describe('AuthTabs', () => {
  beforeEach(() => {
    vi.clearAllMocks()
    vi.restoreAllMocks()
  })

  it('opens on the sign-in panel and keeps the URL untouched', async () => {
    const replaceState = vi.spyOn(history, 'replaceState').mockImplementation(() => {})
    const screen = render(AuthTabs, { initial: 'login' })

    await expect.element(screen.getByRole('heading', { name: 'Welcome back' })).toBeInTheDocument()
    await expect.element(screen.getByRole('tab', { name: /sign in/i })).toHaveAttribute('data-state', 'active')
    expect(screen.container.querySelector('input[name="password"]')).not.toBeNull()
    expect(replaceState).not.toHaveBeenCalled()
  })

  it('opens on the create-account panel when mounted from /register', async () => {
    const screen = render(AuthTabs, { initial: 'register' })

    await expect.element(screen.getByRole('heading', { name: 'Create your account' })).toBeInTheDocument()
    await expect
      .element(screen.getByRole('tab', { name: /create account/i }))
      .toHaveAttribute('data-state', 'active')
    expect(screen.container.querySelector('input[name="password"]')).toBeNull()
  })

  it('swaps panels and rewrites the URL when a tab is selected', async () => {
    const replaceState = vi.spyOn(history, 'replaceState').mockImplementation(() => {})
    const screen = render(AuthTabs, { initial: 'login' })

    await screen.getByRole('tab', { name: /create account/i }).click()

    await expect.element(screen.getByRole('heading', { name: 'Create your account' })).toBeInTheDocument()
    expect(screen.container.querySelector('input[name="password"]')).toBeNull()
    await vi.waitFor(() => {
      expect(replaceState).toHaveBeenCalledWith(history.state, '', '/register')
    })
  })

  it('switches to the other panel from the footer prompt', async () => {
    const screen = render(AuthTabs, { initial: 'register' })

    await screen.getByRole('button', { name: /^sign in$/i }).click()

    await expect.element(screen.getByRole('heading', { name: 'Welcome back' })).toBeInTheDocument()
    await expect.element(screen.getByRole('tab', { name: /sign in/i })).toHaveAttribute('data-state', 'active')
  })

  it('carries the typed email across a tab switch', async () => {
    const screen = render(AuthTabs, { initial: 'login' })

    await screen.getByLabelText(/email/i).fill('someone@example.com')
    await screen.getByRole('tab', { name: /create account/i }).click()

    await expect.element(screen.getByLabelText(/email/i)).toHaveValue('someone@example.com')
  })

  it('keeps the shared Turnstile token alive across a tab switch', async () => {
    const screen = render(AuthTabs, { initial: 'login' })

    await screen.getByRole('tab', { name: /create account/i }).click()
    await screen.getByLabelText(/email/i).fill('someone@example.com')
    await screen.getByRole('button', { name: /continue/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.register).toHaveBeenCalledWith({
        email: 'someone@example.com',
        turnstile_token: 'dev-mode-skip',
      })
    })
  })

  it('restores the original URL when the user tabs back', async () => {
    const replaceState = vi.spyOn(history, 'replaceState').mockImplementation(() => {})
    const screen = render(AuthTabs, { initial: 'login' })

    await screen.getByRole('tab', { name: /create account/i }).click()
    await screen.getByRole('tab', { name: /sign in/i }).click()

    await vi.waitFor(() => {
      expect(replaceState).toHaveBeenLastCalledWith(history.state, '', '/login')
    })
  })
})
