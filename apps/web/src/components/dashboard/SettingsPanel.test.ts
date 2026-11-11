import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import SettingsPanel from './SettingsPanel.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: {
      me: vi.fn(),
      logout: vi.fn(),
    },
    users: {
      changePassword: vi.fn(),
      setPassword: vi.fn(),
      exportData: vi.fn(),
      requestDeletion: vi.fn(),
      cancelDeletion: vi.fn(),
    },
    oauth: {
      startLink: vi.fn(),
      unlink: vi.fn(),
    },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
  redirect: vi.fn(),
}))

import { api } from '../../lib/api'
import { resetSession, session } from '../../lib/auth/session.svelte'
import { getSessionUser } from '../../lib/auth/session'
import { getTokenExpiry, setTokenExpiry } from '../../lib/auth/token'
import { navigateReplace } from '../../lib/navigation'
import type { User } from '../../lib/types'

const passwordUser: User = {
  id: '1',
  email: 'test@example.com',
  first_name: '',
  last_name: '',
  email_verified: true,
  has_password: true,
  oauth_providers: ['google'],
}

/** Seed the tab's session store (and its sessionStorage cache). */
function setCurrentUser(user: User) {
  session.set(user)
}

describe('SettingsPanel', () => {
  beforeEach(() => {
    resetSession()
  })

  it('loads and displays account details', async () => {
    setCurrentUser(passwordUser)

    const screen = await render(SettingsPanel)

    await expect.element(screen.getByText('test@example.com')).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /unlink google/i })).toBeInTheDocument()
    // The header owns the account check; the panel must not trigger it.
    expect(api.auth.me).not.toHaveBeenCalled()
  })

  it('shows section placeholders, not forms, until the account is known', async () => {
    const screen = await render(SettingsPanel)

    await expect.element(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
    await expect.element(screen.getByTestId('settings-skeleton')).toBeInTheDocument()
    // Which password form to show depends on has_password, so neither is shown.
    expect(screen.container.querySelector('form')).toBeNull()
    expect(screen.container.textContent).not.toMatch(/loading/i)

    // The background check answers: the real sections replace the placeholders.
    setCurrentUser({ ...passwordUser, has_password: false })

    await expect.element(screen.getByText('Set a password')).toBeInTheDocument()
    await expect.element(screen.getByTestId('settings-skeleton')).not.toBeInTheDocument()
  })

  it('shows the server details when the check corrects a stale cache', async () => {
    setCurrentUser({ ...passwordUser, has_password: false })
    const screen = await render(SettingsPanel)
    await expect.element(screen.getByText('Set a password')).toBeInTheDocument()

    setCurrentUser(passwordUser)

    await expect.element(screen.getByLabelText(/current password/i)).toBeInTheDocument()
    expect(screen.container.textContent).not.toContain('Set a password')
  })

  it('records a newly set password in the session store', async () => {
    setCurrentUser({ ...passwordUser, has_password: false })
    vi.mocked(api.users.setPassword).mockResolvedValue(undefined)
    const screen = await render(SettingsPanel)

    await screen.getByLabelText(/^new password/i).fill('newpassword123')
    await screen.getByRole('button', { name: /set password/i }).click()

    await expect.element(screen.getByText('Password set.')).toBeInTheDocument()
    expect(api.users.setPassword).toHaveBeenCalledWith({ new_password: 'newpassword123' })
    expect(session.user?.has_password).toBe(true)
    // Written through, so a later page in the tab starts from it.
    expect(getSessionUser()?.has_password).toBe(true)
    await expect.element(screen.getByLabelText(/current password/i)).toBeInTheDocument()
  })

  it('records an unlinked Google account in the session store', async () => {
    setCurrentUser(passwordUser)
    vi.mocked(api.oauth.unlink).mockResolvedValue(undefined)
    const screen = await render(SettingsPanel)

    await screen.getByRole('button', { name: /unlink google/i }).click()

    await expect.element(screen.getByText('Google account unlinked.')).toBeInTheDocument()
    expect(session.user?.oauth_providers).toEqual([])
    expect(getSessionUser()?.oauth_providers).toEqual([])
    await expect.element(screen.getByText('Not linked')).toBeInTheDocument()
  })

  it('changes password for users with an existing password', async () => {
    setCurrentUser(passwordUser)
    vi.mocked(api.users.changePassword).mockResolvedValue(undefined)
    const screen = await render(SettingsPanel)

    await screen.getByLabelText(/current password/i).fill('oldpassword123')
    await screen.getByLabelText(/^new password/i).fill('newpassword123')
    await screen.getByRole('button', { name: /change password/i }).click()

    await vi.waitFor(() => {
      expect(api.users.changePassword).toHaveBeenCalledWith({
        current_password: 'oldpassword123',
        new_password: 'newpassword123',
      })
    })
  })

  it('links Google when not already linked', async () => {
    setCurrentUser({ ...passwordUser, oauth_providers: [] })
    const screen = await render(SettingsPanel)

    await screen.getByRole('button', { name: /link google/i }).click()

    expect(api.oauth.startLink).toHaveBeenCalledWith('google')
  })

  it('schedules account deletion after typing DELETE and password', async () => {
    setCurrentUser(passwordUser)
    vi.mocked(api.users.requestDeletion).mockResolvedValue({
      deletion_scheduled_at: '2026-07-04T00:00:00Z',
    })
    const screen = await render(SettingsPanel)

    await screen.getByLabelText(/confirm your password/i).fill('mypassword123')
    await screen.getByLabelText(/type delete to confirm/i).fill('DELETE')
    await screen.getByRole('button', { name: /delete my account/i }).click()

    await vi.waitFor(() => {
      expect(api.users.requestDeletion).toHaveBeenCalledWith({
        confirmation: 'DELETE',
        password: 'mypassword123',
      })
    })
    await expect.element(screen.getByText(/scheduled for deletion/i)).toBeInTheDocument()
    expect(session.user?.deletion_scheduled_at).toBe('2026-07-04T00:00:00Z')
    expect(getSessionUser()?.deletion_scheduled_at).toBe('2026-07-04T00:00:00Z')
  })

  it('blocks deletion until the password is provided', async () => {
    setCurrentUser(passwordUser)
    const screen = await render(SettingsPanel)

    await screen.getByLabelText(/type delete to confirm/i).fill('DELETE')
    await screen.getByRole('button', { name: /delete my account/i }).click()

    await expect
      .element(screen.getByRole('alert'))
      .toMatchTextContent(/enter your password to confirm/i)
    expect(api.users.requestDeletion).not.toHaveBeenCalled()
  })

  it('surfaces an error when scheduling deletion fails', async () => {
    setCurrentUser(passwordUser)
    vi.mocked(api.users.requestDeletion).mockRejectedValue(new Error('Deletion service down'))
    const screen = await render(SettingsPanel)

    await screen.getByLabelText(/confirm your password/i).fill('mypassword123')
    await screen.getByLabelText(/type delete to confirm/i).fill('DELETE')
    await screen.getByRole('button', { name: /delete my account/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent('Deletion service down')
  })

  it('surfaces an error when unlinking Google fails', async () => {
    setCurrentUser(passwordUser)
    vi.mocked(api.oauth.unlink).mockRejectedValue(new Error('Unlink failed'))
    const screen = await render(SettingsPanel)

    await screen.getByRole('button', { name: /unlink google/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent('Unlink failed')
    // A failed change leaves the record as it was.
    expect(session.user?.oauth_providers).toEqual(['google'])
    expect(getSessionUser()?.oauth_providers).toEqual(['google'])
    await expect.element(screen.getByText('Linked', { exact: true })).toBeInTheDocument()
  })

  it('exports account data as a downloadable JSON file', async () => {
    setCurrentUser(passwordUser)
    vi.mocked(api.users.exportData).mockResolvedValue({
      id: 'user-1',
      email: 'test@example.com',
      first_name: '',
      last_name: '',
      email_verified: true,
      has_password: true,
      role: 'user',
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-01T00:00:00Z',
      linked_oauth_accounts: [],
      exported_at: '2026-06-20T00:00:00Z',
    })
    // Don't trigger a real browser download or leak object URLs.
    const clickSpy = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})
    const createUrl = vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:fake')
    const revokeUrl = vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {})

    const screen = await render(SettingsPanel)

    await screen.getByRole('button', { name: /export my data/i }).click()

    await vi.waitFor(() => expect(api.users.exportData).toHaveBeenCalled())
    expect(clickSpy).toHaveBeenCalled()
    expect(createUrl).toHaveBeenCalled()
    expect(revokeUrl).toHaveBeenCalledWith('blob:fake')
    await expect.element(screen.getByText(/data export has been downloaded/i)).toBeInTheDocument()

    clickSpy.mockRestore()
    createUrl.mockRestore()
    revokeUrl.mockRestore()
  })

  it('cancels a pending deletion', async () => {
    setCurrentUser({
      ...passwordUser,
      deletion_scheduled_at: '2026-07-04T00:00:00Z',
    })
    vi.mocked(api.users.cancelDeletion).mockResolvedValue({})
    const screen = await render(SettingsPanel)

    await screen.getByRole('button', { name: /cancel deletion/i }).click()

    await vi.waitFor(() => {
      expect(api.users.cancelDeletion).toHaveBeenCalled()
    })
    await expect.element(screen.getByText('Account deletion cancelled.')).toBeInTheDocument()
    expect(session.user?.deletion_scheduled_at).toBeNull()
    expect(getSessionUser()?.deletion_scheduled_at).toBeNull()
  })

  it('logs out through the session store and navigates to login', async () => {
    setTokenExpiry(1_700_000_000)
    setCurrentUser(passwordUser)
    vi.mocked(api.auth.logout).mockResolvedValue(undefined)
    const screen = await render(SettingsPanel)

    await screen.getByRole('button', { name: /log out/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.logout).toHaveBeenCalledWith({ keepalive: true })
      expect(navigateReplace).toHaveBeenCalledWith('/login')
    })
    // Local state, the cache and the session hint are gone.
    expect(session.user).toBeNull()
    expect(getSessionUser()).toBeNull()
    expect(getTokenExpiry()).toBeNull()
  })
})
