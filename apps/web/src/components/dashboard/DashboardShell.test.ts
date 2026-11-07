import { describe, expect, it } from 'vitest'
import { render } from 'vitest-browser-svelte'
import DashboardShellWrapper from './DashboardShellWrapper.test.svelte'

describe('DashboardShell', () => {
  it('renders brand header and dashboard nav', async () => {
    const screen = render(DashboardShellWrapper)

    await expect.element(screen.getByRole('link', { name: /app/i })).toHaveAttribute('href', '/dashboard')
    await expect.element(screen.getByRole('banner')).toBeInTheDocument()
    await expect.element(screen.getByRole('navigation', { name: /dashboard/i })).toBeInTheDocument()
  })

  it('renders children inside the main region', async () => {
    const screen = render(DashboardShellWrapper, { props: { childText: 'Secret dashboard' } })

    await expect.element(screen.getByRole('main')).toBeInTheDocument()
    await expect.element(screen.getByTestId('shell-content')).toHaveTextContent('Secret dashboard')
  })
})
