// Navigation timing from Astro's ClientRouter lifecycle (OT-006 Decision 6a).
//
// | Mark | Event                                                   |
// |------|---------------------------------------------------------|
// | t0   | `astro:before-preparation` (navigation starts)          |
// | t1   | `astro:after-preparation` (new page fetched)            |
// | t2   | `astro:before-swap` (carries the `ViewTransition`)      |
// | t3   | `astro:after-swap` (DOM swapped)                        |
// | t4   | `astro:page-load` (scripts ran, page usable)            |
//
// fetch = t1 − t0, swap = t3 − t2, transition = t2 → `finished` (only when it
// ran), total = t0 → the later of t4 and `finished`.
//
// - A new t0 before the open navigation's t4 aborts it: reported as
//   `aborted` for the page it was heading to, with no durations.
// - A navigation still open when the document is hidden for good
//   (`pagehide`: Astro fell back to a full load) is dropped; the new document
//   measures itself as a `hard` load.
// - The full load is a `hard` navigation, timed from `timeOrigin` (0) to the
//   first `astro:page-load` (window `load`).
//
// This module also owns "the current navigation" (kind, start, destination)
// that `TelemetryBoundary` times panels against (Decision 6b). It is tracked
// whether or not this page load is sampled; only reporting is sampled.

import type { components } from '../api/schema'

type Schemas = components['schemas']
export type NavType = Schemas['NavType']
export type ViewTransitionResult = Schemas['ViewTransitionResult']
/** The navigation event body, without its `type` tag. */
export type NavigationReport = Schemas['ClientNavigationEvent']

/** The navigation that showed the current page. */
export interface CurrentNavigation {
  /** Rises by one per navigation; a different ID means a newer navigation. */
  id: number
  type: NavType
  /** `performance.now()` at its start: 0 for the full load. */
  start: number
  /** The pathname it was heading to. */
  pathname: string
}

let current: CurrentNavigation = {
  id: 0,
  type: 'hard',
  start: 0,
  pathname: typeof location === 'undefined' ? '' : location.pathname,
}
let sampled = false

/** The navigation that showed (or is about to show) the current page. */
export function currentNavigation(): CurrentNavigation {
  return current
}

/** Whether this page load reports navigation and panel timings. */
export function timingSampled(): boolean {
  return sampled
}

/** Minimal shape of the `ViewTransition` Astro puts on `astro:before-swap`. */
interface TransitionLike {
  ready: Promise<unknown>
  finished: Promise<unknown>
}

interface OpenNavigation {
  pathname: string
  t0: number
  t1?: number
  t2?: number
  t3?: number
  t4?: number
  /** No `document.startViewTransition`: Astro's fallback swap ran. */
  unsupported: boolean
  /** Astro gave no transition object at all. */
  noTransition: boolean
  readyRejected: boolean
  /** Waiting for the transition's `finished` to settle. */
  awaitingFinish: boolean
  finishedAt?: number
  done: boolean
}

export interface NavigationTimingOptions {
  /** Queue a navigation for the page it went to. Must not throw (guarded anyway). */
  report: (navigation: NavigationReport, pathname: string) => void
  /** Whether this page load is sampled (`PUBLIC_RUM_SAMPLE_RATE`, decided once). */
  sampled: boolean
  /** Where Astro's lifecycle events arrive. */
  target?: EventTarget
  /** Where `pagehide` arrives. */
  win?: EventTarget
  now?: () => number
  pathname?: () => string
  supportsViewTransitions?: () => boolean
  /**
   * When the full load's first `astro:page-load` already happened before this
   * ran, its time (ms from `timeOrigin`); otherwise null.
   */
  alreadyLoadedAt?: () => number | null
}

const round = (ms: number) => Math.max(0, Math.round(ms))

function toPathname(event: Event, fallback: string): string {
  try {
    const to = (event as Event & { to?: URL }).to
    return to instanceof URL ? to.pathname : fallback
  } catch {
    return fallback
  }
}

function defaultAlreadyLoadedAt(): number | null {
  if (document.readyState !== 'complete') return null
  const entry = performance.getEntriesByType('navigation')[0] as
    PerformanceNavigationTiming | undefined
  return entry && entry.loadEventEnd > 0 ? entry.loadEventEnd : null
}

/**
 * Time this document's navigations. Returns nothing; reports go through
 * `report`. Never throws into the page: every listener is guarded.
 */
