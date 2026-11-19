// Browser error reporter (OT-006 Decision 8): queue, dedupe, cap, batch and
// send. It reads only error objects, never forms, the DOM, the console or
// storage, and it stores nothing: the page-load ID lives in memory.
//
// Every public method swallows its own errors. Reporting must never break or
// slow the page, and the reporter never reports itself.

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

export type ErrorKind = 'error' | 'unhandledrejection' | 'boundary' | 'api'

export interface ErrorEventBody {
  type: 'error'
  kind: ErrorKind
  message: string
  stack?: string
  source?: string
  count: number
}

export interface Batch {
  page_load_id: string
  release: string
  page: string
  events: ErrorEventBody[]
}

/** A captured error before scrubbing. */
export interface CapturedError {
  kind: ErrorKind
  message: string
  stack?: string
  /** The script URL, or `external` for another origin. */
  source?: string
}

export interface ReporterOptions {
  release: string
  /** The current `location.pathname`. */
  pathname: () => string
  /** Whether a Member is signed in now; a change starts a new page-load ID. */
  isSignedIn: () => boolean
  /** Sends one serialized batch. Must not throw (it is guarded anyway). */
  send: (body: string) => void
  /** 32 lowercase hex characters. */
  newPageLoadId?: () => string
  setTimer?: (fn: () => void, ms: number) => unknown
  clearTimer?: (timer: unknown) => void
}

interface Queued {
  pageLoadId: string
  page: string
  key: string
  event: ErrorEventBody
}

export function randomPageLoadId(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16))
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
}

const RELEASE_PATTERN = /^[0-9A-Za-z._-]{1,64}$/

/** `location.pathname` as the server expects it: no trailing slash, capped. */
export function pageOf(pathname: string): string {
  const trimmed = pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname
  return Array.from(trimmed || '/')
    .slice(0, MAX_PAGE_CHARS)
    .join('')
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
  /** Fingerprints seen in the current page load (sent or queued). */
  private seen = new Set<string>()
  private queue: Queued[] = []
  private timer: unknown = null

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
      const key = `${error.kind}\n${message}\n${topFrame(stack)}`
      const page = pageOf(this.options.pathname())

      const queued = this.queue.find(
        (q) => q.key === key && q.pageLoadId === this.pageLoadId && q.page === page
      )
      if (queued) {
        queued.event.count = Math.min(queued.event.count + 1, MAX_COUNT)
        return
      }
      if (!this.seen.has(key) && this.seen.size >= MAX_DISTINCT_ERRORS) return
      this.seen.add(key)

      const event: ErrorEventBody = { type: 'error', kind: error.kind, message, count: 1 }
      if (stack) event.stack = stack
      if (source) event.source = source
      this.queue.push({ pageLoadId: this.pageLoadId, page, key, event })
      this.schedule()
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
          this.options.send(JSON.stringify(batch))
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
      this.seen = new Set()
    }
  }

  private schedule(): void {
    if (this.timer !== null) return
    this.timer = this.options.setTimer(() => {
      this.timer = null
      this.flush()
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
  const event = { ...batch.events[0] }
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
 */
export function beaconSend(body: string): void {
  try {
    const blob = new Blob([body], { type: 'application/json' })
    if (typeof navigator.sendBeacon === 'function' && navigator.sendBeacon(ENDPOINT, blob)) return
  } catch {
    // Fall through to fetch.
  }
  try {
    void fetch(ENDPOINT, {
      method: 'POST',
      body,
      headers: { 'Content-Type': 'application/json' },
      keepalive: true,
      credentials: 'same-origin',
    }).catch(() => {})
  } catch {
    // Reporting is best-effort.
  }
}
