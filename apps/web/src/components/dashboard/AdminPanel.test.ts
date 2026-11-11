import { beforeEach, describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'
import AdminPanel from './AdminPanel.svelte'

vi.mock('../../lib/api', () => ({
  api: {
    auth: { me: vi.fn() },
    admin: { listUsers: vi.fn(), setRole: vi.fn() },
  },
}))

vi.mock('../../lib/navigation', () => ({
  navigateReplace: vi.fn(),
  redirect: vi.fn(),
}))

import { api } from '../../lib/api'
import { ApiErrorException } from '../../lib/api/errors'
import { resetSession, session } from '../../lib/auth/session.svelte'
import { navigateReplace } from '../../lib/navigation'
import type { AdminUserRow, User, UserRole } from '../../lib/types'

function viewer(role: UserRole): User {
  return {
    id: 'viewer',
    email: 'viewer@example.com',
    first_name: '',
    last_name: '',
    email_verified: true,
    has_password: true,
    oauth_providers: [],
    role,
  }
}

const alice: AdminUserRow = {
  id: 'u1',
  email: 'alice@example.com',
  role: 'user',
  email_verified: true,
  created_at: '2026-01-15T12:00:00Z',
}

const bob: AdminUserRow = {
  id: 'u2',
  email: 'bob@example.com',
  role: 'admin',
  email_verified: true,
  created_at: 'not-a-date',
}

function aliceRoleSelect(screen: Awaited<ReturnType<typeof render>>) {
  return screen.getByRole('row').filter({ hasText: 'alice@example.com' }).getByRole('combobox')
}

/** Seed the store as the tab's cache would: known, but not yet confirmed. */
function setCurrentUser(user: User) {
  session.set(user)
}

/** A promise plus its resolve/reject, for controlling when a mock answers. */
function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

describe('AdminPanel', () => {
  beforeEach(() => {
    resetSession()
    vi.mocked(api.admin.listUsers).mockResolvedValue({ users: [alice, bob], limit: 50, offset: 0 })
  })

  it('loads the list at once, behind a skeleton, without waiting for the role', async () => {
    // No cached user and no check yet: the server decides (D5).
    const list = deferred<{ users: AdminUserRow[]; limit: number; offset: number }>()
    vi.mocked(api.admin.listUsers).mockReturnValue(list.promise)

    const screen = await render(AdminPanel)

    await expect.element(screen.getByTestId('admin-skeleton')).toBeInTheDocument()
    expect(api.admin.listUsers).toHaveBeenCalledTimes(1)
    expect(screen.container.textContent).not.toMatch(/loading/i)

    list.resolve({ users: [alice], limit: 50, offset: 0 })

    await expect.element(screen.getByText('alice@example.com')).toBeInTheDocument()
    await expect.element(screen.getByTestId('admin-skeleton')).not.toBeInTheDocument()
    expect(navigateReplace).not.toHaveBeenCalled()
  })

  it('sends a non-admin Home when the server refuses the list', async () => {
    vi.mocked(api.admin.listUsers).mockRejectedValue(
      new ApiErrorException({ code: 'forbidden', message: 'Forbidden' }, { status: 403 })
    )

    const screen = await render(AdminPanel)

    await vi.waitFor(() => expect(navigateReplace).toHaveBeenCalledWith('/dashboard'))
    expect(screen.container.textContent).not.toContain('alice@example.com')
    // Leaving, not erroring: no alert for the refused request.
    expect(screen.container.textContent).not.toContain('Forbidden')
  })

  it('sends a confirmed non-admin Home before the list answers, and never shows it', async () => {
    const list = deferred<{ users: AdminUserRow[]; limit: number; offset: number }>()
    vi.mocked(api.admin.listUsers).mockReturnValue(list.promise)
    vi.mocked(api.auth.me).mockResolvedValue(viewer('user'))

    const screen = await render(AdminPanel)
    await session.revalidate()

    await vi.waitFor(() => expect(navigateReplace).toHaveBeenCalledWith('/dashboard'))
    expect(navigateReplace).toHaveBeenCalledTimes(1)

    // Even if the list then arrives, it is not rendered.
    list.resolve({ users: [alice], limit: 50, offset: 0 })
    await list.promise
    await new Promise((resolve) => setTimeout(resolve, 50))
    expect(screen.container.textContent).not.toContain('alice@example.com')
  })

  it('does not redirect on an unconfirmed (cached) non-admin role', async () => {
    // A stale cache may predate a promotion; only the check or the server decides.
    setCurrentUser(viewer('user'))

    const screen = await render(AdminPanel)

    await expect.element(screen.getByText('alice@example.com')).toBeInTheDocument()
    expect(navigateReplace).not.toHaveBeenCalled()
  })

  it('shows a read-only user list to admins', async () => {
    setCurrentUser(viewer('admin'))

    const screen = await render(AdminPanel)

    await expect.element(screen.getByText('alice@example.com')).toBeInTheDocument()
    await expect.element(screen.getByText('All registered users.')).toBeInTheDocument()
    expect(screen.container.querySelector('select')).toBeNull()
    // An unparseable timestamp is shown as-is rather than "Invalid Date".
    await expect.element(screen.getByText('not-a-date')).toBeInTheDocument()
    expect(navigateReplace).not.toHaveBeenCalled()
    // The header owns the account check; the panel must not trigger it.
    expect(api.auth.me).not.toHaveBeenCalled()
  })

  it('shows the load error when listing users fails', async () => {
    setCurrentUser(viewer('admin'))
    vi.mocked(api.admin.listUsers).mockRejectedValue(new Error('Forbidden'))

    const screen = await render(AdminPanel)

    await expect.element(screen.getByText('Forbidden')).toBeInTheDocument()
  })

  it('shows an empty state when there are no users', async () => {
    setCurrentUser(viewer('admin'))
    vi.mocked(api.admin.listUsers).mockResolvedValue({ users: [], limit: 50, offset: 0 })

    const screen = await render(AdminPanel)

    await expect.element(screen.getByText('No users.')).toBeInTheDocument()
  })

  it('lets super-admins change a role', async () => {
    setCurrentUser(viewer('super_admin'))
    vi.mocked(api.admin.setRole).mockResolvedValue({ ...alice, role: 'admin' })

    const screen = await render(AdminPanel)
    await expect.element(screen.getByText('Manage user roles.')).toBeInTheDocument()

    await aliceRoleSelect(screen).selectOptions('admin')

    await expect
      .element(screen.getByText('Updated alice@example.com to admin.'))
      .toBeInTheDocument()
    expect(api.admin.setRole).toHaveBeenCalledWith('u1', 'admin')
    await expect.element(aliceRoleSelect(screen)).toHaveValue('admin')
  })

  it('resets the select to the stored role when the change fails', async () => {
    setCurrentUser(viewer('super_admin'))
    vi.mocked(api.admin.setRole).mockRejectedValue(new Error('Cannot demote the last super admin'))

    const screen = await render(AdminPanel)
    await expect.element(aliceRoleSelect(screen)).toHaveValue('user')

    await aliceRoleSelect(screen).selectOptions('super_admin')

    await expect.element(screen.getByText('Cannot demote the last super admin')).toBeInTheDocument()
    await expect.element(aliceRoleSelect(screen)).toHaveValue('user')
    await expect.element(aliceRoleSelect(screen)).toBeEnabled()
  })
})
