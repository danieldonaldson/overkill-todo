import { describe, expect, it } from 'vitest'

import {
  type NavigationReport,
  currentNavigation,
  installNavigationTiming,
  timingSampled,
} from './navigation'

/** A `ViewTransition` whose promises the test settles. */
function transition() {
  let resolveReady!: () => void
  let rejectReady!: (e: unknown) => void
  let resolveFinished!: () => void
  const ready = new Promise<void>((res, rej) => {
    resolveReady = res
    rejectReady = rej
  })
  const finished = new Promise<void>((res) => {
    resolveFinished = res
  })
  return { ready, finished, resolveReady, rejectReady, resolveFinished }
}

/** Let promise callbacks run. */
const settle = () => new Promise((r) => setTimeout(r, 0))

function harness({
  sampled = true,
  supports = true,
  alreadyLoadedAt = null as number | null,
} = {}) {
  const target = new EventTarget()
  const win = new EventTarget()
  let clock = 0
  let path = '/dashboard'
  const reports: { nav: NavigationReport; page: string }[] = []
  installNavigationTiming({
    report: (nav, page) => reports.push({ nav, page }),
    sampled,
    target,
    win,
    now: () => clock,
    pathname: () => path,
    supportsViewTransitions: () => supports,
    alreadyLoadedAt: () => alreadyLoadedAt,
  })
  const at = (ms: number) => {
    clock = ms
  }
  const fire = (type: string, extra: object = {}) => {
    target.dispatchEvent(Object.assign(new Event(type), extra))
  }
  const start = (ms: number, to: string) => {
    at(ms)
    fire('astro:before-preparation', { to: new URL(to, 'https://overkill.example') })
  }
  return {
    reports,
    at,
    fire,
    start,
    win,
    setPath: (p: string) => {
      path = p
    },
  }
}

