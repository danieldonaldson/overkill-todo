// Typed shape for backend error response bodies.
//
// The canonical DTO is `ErrorResponse { code, message }` (backend
// `shared::error.rs`). Some auth endpoints return a richer structured payload
// (`error` / `error_code` plus context fields like `attempts_remaining`),
// which is why both naming styles appear here. Callers narrow with the
// `isApiError` / `getErrorCode` helpers rather than probing `Record<string,
// unknown>` ad hoc.

export interface ApiError {
  /** Canonical error code (`ErrorResponse.code`). */
  code?: string
  /** Canonical human-readable message (`ErrorResponse.message`). */
  message?: string
  /** Structured auth-flow code (e.g. `CODE_INVALID`, `CODE_LOCKED`). */
  error_code?: string
  /** Alternate message field used by some auth handlers. */
  error?: string
  /** Remaining attempts before lockout (code/password flows). */
  attempts_remaining?: number
  /** Cooldown before a resend is allowed, in seconds. */
  cooldown_seconds?: number
  /** Email associated with the failed flow, when echoed back. */
  email?: string
}

/**
 * Narrow an unknown thrown value to an {@link ApiError}-shaped object.
 *
 * Requires the value to be an object carrying at least one recognized error
 * field of the correct primitive type, so an unrelated object (or `{}`) does
 * NOT pass and mask a malformed response.
 */
export function isApiError(value: unknown): value is ApiError {
  if (typeof value !== 'object' || value === null) return false
  const v = value as Record<string, unknown>
  const hasString = (k: string) => typeof v[k] === 'string'
  const hasNumber = (k: string) => typeof v[k] === 'number'
  return (
    hasString('code') ||
    hasString('message') ||
    hasString('error_code') ||
    hasString('error') ||
    hasString('email') ||
    hasNumber('attempts_remaining') ||
    hasNumber('cooldown_seconds')
  )
}

/**
 * Read the effective error code from a parsed error body, preferring the
 * canonical `code` and falling back to the structured-auth `error_code`.
 */
export function getErrorCode(body: ApiError | null | undefined): string | undefined {
  return body?.code ?? body?.error_code
}

/** Read the best available human-readable message from an error body. */
export function getErrorMessage(body: ApiError | null | undefined): string | undefined {
  return body?.message ?? body?.error
}

/**
 * Error thrown by `HttpClient` for every non-OK response.
 *
 * Being a real {@link Error} means `instanceof Error` callers get a usable
 * `.message`, while the parsed body's fields are copied onto the instance so
 * `'error_code' in err` narrowing (and `err.attempts_remaining`, etc.) keeps
 * working for the auth flows that inspect them. `status` is the HTTP status;
 * narrow on it with {@link isApiStatus}.
 */
export class ApiErrorException extends Error implements ApiError {
  /** HTTP status of the response, when the error came from one. */
  status?: number
  code?: string
  error_code?: string
  error?: string
  attempts_remaining?: number
  cooldown_seconds?: number
  email?: string

  /**
   * @param body Parsed response body, or null when it had none (or wasn't JSON).
   * @param options.status HTTP status of the response.
   * @param options.message Overrides the message derived from the body.
   */
  constructor(body: ApiError | null, options: { status?: number; message?: string } = {}) {
    const message = options.message ?? getErrorMessage(body) ?? 'Request failed'
    super(message)
    this.name = 'ApiErrorException'
    if (body) Object.assign(this, body)
    // After the body, whose own `message` / `status` fields would shadow these.
    this.message = message
    this.status = options.status
  }
}

/** True when `err` is an {@link ApiErrorException} for an HTTP `status` response. */
export function isApiStatus(err: unknown, status: number): boolean {
  return err instanceof ApiErrorException && err.status === status
}
