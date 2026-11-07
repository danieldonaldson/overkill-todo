import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-svelte'
import DashboardNav from './DashboardNav.svelte'
import DashboardNavWrapper from './DashboardNavWrapper.test.svelte'
import DashboardNavAdminWrapper from './DashboardNavAdminWrapper.test.svelte'

describe('DashboardNav', () => {
  it('renders Home and Settings links', async () => {
    const screen = render(DashboardNavWrapper)

    await expect.element(screen.getByRole('link', { name: /home/i })).toHaveAttribute('href', '/dashboard')
    await expect.element(screen.getByRole('link', { name: /settings/i })).toHaveAttribute('href', '/dashboard/settings')
  })

  it('renders a theme toggle', async () => {
    const screen = render(DashboardNavWrapper)

    await expect.element(screen.getByRole('button', { name: /toggle theme/i })).toBeInTheDocument()
  })

  it('hides the Admin link for non-admin users', async () => {
    const screen = render(DashboardNavWrapper)

    await expect.element(screen.getByRole('link', { name: /home/i })).toBeInTheDocument()
    expect(screen.container.querySelector('a[href="/dashboard/admin"]')).toBeNull()
  })

  it('shows the Admin link for admin users', async () => {
    const screen = render(DashboardNavAdminWrapper)

    await expect
      .element(screen.getByRole('link', { name: /admin/i }))
      .toHaveAttribute('href', '/dashboard/admin')
  })

  it('marks the active route with aria-current="page"', async () => {
    const screen = render(DashboardNav, { props: { pathname: '/dashboard/settings' } })

    await expect
      .element(screen.getByRole('link', { name: /settings/i }))
      .toHaveAttribute('aria-current', 'page')
    // Home is an exact match, so it is not current on a nested settings route.
    await expect
      .element(screen.getByRole('link', { name: /home/i }))
      .not.toHaveAttribute('aria-current')
  })
})
