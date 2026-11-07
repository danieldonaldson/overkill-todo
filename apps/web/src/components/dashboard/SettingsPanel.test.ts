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

describe('SettingsPanel', () => {
  beforeEach(() => {
    sessionStorage.clear()
    vi.clearAllMocks()
  })

  it('loads and displays account details', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)

    const screen = render(SettingsPanel)

    await expect.element(screen.getByText('test@example.com')).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: /unlink google/i })).toBeInTheDocument()
  })

  it('changes password for users with an existing password', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)
    vi.mocked(api.users.changePassword).mockResolvedValue(undefined)
    const screen = render(SettingsPanel)

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
    vi.mocked(api.auth.me).mockResolvedValue({ ...passwordUser, oauth_providers: [] })
    const screen = render(SettingsPanel)

    await screen.getByRole('button', { name: /link google/i }).click()

    expect(api.oauth.startLink).toHaveBeenCalledWith('google')
  })

  it('schedules account deletion after typing DELETE and password', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)
    vi.mocked(api.users.requestDeletion).mockResolvedValue({
      deletion_scheduled_at: '2026-07-04T00:00:00Z',
    })
    const screen = render(SettingsPanel)

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
  })

  it('blocks deletion until the password is provided', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)
    const screen = render(SettingsPanel)

    await screen.getByLabelText(/type delete to confirm/i).fill('DELETE')
    await screen.getByRole('button', { name: /delete my account/i }).click()

    await expect
      .element(screen.getByRole('alert'))
      .toHaveTextContent(/enter your password to confirm/i)
    expect(api.users.requestDeletion).not.toHaveBeenCalled()
  })

  it('surfaces an error when scheduling deletion fails', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)
    vi.mocked(api.users.requestDeletion).mockRejectedValue(new Error('Deletion service down'))
    const screen = render(SettingsPanel)

    await screen.getByLabelText(/confirm your password/i).fill('mypassword123')
    await screen.getByLabelText(/type delete to confirm/i).fill('DELETE')
    await screen.getByRole('button', { name: /delete my account/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent('Deletion service down')
  })

  it('surfaces an error when unlinking Google fails', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)
    vi.mocked(api.oauth.unlink).mockRejectedValue(new Error('Unlink failed'))
    const screen = render(SettingsPanel)

    await screen.getByRole('button', { name: /unlink google/i }).click()

    await expect.element(screen.getByRole('alert')).toHaveTextContent('Unlink failed')
  })

  it('exports account data as a downloadable JSON file', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)
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

    const screen = render(SettingsPanel)

    await screen.getByRole('button', { name: /export my data/i }).click()

    await vi.waitFor(() => expect(api.users.exportData).toHaveBeenCalled())
    expect(clickSpy).toHaveBeenCalled()
    expect(createUrl).toHaveBeenCalled()
    expect(revokeUrl).toHaveBeenCalledWith('blob:fake')
    await expect
      .element(screen.getByText(/data export has been downloaded/i))
      .toBeInTheDocument()

    clickSpy.mockRestore()
    createUrl.mockRestore()
    revokeUrl.mockRestore()
  })

  it('cancels a pending deletion', async () => {
    vi.mocked(api.auth.me).mockResolvedValue({
      ...passwordUser,
      deletion_scheduled_at: '2026-07-04T00:00:00Z',
    })
    vi.mocked(api.users.cancelDeletion).mockResolvedValue({})
    const screen = render(SettingsPanel)

    await screen.getByRole('button', { name: /cancel deletion/i }).click()

    await vi.waitFor(() => {
      expect(api.users.cancelDeletion).toHaveBeenCalled()
    })
  })

  it('logs out and navigates to login', async () => {
    vi.mocked(api.auth.me).mockResolvedValue(passwordUser)
    vi.mocked(api.auth.logout).mockResolvedValue(undefined)
    const screen = render(SettingsPanel)

    await screen.getByRole('button', { name: /log out/i }).click()

    await vi.waitFor(() => {
      expect(api.auth.logout).toHaveBeenCalled()
      expect(navigateReplace).toHaveBeenCalledWith('/login')
    })
  })
})
