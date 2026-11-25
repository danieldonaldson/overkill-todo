// Browser error reporter (OT-006 Decision 8): queue, dedupe, cap, batch and
// send. It reads only error objects, never forms, the DOM, the console or
// storage, and it stores nothing: the page-load ID lives in memory.
//
// Every public method swallows its own errors. Reporting must never break or
// slow the page, and the reporter never reports itself.

import type { components } from '../api/schema'
import { MAX_MESSAGE_CHARS, MAX_SOURCE_CHARS, MAX_STACK_CHARS, scrub } from './scrub'

export const ENDPOINT = '/api/v1/client-telemetry'
/** The server's body limit; batches are split to stay under it. */
export const MAX_BATCH_BYTES = 16 * 1024
/** The server's per-batch event limit. */
export const MAX_EVENTS_PER_BATCH = 30
/** Distinct errors reported from one page load; repeats still count. */
export const MAX_DISTINCT_ERRORS = 20
/** A queued error is sent at most this long after it happened. */
export const FLUSH_INTERVAL_MS = 10_000
const MAX_COUNT = 10_000
const MAX_PAGE_CHARS = 200
const MAX_ISLAND_CHARS = 100
const MAX_API_ROUTE_CHARS = 200
const TRACE_ID_PATTERN = /^[0-9a-f]{32}$/
const MAX_ATTRIBUTION_CHARS = 100
/** The server's ceiling for a vital's value (ms, or the CLS score). */
const MAX_VITAL_VALUE = 600_000
/** The server's ceiling for a navigation or island duration, in ms. */
const MAX_DURATION_MS = 600_000

export type ErrorKind = 'error' | 'unhandledrejection' | 'boundary' | 'api'

export interface ErrorEventBody {
  type: 'error'
  kind: ErrorKind
  message: string
  stack?: string
  source?: string
  island?: string
  api_route?: string
  status?: number
  network_failed?: boolean
  trace_id?: string
  count: number
}

/** A finished page load (full or in-site): the error rate's denominator. */
export interface PageLoadEventBody {
  type: 'page_load'
}

export type VitalName = 'LCP' | 'FCP' | 'TTFB' | 'INP' | 'CLS'
export type Rating = 'good' | 'needs-improvement' | 'poor'

/** One Web Vital (Decision 6). Attribution only ever comes with `poor`. */
export interface VitalEventBody {
  type: 'vital'
  name: VitalName
  value: number
  rating: Rating
  attribution?: string
}

/** A full load or in-site navigation with its timings (Decision 6a). Sampled. */
export type NavigationEventBody = components['schemas']['ClientNavigationEvent'] & {
  type: 'navigation'
}

/** When an island became interactive or ready (Decision 6b). Sampled. */
export type IslandEventBody = components['schemas']['ClientIslandEvent'] & { type: 'island' }

export type EventBody =
  ErrorEventBody | PageLoadEventBody | VitalEventBody | NavigationEventBody | IslandEventBody

export interface Batch {
  page_load_id: string
  release: string
  page: string
  events: EventBody[]
}

/** A captured error before scrubbing. */
export interface CapturedError {
  kind: ErrorKind
  message: string
  stack?: string
  /** The script URL, or `external` for another origin. */
  source?: string
  /** For `boundary` errors: the island (`TelemetryBoundary` name) that caught it. */
  island?: string
  /** For `api` errors: the route template, and the status or network failure. */
  apiRoute?: string
  status?: number
  networkFailed?: boolean
  /** For `api` errors: the response's `X-Trace-Id` (32 lowercase hex). */
  traceId?: string
}

export interface ReporterOptions {
  release: string
  /** The current `location.pathname`. */
  pathname: () => string
  /** Whether a Member is signed in now; a change starts a new page-load ID. */
  isSignedIn: () => boolean
  /**
   * Sends one serialized batch. `anonymous` batches were collected while
   * signed out and must go without cookies, so a session that appeared since
   * (signing in on this page) can't be credited with them. Must not throw (it
   * is guarded anyway).
   */
  send: (body: string, options: SendOptions) => void
  /** 32 lowercase hex characters. */
  newPageLoadId?: () => string
  setTimer?: (fn: () => void, ms: number) => unknown
  clearTimer?: (timer: unknown) => void
}

