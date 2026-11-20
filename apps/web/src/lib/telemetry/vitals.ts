// Web Vitals (OT-006 Decision 6).
//
// - LCP, FCP and TTFB come from `web-vitals` for the full page load only, and
//   are reported for the page that loaded, even if LCP is only final after an
//   in-site navigation started.
// - INP and CLS come from our own observers (`event` over 40 ms, plus
//   `first-input`, as `web-vitals` uses), per page *visit*: `web-vitals`
//   accumulates both over the document's lifetime, which under ClientRouter
//   would credit Settings' interactions to the Home page that was loaded in
//   full. A visit starts at the full load or at `astro:page-load` after a swap,
//   and ends at `astro:before-preparation` or when the page is hidden. The
//   algorithms mirror `web-vitals` so a full load with no swaps gives the same
//   values.
//
// Sampling is decided once per full load; errors and page loads never are.

import { onFCP, onLCP, onTTFB } from 'web-vitals/attribution'

import type { Rating, VitalEventBody, VitalName } from './reporter'

/** `[good, poor]` thresholds: good is ≤ the first, poor is > the second. */
export const THRESHOLDS: Record<VitalName, readonly [number, number]> = {
  LCP: [2500, 4000],
  FCP: [1800, 3000],
  TTFB: [800, 1800],
  INP: [200, 500],
  CLS: [0.1, 0.25],
}

export function rate(name: VitalName, value: number): Rating {
  const [good, poor] = THRESHOLDS[name]
  if (value <= good) return 'good'
  return value <= poor ? 'needs-improvement' : 'poor'
}

/** A Web Vital to queue, and the pathname it belongs to. */
export type ReportVital = (vital: Omit<VitalEventBody, 'type'>, pathname: string) => void

const MAX_SELECTOR_CHARS = 100

/**
 * A short CSS selector for `node`, in the format `web-vitals` uses for its own
 * attribution (`div.card>button.primary`, stopping at an ID), at most 100
 * characters. Only tag names, IDs and classes: never text or attribute values.
 */