export function installNavigationTiming({
  report,
  sampled: isSampled,
  target = document,
  win = window,
  now = () => performance.now(),
  pathname = () => location.pathname,
  supportsViewTransitions = () => typeof document.startViewTransition === 'function',
  alreadyLoadedAt = defaultAlreadyLoadedAt,
}: NavigationTimingOptions): void {
  sampled = isSampled
  current = { id: 0, type: 'hard', start: 0, pathname: pathname() }
  let open: OpenNavigation | null = null
  let hardReported = false

  const send = (navigation: NavigationReport, page: string) => {
    if (!sampled) return
    try {
      report(navigation, page)
    } catch {
      // Never let reporting break the page.
    }
  }

  const guard =
    <E extends Event>(fn: (event: E) => void) =>
    (event: Event) => {
      try {
        fn(event as E)
      } catch {
        // Never let timing break navigation.
      }
    }

  const reportHard = (total: number) => {
    if (hardReported) return
    hardReported = true
    send(
      {
        nav_type: 'hard',
        view_transition: 'none',
        outcome: 'completed',
        total_ms: round(total),
      },
      current.id === 0 ? current.pathname : pathname()
    )
  }

  const complete = (nav: OpenNavigation) => {
    if (nav.done || nav.t4 === undefined || nav.awaitingFinish) return
    nav.done = true
    if (open === nav) open = null
    let viewTransition: ViewTransitionResult
    if (nav.unsupported) viewTransition = 'unsupported'
    else if (nav.noTransition || nav.readyRejected) viewTransition = 'skipped'
    else viewTransition = 'ran'
    const end = Math.max(nav.t4, nav.finishedAt ?? nav.t4)
    const body: NavigationReport = {
      nav_type: 'soft',
      view_transition: viewTransition,
      outcome: 'completed',
      total_ms: round(end - nav.t0),
    }
    if (nav.t1 !== undefined) body.fetch_ms = round(nav.t1 - nav.t0)
    if (nav.t2 !== undefined && nav.t3 !== undefined) body.swap_ms = round(nav.t3 - nav.t2)
    if (viewTransition === 'ran' && nav.t2 !== undefined && nav.finishedAt !== undefined) {
      body.transition_ms = round(nav.finishedAt - nav.t2)
    }
    send(body, nav.pathname)
  }

  const loadedAt = alreadyLoadedAt()
  if (loadedAt !== null) reportHard(loadedAt)

  target.addEventListener(
    'astro:before-preparation',
    guard((event) => {
      const t0 = now()
      if (open && open.t4 === undefined) {
        open.done = true
        send({ nav_type: 'soft', view_transition: 'skipped', outcome: 'aborted' }, open.pathname)
      }
      const to = toPathname(event, pathname())
      open = {
        pathname: to,
        t0,
        unsupported: false,
        noTransition: false,
        readyRejected: false,
        awaitingFinish: false,
        done: false,
      }
      current = { id: current.id + 1, type: 'soft', start: t0, pathname: to }
    })
  )
  target.addEventListener(
    'astro:after-preparation',
    guard(() => {
      if (open) open.t1 = now()
    })
  )
  target.addEventListener(
    'astro:before-swap',
    guard((event) => {
      const nav = open
      if (!nav) return
      nav.t2 = now()
      nav.unsupported = !supportsViewTransitions()
      const transition = (event as Event & { viewTransition?: TransitionLike }).viewTransition
      if (!transition) {
        nav.noTransition = true
        return
      }
      nav.awaitingFinish = true
      transition.ready.then(
        () => {},
        () => {
          nav.readyRejected = true
        }
      )
      const settle = () => {
        try {
          nav.finishedAt = now()
          nav.awaitingFinish = false
          complete(nav)
        } catch {
          // Never let timing break navigation.
        }
      }
      transition.finished.then(settle, settle)
    })
  )
  target.addEventListener(
    'astro:after-swap',
    guard(() => {
      if (open) open.t3 = now()
    })
  )
  target.addEventListener(
    'astro:page-load',
    guard(() => {
      const t4 = now()
      if (!open) {
        // The full load's first page-load (window `load`).
        if (current.id === 0) reportHard(t4)
        return
      }
      open.t4 = t4
      complete(open)
    })
  )
  win.addEventListener(
    'pagehide',
    guard(() => {
      // Astro fell back to a full load (or the tab is going away): the new
      // document times its own load as `hard`.
      if (open && open.t4 === undefined) {
        open.done = true
        open = null
      }
    })
  )
}