export interface SendOptions {
  anonymous: boolean
}

interface Queued {
  pageLoadId: string
  page: string
  /** Dedupe key; `null` for events that are never merged (page loads). */
  key: string | null
  event: EventBody
}

export function randomPageLoadId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
}

const RELEASE_PATTERN = /^[0-9A-Za-z._-]{1,64}$/

/**
 * `location.pathname` as the server expects it: no trailing slash, list pages
 * collapsed to one template, capped (OT-013 design D9). A malformed list id is
 * the same page as a real one.
 */
export function pageOf(pathname: string): string {
  const trimmed = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname
  const page = /^\/dashboard\/lists\/.+/.test(trimmed) ? '/dashboard/lists/[id]' : trimmed || '/'
  return Array.from(page).slice(0, MAX_PAGE_CHARS).join('')
}

/** The first stack line naming a frame (`at …` or `fn@url`), or ''. */
function topFrame(stack: string | undefined): string {
  if (!stack) return ''
  for (const line of stack.split('\n')) {
    const t = line.trim()
    if (t.startsWith('at ') || t.includes('@')) return t
  }
  return ''
}

const byteLength = (s: string) => new TextEncoder().encode(s).length

export class Reporter {
  private readonly options: Required<ReporterOptions>
  private readonly release: string
  private pageLoadId: string
  private signedIn: boolean
  /** Page-load IDs started while signed out (the ID rotates on sign-in or out). */
  private anonymousIds = new Set<string>()
  /** Fingerprints seen in the current page load (sent or queued). */
  private seen = new Set<string>()
  private queue: Queued[] = []
  private timer: unknown = null
  /**
   * Set when an API call couldn't reach the server: the timer holds the queue
   * until the server answers again (or the page is hidden), so the report of
   * that failure isn't lost to the same outage.
   */
  private offline = false

  constructor(options: ReporterOptions) {
    this.options = {
      newPageLoadId: randomPageLoadId,
      setTimer: (fn, ms) => setTimeout(fn, ms),
      clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
      ...options,
    }
    this.release = RELEASE_PATTERN.test(options.release) ? options.release : 'dev'
    this.pageLoadId = this.options.newPageLoadId()
    this.signedIn = this.safeSignedIn()
    if (!this.signedIn) this.anonymousIds.add(this.pageLoadId)
  }

  /** The page-load ID errors captured now would carry. */
  get currentPageLoadId(): string {
    this.rotateIfSessionChanged()
    return this.pageLoadId
  }

  /** Queue an error: scrubbed, deduplicated within the page load, capped. */
  capture(error: CapturedError): void {
    try {
      this.rotateIfSessionChanged()
      const message = scrub(error.message || 'Unknown error', MAX_MESSAGE_CHARS)
      const stack = error.stack ? scrub(error.stack, MAX_STACK_CHARS) : undefined
      const source = error.source ? scrub(error.source, MAX_SOURCE_CHARS) : undefined
      const island = error.island
        ? Array.from(error.island).slice(0, MAX_ISLAND_CHARS).join('')
        : undefined
      const key = `${error.kind}\n${island ?? ''}\n${message}\n${topFrame(stack)}`
      const page = pageOf(this.options.pathname())

      const queued = this.queue.find(
        (q) => q.key === key && q.pageLoadId === this.pageLoadId && q.page === page
      )
      if (queued && queued.event.type === 'error') {
        queued.event.count = Math.min(queued.event.count + 1, MAX_COUNT)
        return
      }
      if (!this.seen.has(key) && this.seen.size >= MAX_DISTINCT_ERRORS) return
      this.seen.add(key)

      const event: ErrorEventBody = { type: 'error', kind: error.kind, message, count: 1 }
      if (stack) event.stack = stack
      if (source) event.source = source
      if (island) event.island = island
      if (error.apiRoute) event.api_route = scrub(error.apiRoute, MAX_API_ROUTE_CHARS)
      if (error.status !== undefined && error.status >= 100 && error.status <= 599) {
        event.status = error.status
      }
      if (error.networkFailed) event.network_failed = true
      if (error.traceId && TRACE_ID_PATTERN.test(error.traceId)) event.trace_id = error.traceId
      this.queue.push({ pageLoadId: this.pageLoadId, page, key, event })
      this.schedule()
    } catch {
      // Never let reporting break the page.
    }
  }

