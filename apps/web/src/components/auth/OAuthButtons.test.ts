import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import OAuthButtons from './OAuthButtons.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    oauth: {
      startLogin: vi.fn(),
      startLink: vi.fn(),
    },
  },
}))

import { api } from '../../lib/api'

describe('OAuthButtons', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders a Google login button with divider copy', async () => {
    const screen = render(OAuthButtons)

    await expect.element(screen.getByText('Or continue with')).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /google/i })).toHaveAttribute('type', 'button')
  })

  it('starts Google login by default', async () => {
    const screen = render(OAuthButtons)

    await screen.getByRole('button', { name: /google/i }).click()

    expect(api.oauth.startLogin).toHaveBeenCalledWith('google')
    expect(api.oauth.startLink).not.toHaveBeenCalled()
  })

  it('starts Google linking in link mode', async () => {
    const screen = render(OAuthButtons, { props: { mode: 'link' } })

    await screen.getByRole('button', { name: /google/i }).click()

    expect(api.oauth.startLink).toHaveBeenCalledWith('google')
    expect(api.oauth.startLogin).not.toHaveBeenCalled()
  })
})
