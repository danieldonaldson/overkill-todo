import { beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-svelte'
import BoundaryHarness from './BoundaryHarness.test.svelte'

vi.mock('./index', () => ({ reportBoundaryError: vi.fn() }))

import { reportBoundaryError } from './index'

describe('TelemetryBoundary', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('renders its content when nothing fails, and reports nothing', async () => {
    const screen = await render(BoundaryHarness, { control: { fail: false } })
    await expect.element(screen.getByText('Panel content')).toBeInTheDocument()
    await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
    expect(reportBoundaryError).not.toHaveBeenCalled()
  })

  it('replaces only its island with the failure message and reports the island', async () => {
    const screen = await render(BoundaryHarness, { control: { fail: true } })
    await expect.element(screen.getByRole('alert')).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument()
    await expect
      .element(screen.getByRole('button', { name: 'Reload the page' }))
      .toBeInTheDocument()
    await expect.element(screen.getByText('Panel content')).not.toBeInTheDocument()
    await expect
      .element(screen.getByRole('link', { name: 'Outside the boundary' }))
      .toBeInTheDocument()
    expect(reportBoundaryError).toHaveBeenCalledTimes(1)
    expect(vi.mocked(reportBoundaryError).mock.calls[0][0]).toBe('SettingsPanel')
  })

  it('shows no error text, stack or trace ID', async () => {
    const screen = await render(BoundaryHarness, { control: { fail: true } })
    await expect.element(screen.getByRole('alert')).toBeInTheDocument()
    const text = screen.container.textContent ?? ''
    for (const leaked of ['boom', 'alice', 'example.com', 'reset-password', 'Error', ' at ']) {
      expect(text).not.toContain(leaked)
    }
  })

  it('rebuilds the island in place when Try again succeeds', async () => {
    const control = { fail: true }
    const screen = await render(BoundaryHarness, { control })
    await expect.element(screen.getByRole('alert')).toBeInTheDocument()
    control.fail = false
    await screen.getByRole('button', { name: 'Try again' }).click()
    await expect.element(screen.getByText('Panel content')).toBeInTheDocument()
    await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
    expect(reportBoundaryError).toHaveBeenCalledTimes(1)
  })

  it('shows the message again, and reports again, when Try again fails again', async () => {
    const screen = await render(BoundaryHarness, { control: { fail: true } })
    await screen.getByRole('button', { name: 'Try again' }).click()
    await expect.element(screen.getByRole('alert')).toBeInTheDocument()
    await expect.element(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument()
    await vi.waitFor(() => expect(reportBoundaryError).toHaveBeenCalledTimes(2))
  })

  it('fits a phone-width screen without horizontal scrolling', async () => {
    await page.viewport(320, 640)
    try {
      const screen = await render(BoundaryHarness, { control: { fail: true } })
      const alert = screen.getByRole('alert').element() as HTMLElement
      await expect.element(alert).toBeInTheDocument()
      expect(alert.scrollWidth).toBeLessThanOrEqual(alert.clientWidth)
      expect(alert.getBoundingClientRect().right).toBeLessThanOrEqual(320)
      expect(document.documentElement.scrollWidth).toBeLessThanOrEqual(320)
    } finally {
      await page.viewport(1280, 720)
    }
  })

  it('renders nothing in place of an island with no UI, but still reports', async () => {
    const screen = await render(BoundaryHarness, { control: { fail: true }, fallback: 'none' })
    await expect
      .element(screen.getByRole('link', { name: 'Outside the boundary' }))
      .toBeInTheDocument()
    await expect.element(screen.getByRole('alert')).not.toBeInTheDocument()
    expect(reportBoundaryError).toHaveBeenCalledTimes(1)
  })
})