export function selectorOf(node: Node | null | undefined): string | undefined {
  let selector = ''
  try {
    let current: Node | null | undefined = node
    while (current && current.nodeType !== Node.DOCUMENT_NODE) {
      const el = current as Element
      const part = el.id
        ? `#${el.id}`
        : [
            current.nodeType === Node.ELEMENT_NODE
              ? current.nodeName.toLowerCase()
              : current.nodeName.toUpperCase().replace(/^#/, ''),
            ...Array.from(el.classList ?? []).sort(),
          ].join('.')
      if (selector.length + part.length > MAX_SELECTOR_CHARS - 1) {
        selector ||= part
        break
      }
      selector = selector ? `${part}>${selector}` : part
      if (el.id) break
      current = current.parentNode
    }
  } catch {
    // An odd node: whatever we have so far.
  }
  return selector ? Array.from(selector).slice(0, MAX_SELECTOR_CHARS).join('') : undefined
}

/** The subset of `PerformanceEventTiming` the INP tracker reads. */
export interface EventEntry {
  interactionId?: number
  duration: number
  startTime: number
  target?: Node | null
}

/** The subset of `LayoutShift` the CLS tracker reads. */
export interface ShiftEntry {
  value: number
  startTime: number
  hadRecentInput: boolean
  sources?: readonly { node?: Node | null }[]
}

/** Longest interactions kept as INP candidates, as `web-vitals` does. */
const MAX_INTERACTIONS = 10

interface Interaction {
  id: number
  latency: number
  target?: Node | null
}

/** INP over a set of `event` entries: the worst interaction, or p98 past 50. */
export class InteractionTracker {
  private longest: Interaction[] = []
  private minId = Infinity
  private maxId = 0

  add(entry: EventEntry): void {
    const id = entry.interactionId
    if (!id) return
    this.minId = Math.min(this.minId, id)
    this.maxId = Math.max(this.maxId, id)
    const existing = this.longest.find((i) => i.id === id)
    if (existing) {
      if (entry.duration > existing.latency) {
        existing.latency = entry.duration
        existing.target = entry.target
      }
    } else if (
      this.longest.length < MAX_INTERACTIONS ||
      entry.duration > (this.longest.at(-1)?.latency ?? 0)
    ) {
      this.longest.push({ id, latency: entry.duration, target: entry.target })
    } else {
      return
    }
    this.longest.sort((a, b) => b.latency - a.latency)
    this.longest.length = Math.min(this.longest.length, MAX_INTERACTIONS)
  }

  /**
   * Interactions seen, estimated from the IDs (they rise by 7 per interaction
   * in Chromium), for when `performance.interactionCount` isn't available.
   */
  get estimatedCount(): number {
    return this.maxId === 0 ? 0 : Math.round((this.maxId - this.minId) / 7) + 1
  }

  /** INP for `interactionCount` interactions, or null if none was measured. */
  value(interactionCount = this.estimatedCount): { latency: number; target?: Node | null } | null {
    if (this.longest.length === 0) return null
    const index = Math.min(this.longest.length - 1, Math.floor(interactionCount / 50))
    const pick = this.longest[index]
    return { latency: pick.latency, target: pick.target }
  }
}

/** CLS over a set of `layout-shift` entries: the largest session window. */
export class ShiftTracker {
  private windowValue = 0
  private window: ShiftEntry[] = []
  private max = 0
  private maxWindow: ShiftEntry[] = []
  private shifted = false

  add(entry: ShiftEntry): void {
    if (entry.hadRecentInput) return
    this.shifted = true
    const first = this.window[0]
    const last = this.window.at(-1)
    if (
      this.windowValue &&
      first &&
      last &&
      entry.startTime - last.startTime < 1000 &&
      entry.startTime - first.startTime < 5000
    ) {
      this.windowValue += entry.value
      this.window.push(entry)
    } else {
      this.windowValue = entry.value
      this.window = [entry]
    }
    if (this.windowValue > this.max) {
      this.max = this.windowValue
      this.maxWindow = [...this.window]
    }
  }

  /** Whether any shift without recent input was seen. */
  get hasShifts(): boolean {
    return this.shifted
  }

  /** CLS, and the node of the largest shift in the worst window. */
  value(): { value: number; node?: Node | null } {
    let largest: ShiftEntry | undefined
    for (const entry of this.maxWindow) {
      if (!largest || entry.value > largest.value) largest = entry
    }
    return { value: this.max, node: largest?.sources?.find((s) => s.node)?.node }
  }
}

export interface VisitOptions {
  report: ReportVital
  /** The current `location.pathname`. */
  pathname: () => string
  /** `performance.interactionCount` where the browser has it, else null. */
  interactionCount?: () => number | null
  now?: () => number
}

/** INP and CLS for one page visit at a time. */
export class VisitTracker {
  private interactions = new InteractionTracker()
  private shifts = new ShiftTracker()
  private page = ''
  private startedAt = 0
  private baseCount = 0
  /** The visit began at a load or swap (not a return to a hidden tab). */
  private fresh = true
  private active = false
  private readonly options: Required<VisitOptions>

  constructor(options: VisitOptions) {
    this.options = { interactionCount: () => null, now: () => performance.now(), ...options }
  }

  get isActive(): boolean {
    return this.active
  }

  /**
   * Start a visit on the current page. The full load's visit starts at 0, so
   * buffered entries from before this script ran count.
   */
  start({ fullLoad = false, fresh = true } = {}): void {
    this.interactions = new InteractionTracker()
    this.shifts = new ShiftTracker()
    this.page = this.options.pathname()
    this.startedAt = fullLoad ? 0 : this.options.now()
    this.baseCount = this.options.interactionCount() ?? 0
    this.fresh = fresh
    this.active = true
  }

  addEvent(entry: EventEntry): void {
    if (this.active && entry.startTime >= this.startedAt) this.interactions.add(entry)
  }

  addShift(entry: ShiftEntry): void {
    if (this.active && entry.startTime >= this.startedAt) this.shifts.add(entry)
  }

  /**
   * End the visit and report its INP (if there was an interaction) and CLS.
   * A visit resumed after the tab was hidden reports CLS only if something
   * shifted, so hiding and showing a tab doesn't add a row of zeros each time.
   * Returns whether anything was reported.
   */
  end(): boolean {
    if (!this.active) return false
    this.active = false
    let reported = false
    const count = this.options.interactionCount()
    const visitCount = count === null ? this.interactions.estimatedCount : count - this.baseCount
    // Interactions too quick for an `event` entry (under 40 ms) still count:
    // as `web-vitals` does for soft navigations, a visit with interactions
    // but no candidate has an INP of 8 ms, so p75 isn't drawn only from the
    // slow ones.
    const inp =
      this.interactions.value(visitCount) ?? (visitCount > 0 ? { latency: 8, target: null } : null)
    if (inp) {
      const rating = rate('INP', inp.latency)
      this.options.report(
        {
          name: 'INP',
          value: inp.latency,
          rating,
          attribution: rating === 'poor' ? selectorOf(inp.target) : undefined,
        },
        this.page
      )
      reported = true
    }
    if (this.fresh || this.shifts.hasShifts) {
      const cls = this.shifts.value()
      const rating = rate('CLS', cls.value)
      this.options.report(
        {
          name: 'CLS',
          value: cls.value,
          rating,
          attribution: rating === 'poor' ? selectorOf(cls.node) : undefined,
        },
        this.page
      )
      reported = true
    }
    return reported
  }
}

export interface InstallVitalsOptions {
  report: ReportVital
  /** Share of full loads that measure, 0–1 (`PUBLIC_RUM_SAMPLE_RATE`). */
  sampleRate: number
  /** Called after a visit ends because the page was hidden, to send at once. */
  onHidden?: () => void
  random?: () => number
  /** Where Astro's lifecycle and `visibilitychange` events arrive. */
  target?: EventTarget
  isHidden?: () => boolean
  pathname?: () => string
  /** Whether to start `web-vitals` and the observers (off in unit tests). */
  measure?: boolean
}

/** The running visit tracking, returned for tests. */
export interface Vitals {
  visit: VisitTracker
}

function observe<T>(type: string, onEntries: (entries: T[]) => void, extra: object = {}) {
  if (!PerformanceObserver.supportedEntryTypes?.includes(type)) return null
  const observer = new PerformanceObserver((list) => onEntries(list.getEntries() as T[]))
  observer.observe({ type, buffered: true, ...extra } as PerformanceObserverInit)
  return observer
}

/**
 * Measure this document's Web Vitals, if this full load is sampled; null if
 * it isn't. Must not throw; the caller guards it anyway.
 */
export function installVitals({
  report,
  sampleRate,
  onHidden = () => {},
  random = Math.random,
  target = document,
  isHidden = () => document.visibilityState === 'hidden',
  pathname = () => location.pathname,
  measure = true,
}: InstallVitalsOptions): Vitals | null {
  if (!(random() < sampleRate)) return null

  const fullLoadPage = pathname()
  const loadVital =
    (name: 'LCP' | 'FCP' | 'TTFB') =>
    (metric: { value: number; rating: Rating; attribution?: object }) => {
      const target =
        name === 'LCP' ? (metric.attribution as { target?: string } | undefined)?.target : undefined
      report(
        {
          name,
          value: metric.value,
          rating: metric.rating,
          attribution: metric.rating === 'poor' ? target : undefined,
        },
        fullLoadPage
      )
      if (isHidden()) onHidden()
    }
  if (measure) {
    onLCP(loadVital('LCP'))
    onFCP(loadVital('FCP'))
    onTTFB(loadVital('TTFB'))
  }

  const visit = new VisitTracker({
    report,
    pathname,
    interactionCount: () => {
      const count = (performance as { interactionCount?: unknown }).interactionCount
      return typeof count === 'number' ? count : null
    },
  })
  const events = measure
    ? observe<EventEntry>('event', (entries) => entries.forEach((e) => visit.addEvent(e)), {
        durationThreshold: 40,
      })
    : null
  // The first interaction of the document whatever its duration, as
  // `web-vitals` uses it, so a full load with only quick interactions has one.
  if (measure) {
    observe<EventEntry>('first-input', (entries) => entries.forEach((e) => visit.addEvent(e)))
  }
  const shifts = measure
    ? observe<ShiftEntry>('layout-shift', (entries) => entries.forEach((e) => visit.addShift(e)))
    : null
  /** Process entries the observers hold but haven't delivered yet. */
  const drain = () => {
    events?.takeRecords().forEach((e) => visit.addEvent(e as unknown as EventEntry))
    shifts?.takeRecords().forEach((e) => visit.addShift(e as unknown as ShiftEntry))
  }

  visit.start({ fullLoad: true })
  let navigating = false
  target.addEventListener('astro:before-preparation', () => {
    navigating = true
    drain()
    visit.end()
  })
  target.addEventListener('astro:page-load', () => {
    // The full load's own first page-load: its visit is already running.
    if (!navigating) return
    navigating = false
    visit.start()
  })
  target.addEventListener('visibilitychange', () => {
    if (isHidden()) {
      drain()
      if (visit.end()) onHidden()
    } else if (!navigating && !visit.isActive) {
      visit.start({ fresh: false })
    }
  })
  return { visit }
}
