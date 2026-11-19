// Browser error reporting entry point (OT-006 Decision 8), loaded on every page
// from `Base.astro` as a bundled module script, so it runs after parse and never
// blocks paint. Errors thrown before it loads aren't captured; islands hydrate
// after it.

import { session } from '../auth/session.svelte'
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

/** Start reporting for this document. Safe to call more than once. */
export function installTelemetry(): Reporter | null {
  if (reporter || typeof window === 'undefined') return reporter
  try {
    const r = new Reporter({
      release: RELEASE,
      pathname: () => location.pathname,
      isSignedIn: () => session.user !== null,
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