  /**
   * Count a finished page load on the current page: the full load, or the
   * page an in-site navigation landed on. Never sampled, never merged.
   */
  pageLoad(): void {
    try {
      this.rotateIfSessionChanged()
      const page = pageOf(this.options.pathname())
      this.queue.push({
        pageLoadId: this.pageLoadId,
        page,
        key: null,
        event: { type: 'page_load' },
      })
      this.schedule()
    } catch {
      // Never let reporting break the page.
    }
  }

  /**
   * Queue a Web Vital for `pathname`, the page it was measured on, which may
   * no longer be the current one (LCP is final only at the first input, and a
   * visit ends as the next navigation starts). Never merged.
   */
  vital(vital: Omit<VitalEventBody, 'type'>, pathname: string): void {
    try {
      if (!Number.isFinite(vital.value)) return
      this.rotateIfSessionChanged()
      const value = Math.min(Math.max(vital.value, 0), MAX_VITAL_VALUE)
      const event: VitalEventBody = {
        type: 'vital',
        name: vital.name,
        // Whole ms are plenty; CLS keeps four decimals.
        value: vital.name === 'CLS' ? Math.round(value * 10_000) / 10_000 : Math.round(value),
        rating: vital.rating,
      }
      if (vital.rating === 'poor' && vital.attribution) {
        event.attribution = scrub(vital.attribution, MAX_ATTRIBUTION_CHARS)
      }
      this.queue.push({ pageLoadId: this.pageLoadId, page: pageOf(pathname), key: null, event })
      this.schedule()
    } catch {
      // Never let reporting break the page.
    }
  }

  /** Queue a navigation for `pathname`, the page it went to. Never merged. */
  navigation(navigation: Omit<NavigationEventBody, 'type'>, pathname: string): void {
    try {
      const durations = [
        navigation.total_ms,
        navigation.fetch_ms,
        navigation.swap_ms,
        navigation.transition_ms,
      ]
      // One out-of-range value would get the whole batch refused.
      if (durations.some((ms) => ms != null && !(ms >= 0 && ms <= MAX_DURATION_MS))) return
      this.push({ type: 'navigation', ...navigation }, pathname)
    } catch {
      // Never let reporting break the page.
    }
  }

  /** Queue an island timing mark for `pathname`, its page. Never merged. */
  island(island: Omit<IslandEventBody, 'type'>, pathname: string): void {
    try {
      // A panel that appeared 10 minutes in (scrolled into view late) isn't a
      // load time, and the server would refuse the batch.
      if (!(island.ms >= 0 && island.ms <= MAX_DURATION_MS)) return
      this.push(
        {
          type: 'island',
          ...island,
          name: Array.from(island.name).slice(0, MAX_ISLAND_CHARS).join(''),
          ms: Math.round(island.ms),
        },
        pathname
      )
    } catch {
      // Never let reporting break the page.
    }
  }

  private push(event: EventBody, pathname: string): void {
    try {
      this.rotateIfSessionChanged()
      this.queue.push({ pageLoadId: this.pageLoadId, page: pageOf(pathname), key: null, event })
      this.schedule()
    } catch {
      // Never let reporting break the page.
    }
  }

  /** An API call couldn't reach the server: hold timed flushes until it can. */
  markOffline(): void {
    this.offline = true
  }

  /** The server answered again: send what was held. */
  markOnline(): void {
    try {
      if (!this.offline) return
      this.offline = false
      if (this.queue.length > 0) this.flush()
    } catch {
      // Never let reporting break the page.
    }
  }

  /** Send everything queued now (page hidden or leaving, or the timer). */
  flush(): void {
    try {
      if (this.timer !== null) {
        this.options.clearTimer(this.timer)
        this.timer = null
      }
      const queued = this.queue
      this.queue = []
      for (const batch of this.batches(queued)) {
        try {
          const anonymous = this.anonymousIds.has(batch.page_load_id)
          this.options.send(JSON.stringify(batch), { anonymous })
        } catch {
          // A failed send loses that batch, nothing more.
        }
      }
    } catch {
      // Never let reporting break the page.
    }
  }