describe('installNavigationTiming', () => {
  it('times the full load as hard, from 0 to the first page-load', () => {
    const h = harness()
    h.at(850.4)
    h.fire('astro:page-load')
    expect(h.reports).toEqual([
      {
        nav: { nav_type: 'hard', view_transition: 'none', outcome: 'completed', total_ms: 850 },
        page: '/dashboard',
      },
    ])
    expect(currentNavigation()).toMatchObject({ id: 0, type: 'hard', start: 0 })
  })

  it('uses the load time when the page had already loaded', () => {
    const h = harness({ alreadyLoadedAt: 1200 })
    h.fire('astro:page-load')
    expect(h.reports).toHaveLength(1)
    expect(h.reports[0].nav.total_ms).toBe(1200)
  })

  it('times an in-site navigation through every phase in order', async () => {
    const h = harness()
    h.at(500)
    h.fire('astro:page-load')
    h.start(1000, '/dashboard/settings/')
    expect(currentNavigation()).toMatchObject({
      type: 'soft',
      start: 1000,
      pathname: '/dashboard/settings/',
    })
    h.at(1120)
    h.fire('astro:after-preparation')
    const vt = transition()
    h.at(1130)
    h.fire('astro:before-swap', { viewTransition: vt })
    h.at(1138)
    h.fire('astro:after-swap')
    vt.resolveReady()
    h.at(1150)
    h.fire('astro:page-load')
    await settle()
    // Still animating: not reported until `finished` settles.
    expect(h.reports).toHaveLength(1)
    h.at(1400)
    vt.resolveFinished()
    await settle()
    expect(h.reports[1]).toEqual({
      nav: {
        nav_type: 'soft',
        view_transition: 'ran',
        outcome: 'completed',
        total_ms: 400,
        fetch_ms: 120,
        swap_ms: 8,
        transition_ms: 270,
      },
      page: '/dashboard/settings/',
    })
  })

  it('ends at page-load when the animation finished first', async () => {
    const h = harness()
    h.fire('astro:page-load')
    h.start(0, '/dashboard/settings')
    h.fire('astro:after-preparation')
    const vt = transition()
    h.fire('astro:before-swap', { viewTransition: vt })
    h.fire('astro:after-swap')
    vt.resolveReady()
    h.at(200)
    vt.resolveFinished()
    await settle()
    expect(h.reports).toHaveLength(1)
    h.at(600)
    h.fire('astro:page-load')
    expect(h.reports[1].nav).toMatchObject({ total_ms: 600, transition_ms: 200 })
  })

  it('labels a transition whose ready rejected as skipped, with no transition phase', async () => {
    const h = harness()
    h.fire('astro:page-load')
    h.start(0, '/dashboard/admin')
    const vt = transition()
    h.fire('astro:before-swap', { viewTransition: vt })
    vt.rejectReady(new DOMException('skipped', 'AbortError'))
    vt.resolveFinished()
    h.at(90)
    h.fire('astro:page-load')
    await settle()
    expect(h.reports[1].nav.view_transition).toBe('skipped')
    expect(h.reports[1].nav.transition_ms).toBeUndefined()
  })

  it('labels a browser without startViewTransition unsupported', async () => {
    const h = harness({ supports: false })
    h.fire('astro:page-load')
    h.start(0, '/dashboard/settings')
    h.at(40)
    h.fire('astro:after-preparation')
    // Astro's simulated transition: ready and finished both resolve.
    const vt = transition()
    h.fire('astro:before-swap', { viewTransition: vt })
    h.at(45)
    h.fire('astro:after-swap')
    vt.resolveReady()
    vt.resolveFinished()
    h.at(60)
    h.fire('astro:page-load')
    await settle()
    expect(h.reports[1].nav).toEqual({
      nav_type: 'soft',
      view_transition: 'unsupported',
      outcome: 'completed',
      total_ms: 60,
      fetch_ms: 40,
      swap_ms: 5,
    })
  })

  it('counts an interrupted navigation as aborted for where it was heading, then times the next', async () => {
    const h = harness()
    h.fire('astro:page-load')
    h.start(0, '/dashboard/settings')
    const before = currentNavigation().id
    h.start(50, '/dashboard/admin')
    expect(currentNavigation().id).toBe(before + 1)
    expect(h.reports[1]).toEqual({
      nav: { nav_type: 'soft', view_transition: 'skipped', outcome: 'aborted' },
      page: '/dashboard/settings',
    })
    h.at(100)
    h.fire('astro:after-preparation')
    h.fire('astro:before-swap')
    h.fire('astro:after-swap')
    h.at(150)
    h.fire('astro:page-load')
    await settle()
    expect(h.reports).toHaveLength(3)
    expect(h.reports[2]).toMatchObject({
      nav: { outcome: 'completed', total_ms: 100, fetch_ms: 50 },
      page: '/dashboard/admin',
    })
  })

  it('does not abort a navigation that loaded and is only finishing its animation', async () => {
    const h = harness()
    h.fire('astro:page-load')
    h.start(0, '/dashboard/settings')
    const vt = transition()
    h.fire('astro:before-swap', { viewTransition: vt })
    vt.resolveReady()
    h.at(100)
    h.fire('astro:page-load')
    h.start(150, '/dashboard')
    h.at(160)
    vt.resolveFinished()
    await settle()
    const outcomes = h.reports.map((r) => [r.page, r.nav.outcome])
    expect(outcomes).toEqual([
      ['/dashboard', 'completed'],
      ['/dashboard/settings', 'completed'],
    ])
    expect(h.reports[1].nav.total_ms).toBe(160)
  })

  it('drops a navigation still open at pagehide (Astro fell back to a full load)', async () => {
    const h = harness()
    h.fire('astro:page-load')
    h.start(0, '/dashboard/settings')
    h.fire('astro:after-preparation')
    h.win.dispatchEvent(new Event('pagehide'))
    h.fire('astro:page-load')
    await settle()
    expect(h.reports).toHaveLength(1)
    expect(h.reports[0].nav.nav_type).toBe('hard')
  })

  it('times back and forward navigations like links', async () => {
    const h = harness()
    h.fire('astro:page-load')
    h.start(0, '/dashboard')
    h.fire('astro:after-preparation', { navigationType: 'traverse' })
    h.fire('astro:before-swap')
    h.fire('astro:after-swap')
    h.at(30)
    h.fire('astro:page-load')
    expect(h.reports[1]).toMatchObject({
      nav: { nav_type: 'soft', outcome: 'completed', total_ms: 30 },
      page: '/dashboard',
    })
  })

  it('tracks the current navigation but reports nothing when not sampled', async () => {
    const h = harness({ sampled: false })
    expect(timingSampled()).toBe(false)
    h.fire('astro:page-load')
    h.start(10, '/dashboard/settings')
    expect(currentNavigation()).toMatchObject({ type: 'soft', start: 10 })
    h.fire('astro:before-swap')
    h.fire('astro:page-load')
    await settle()
    expect(h.reports).toEqual([])
  })

  it('never lets a throwing reporter escape into the page', () => {
    const target = new EventTarget()
    installNavigationTiming({
      report: () => {
        throw new Error('boom')
      },
      sampled: true,
      target,
      win: new EventTarget(),
      alreadyLoadedAt: () => null,
    })
    expect(() => target.dispatchEvent(new Event('astro:page-load'))).not.toThrow()
  })
})
