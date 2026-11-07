import { describe, expect, it, vi } from 'vitest'
import { render } from 'vitest-browser-svelte'

const { toggleMode } = vi.hoisted(() => ({ toggleMode: vi.fn() }))
vi.mock('mode-watcher', () => ({
  toggleMode,
  mode: { current: 'light' },
}))

import ThemeToggle from './ThemeToggle.svelte'

describe('ThemeToggle', () => {
  it('renders a labelled toggle button reflecting the current (light) mode', async () => {
    const screen = render(ThemeToggle)

    const button = screen.getByRole('button', { name: /toggle theme/i })
    await expect.element(button).toBeInTheDocument()
    await expect.element(button).toHaveAttribute('title', 'Switch to dark mode')
  })

  it('calls toggleMode when clicked', async () => {
    const screen = render(ThemeToggle)

    await screen.getByRole('button', { name: /toggle theme/i }).click()

    expect(toggleMode).toHaveBeenCalled()
  })
})