  private safeSignedIn(): boolean {
    try {
      return this.options.isSignedIn()
    } catch {
      return this.signedIn ?? false
    }
  }

  private rotateIfSessionChanged(): void {
    const signedIn = this.safeSignedIn()
    if (signedIn !== this.signedIn) {
      this.signedIn = signedIn
      this.pageLoadId = this.options.newPageLoadId()
      if (!signedIn) this.anonymousIds.add(this.pageLoadId)
      this.seen = new Set()
    }
  }

  private schedule(): void {
    if (this.timer !== null) return
    this.timer = this.options.setTimer(() => {
      this.timer = null
      if (this.offline) this.schedule()
      else this.flush()
    }, FLUSH_INTERVAL_MS)
  }

  /** Group by page load and page, then split to the server's limits. */
  batches(queued: Queued[]): Batch[] {
    const groups = new Map<string, Batch>()
    for (const q of queued) {
      const id = `${q.pageLoadId} ${q.page}`
      let batch = groups.get(id)
      if (!batch) {
        batch = { page_load_id: q.pageLoadId, release: this.release, page: q.page, events: [] }
        groups.set(id, batch)
      }
      batch.events.push(q.event)
    }
    const out: Batch[] = []
    for (const batch of groups.values()) {
      for (let i = 0; i < batch.events.length; i += MAX_EVENTS_PER_BATCH) {
        out.push(...fit({ ...batch, events: batch.events.slice(i, i + MAX_EVENTS_PER_BATCH) }))
      }
    }
    return out
  }
}

/**
 * Split `batch` until each part serializes under {@link MAX_BATCH_BYTES}. A
 * single event that is still too big has its stack, then its message, trimmed.
 */
export function fit(batch: Batch): Batch[] {
  if (byteLength(JSON.stringify(batch)) <= MAX_BATCH_BYTES) return [batch]
  if (batch.events.length > 1) {
    const half = Math.ceil(batch.events.length / 2)
    return [
      ...fit({ ...batch, events: batch.events.slice(0, half) }),
      ...fit({ ...batch, events: batch.events.slice(half) }),
    ]
  }
  const only = batch.events[0]
  // Only an error can outgrow the limit; other events are a few bytes.
  if (only.type !== 'error') return [batch]
  const event = { ...only }
  while (byteLength(JSON.stringify({ ...batch, events: [event] })) > MAX_BATCH_BYTES) {
    if (event.stack) {
      const chars = Array.from(event.stack)
      const half = Math.floor(chars.length / 2)
      if (half === 0) delete event.stack
      else event.stack = chars.slice(0, half).join('')
    } else if (event.source) {
      delete event.source
    } else {
      const chars = Array.from(event.message)
      event.message = chars.slice(0, Math.floor(chars.length / 2)).join('')
    }
  }
  return [{ ...batch, events: [event] }]
}

/**
 * The transport (Decision 8): `sendBeacon`, or a `keepalive` fetch when the
 * beacon is missing or refused. Never `HttpClient`, so a report can't trigger
 * a session refresh or a sign-in redirect, or report its own failure.
 * Anonymous batches always use the fetch with `credentials: 'omit'`, because
 * a beacon always sends cookies.
 */
export function beaconSend(body: string, { anonymous }: SendOptions = { anonymous: false }): void {
  if (!anonymous) {
    try {
      const blob = new Blob([body], { type: 'application/json' })
      if (typeof navigator.sendBeacon === 'function' && navigator.sendBeacon(ENDPOINT, blob)) return
    } catch {
      // Fall through to fetch.
    }
  }
  try {
    void fetch(ENDPOINT, {
      method: 'POST',
      body,
      headers: { 'Content-Type': 'application/json' },
      keepalive: true,
      credentials: anonymous ? 'omit' : 'same-origin',
    }).catch(() => {})
  } catch {
    // Reporting is best-effort.
  }
}
