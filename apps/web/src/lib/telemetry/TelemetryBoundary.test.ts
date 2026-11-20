import { beforeEach, describe, expect, it, vi } from 'vitest'
import { page } from 'vitest/browser'
import { render } from 'vitest-browser-svelte'
import BoundaryHarness from './BoundaryHarness.test.svelte'

vi.mock('./index', () => ({ reportBoundaryError: vi.fn(), reportIslandTiming: vi.fn() }))

import { reportBoundaryError, reportIslandTiming } from './index'
import { createIslandTiming } from './island-timing'
import { installNavigationTiming } from './navigation'

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

describe('TelemetryBoundary timing', () => {
  let target: EventTarget

  /** Fresh navigation state: a full load, sampled unless told otherwise. */
  function fullLoad(sampled = true) {
    target = new EventTarget()
    installNavigationTiming({
      report: () => {},
      sampled,
      target,
      win: new EventTarget(),
      pathname: () => '/dashboard',
      alreadyLoadedAt: () => null,
    })
  }

  function navigate(to: string) {
    target.dispatchEvent(
      Object.assign(new Event('astro:before-preparation'), {
        to: new URL(to, 'https://overkill.example'),
      })
    )
  }

  /** `[name, stage, nav type, pathname]` of each mark reported. */
  const marks = () =>
    vi
      .mocked(reportIslandTiming)
      .mock.calls.map(([name, stage, nav]) => [name, stage, nav.type, nav.pathname])

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('times an island with nothing to load on a full load: interactive and ready at mount', async () => {
    fullLoad()
    const screen = await render(BoundaryHarness, { control: { fail: false } })
    await expect.element(screen.getByText('Panel content')).toBeInTheDocument()
    expect(marks()).toEqual([
      ['SettingsPanel', 'interactive', 'hard', '/dashboard'],
      ['SettingsPanel', 'ready', 'hard', '/dashboard'],
    ])
    const [, , nav, at] = vi.mocked(reportIslandTiming).mock.calls[0]
    expect(nav.start).toBe(0)
    expect(at).toBeGreaterThan(0)
  })

  it('times an island shown by an in-site navigation from that navigation', async () => {
    fullLoad()
    navigate('/dashboard/settings')
    const timing = createIslandTiming()
    const screen = await render(BoundaryHarness, { control: { fail: false }, timing })
    await expect.element(screen.getByText('Panel content')).toBeInTheDocument()
    expect(marks()).toEqual([['SettingsPanel', 'interactive', 'soft', '/dashboard/settings']])
    timing.markReady()
    expect(marks()).toHaveLength(2)
    expect(marks()[1]).toEqual(['SettingsPanel', 'ready', 'soft', '/dashboard/settings'])
    const [, , nav, at] = vi.mocked(reportIslandTiming).mock.calls[1]
    expect(nav.start).toBeGreaterThan(0)
    expect(at).toBeGreaterThanOrEqual(nav.start)
  })

  it('ignores a second markReady()', async () => {
    fullLoad()
    const timing = createIslandTiming()
    await render(BoundaryHarness, { control: { fail: false }, timing })
    timing.markReady()
    timing.markReady()
    expect(marks().filter(([, stage]) => stage === 'ready')).toHaveLength(1)
  })

  it('drops the marks of a navigation superseded by a newer one', async () => {
    fullLoad()
    navigate('/dashboard/settings')
    const timing = createIslandTiming()
    await render(BoundaryHarness, { control: { fail: false }, timing })
    // The Member moved on before the panel showed its content.
    navigate('/dashboard')
    timing.markReady()
    expect(marks()).toEqual([['SettingsPanel', 'interactive', 'soft', '/dashboard/settings']])
  })

  it('drops an island created for a navigation superseded before it mounted', async () => {
    fullLoad()
    navigate('/dashboard/settings')
    const timing = createIslandTiming()
    navigate('/dashboard/admin')
    await render(BoundaryHarness, { control: { fail: false }, timing })
    timing.markReady()
    expect(reportIslandTiming).not.toHaveBeenCalled()
  })

  it('reports nothing when the page load is not sampled', async () => {
    fullLoad(false)
    const timing = createIslandTiming()
    await render(BoundaryHarness, { control: { fail: false }, timing })
    timing.markReady()
    expect(reportIslandTiming).not.toHaveBeenCalled()
  })

  it('is not timed again when Try again rebuilds the content', async () => {
    fullLoad()
    const control = { fail: true }
    const screen = await render(BoundaryHarness, { control })
    await expect.element(screen.getByRole('alert')).toBeInTheDocument()
    control.fail = false
    await screen.getByRole('button', { name: 'Try again' }).click()
    await expect.element(screen.getByText('Panel content')).toBeInTheDocument()
    expect(marks().map(([, stage]) => stage)).toEqual(['interactive', 'ready'])
  })
})
