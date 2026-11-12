import { beforeEach, describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-svelte'
import DashboardNav from './DashboardNav.svelte'
import { resetSession, session } from '../../lib/auth/session.svelte'
import type { User, UserRole } from '../../lib/types'

function account(role: UserRole): User {
  return {
    id: 'viewer',
    email: 'viewer@example.com',
    first_name: '',
    last_name: '',
    email_verified: true,
    has_password: true,
    oauth_providers: [],
    created_at: '2026-01-01T00:00:00Z',
    role,
  }
}

describe('DashboardNav', () => {
  beforeEach(() => {
    // sessionStorage is cleared by vitest.setup.ts, so this starts unknown.
    resetSession()
  })

  it('renders Home and Settings links', async () => {
    session.set(account('user'))
    const screen = await render(DashboardNav)

    await expect
      .element(screen.getByRole('link', { name: /home/i }))
      .toHaveAttribute('href', '/dashboard')
    await expect
      .element(screen.getByRole('link', { name: /settings/i }))
      .toHaveAttribute('href', '/dashboard/settings')
  })

  it('renders a theme toggle', async () => {
    const screen = await render(DashboardNav)

    await expect.element(screen.getByRole('button', { name: /toggle theme/i })).toBeInTheDocument()
  })

  it('hides the Admin link for non-admin users', async () => {
    session.set(account('user'))
    const screen = await render(DashboardNav)

    await expect.element(screen.getByRole('link', { name: /home/i })).toBeInTheDocument()
    expect(screen.container.querySelector('a[href="/dashboard/admin"]')).toBeNull()
  })

  it('shows the Admin link for admin users', async () => {
    session.set(account('super_admin'))
    const screen = await render(DashboardNav)

    await expect
      .element(screen.getByRole('link', { name: /admin/i }))
      .toHaveAttribute('href', '/dashboard/admin')
  })

  it('shows no Admin link while the role is unknown, then adds it once known', async () => {
    // Nothing cached and the check hasn't answered: never guess "admin" (D5).
    expect(session.user).toBeNull()
    const screen = await render(DashboardNav)

    await expect.element(screen.getByRole('link', { name: /home/i })).toBeInTheDocument()
    expect(screen.container.querySelector('a[href="/dashboard/admin"]')).toBeNull()

    // A cached account without a role (e.g. stored by an older build) is no
    // better than none. The API always sends `role`, hence the cast.
    session.set({ ...account('user'), role: undefined } as unknown as User)
    await expect.element(screen.getByRole('link', { name: /settings/i })).toBeInTheDocument()
    expect(screen.container.querySelector('a[href="/dashboard/admin"]')).toBeNull()

    // The check confirms an admin: the entry appears without a remount.
    session.set(account('admin'))
    await expect
      .element(screen.getByRole('link', { name: /admin/i }))
      .toHaveAttribute('href', '/dashboard/admin')
  })

  it('marks the active route with aria-current="page"', async () => {
    const screen = await render(DashboardNav, { props: { pathname: '/dashboard/settings' } })

    await expect
      .element(screen.getByRole('link', { name: /settings/i }))
      .toHaveAttribute('aria-current', 'page')
    // Home is an exact match, so it is not current on a nested settings route.
    await expect
      .element(screen.getByRole('link', { name: /home/i }))
      .not.toHaveAttribute('aria-current')
  })
})
