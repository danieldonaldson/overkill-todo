// The hook between `HttpClient` and the browser reporter (OT-006 Decision 8).
//
// It has no imports, so `lib/api/client.ts` can call it without importing the
// reporter, which imports the session store, which imports the API client.
// `installTelemetry()` registers the handlers; until then every call is a
// no-op. Calls never throw.

/** One failed API call: a 5xx, or no answer at all. 4xx are never reported. */
export interface ApiFailure {
  /** HTTP method, upper case. */
  method: string
  /** The path template, e.g. `/admin/users/{id}/role`. Never a raw path. */
  route: string
  /** The response status, when the server answered. */
  status?: number
  /** The request never reached the server. */
  networkFailed?: boolean
  /** The response's `X-Trace-Id`, when the server answered. */
  traceId?: string | null
}

export interface ApiFailureHandlers {
  failure: (failure: ApiFailure) => void
  /** The server answered a request (any status): it's reachable again. */
  reachable: () => void
}

let handlers: ApiFailureHandlers | null = null

export function setApiFailureHandlers(next: ApiFailureHandlers | null): void {
  handlers = next
}

export function reportApiFailure(failure: ApiFailure): void {
  try {
    handlers?.failure(failure)
  } catch {
    // Reporting never affects the call.
  }
}

export function reportApiReachable(): void {
  try {
    handlers?.reachable()
  } catch {
    // Reporting never affects the call.
  }
}

const UUID_SEGMENT = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i
const NUMERIC_SEGMENT = /^\d+$/

/**
 * The route template for an endpoint when the caller gave none: the path
 * without its query, with ID-like segments as `{id}`. A backstop so a raw ID
 * is never reported; callers with path parameters pass their own template.
 */
export function routeTemplate(endpoint: string): string {
  const path = endpoint.split(/[?#]/, 1)[0]
  return path
    .split('/')
    .map((segment) =>
      UUID_SEGMENT.test(segment) || NUMERIC_SEGMENT.test(segment) ? '{id}' : segment
    )
    .join('/')
}
