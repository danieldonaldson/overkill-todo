// Browser error reporting entry point (OT-006 Decision 8), loaded on every page
// from `Base.astro` as a bundled module script, so it runs after parse and never
// blocks paint. Errors thrown before it loads aren't captured; islands hydrate
// after it.

import { getTokenExpiry } from '../auth/token'
import { type ApiFailure, setApiFailureHandlers } from './api-failures'
import { type CapturedError, Reporter, beaconSend } from './reporter'

let reporter: Reporter | null = null

/** The build's release: the commit SHA in release images, `dev` otherwise. */
export const RELEASE: string = import.meta.env.PUBLIC_RELEASE || 'dev'

function sourceOf(filename: string | undefined, hasError: boolean): string | undefined {
  // `Script error.` with no error object: a cross-origin script.
  if (!filename) return hasError ? undefined : 'external'
  try {
    return new URL(filename, location.href).origin === location.origin ? filename : 'external'
  } catch {
    return 'external'
  }
}

function describe(value: unknown): { message: string; stack?: string } {
  if (value instanceof Error) {
    return { message: `${value.name}: ${value.message}`, stack: value.stack }
  }
  try {
    return { message: typeof value === 'string' ? value : `Non-error value: ${String(value)}` }
  } catch {
    return { message: 'Non-error value' }
  }
}

export function fromErrorEvent(event: ErrorEvent): CapturedError {
  const described = event.error === undefined || event.error === null ? null : describe(event.error)
  return {
    kind: 'error',
    message: described?.message ?? (event.message || 'Script error.'),
    stack: described?.stack,
    source: sourceOf(event.filename, described !== null),
  }
}

export function fromRejection(event: PromiseRejectionEvent): CapturedError {
  return { kind: 'unhandledrejection', ...describe(event.reason) }
}

/**
 * Report an error a `TelemetryBoundary` caught (`kind: boundary`). A no-op
 * until `installTelemetry()` has run, and it never throws.
 */
export function reportBoundaryError(island: string, error: unknown): void {
  try {
    reporter?.capture({ kind: 'boundary', island, ...describe(error) })
  } catch {
    // Never let reporting break the page.
  }
}

/** An API failure as a captured `kind: api` error. */
export function fromApiFailure(failure: ApiFailure): CapturedError {
  const outcome = failure.networkFailed ? 'network error' : `status ${failure.status ?? 'unknown'}`
  return {
    kind: 'api',
    message: `${failure.method} ${failure.route} failed: ${outcome}`,
    apiRoute: failure.route,
    status: failure.networkFailed ? undefined : failure.status,
    networkFailed: failure.networkFailed || undefined,
    traceId: failure.networkFailed ? undefined : (failure.traceId ?? undefined),
  }
}

/**
 * Call `count` for the full load now, then once per in-site navigation when its
 * new page has loaded. A navigation counts only after `astro:after-swap`, so
 * the first `astro:page-load` (the full load, already counted) is never counted
 * twice, whichever script runs first.
 */
export function watchPageLoads(target: EventTarget, count: () => void): void {
  count()
  let swapped = false
  target.addEventListener('astro:after-swap', () => {
    swapped = true
  })
  target.addEventListener('astro:page-load', () => {
    if (!swapped) return
    swapped = false
    count()
  })
}

/** Start reporting for this document. Safe to call more than once. */
export function installTelemetry(): Reporter | null {
  if (reporter || typeof window === 'undefined') return reporter
  try {
    const r = new Reporter({
      release: RELEASE,
      pathname: () => location.pathname,
      // The session hint, not the session store: signing in sets it and
      // signing out (or an expired session) clears it, while the store's
      // background `/auth/me` check filling in the user changes nothing.
      isSignedIn: () => getTokenExpiry() !== null,
      send: beaconSend,
    })
    window.addEventListener('error', (e) => {
      try {
        r.capture(fromErrorEvent(e))
      } catch {
        // Never let reporting break the page.
      }
    })
    window.addEventListener('unhandledrejection', (e) => {
      try {
        r.capture(fromRejection(e))
      } catch {
        // Never let reporting break the page.
      }
    })
    watchPageLoads(document, () => r.pageLoad())
    setApiFailureHandlers({
      failure: (failure) => {
        if (failure.networkFailed) r.markOffline()
        r.capture(fromApiFailure(failure))
      },
      reachable: () => r.markOnline(),
    })
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'hidden') r.flush()
    })
    window.addEventListener('pagehide', () => r.flush())
    reporter = r
  } catch {
    // Reporting is optional; the page goes on without it.
  }
  return reporter
}
