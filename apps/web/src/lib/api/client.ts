import type { RefreshResponse } from '../types'
import { redirect } from '../navigation'
import { clearSessionUser } from '../auth/session'
import { getDeviceFingerprint } from '../auth/fingerprint'
import {
  setTokenExpiry,
  clearTokenExpiry,
  getTokenExpiry,
  ensureValidToken,
  coordinateRefresh,
} from '../auth/token'
import { API_BASE } from '../config'
import type { ApiError } from './errors'
import { ApiErrorException, getErrorCode, getErrorMessage } from './errors'

/**
 * Get the API base URL for direct fetch calls.
 * Returns empty string since we use relative paths (/api/v1).
 * This allows the browser to construct the full URL automatically.
 */
export function getApiBaseUrl(): string {
  return ''
}

/** Header name for trace ID returned by server */
const X_TRACE_ID_HEADER = 'x-trace-id'

/** Header name for CSRF token */
const X_CSRF_TOKEN_HEADER = 'x-csrf-token'

/** Cookie name for CSRF token */
const CSRF_TOKEN_COOKIE = 'csrf_token'

/**
 * Last trace ID received from the server.
 * Useful for displaying in error messages for user support.
 */
let lastTraceId: string | null = null

/**
 * Get the last trace ID received from the server.
 * Can be shown to users in error messages for support purposes.
 */
export function getLastTraceId(): string | null {
  return lastTraceId
}

/**
 * Read the CSRF token from the csrf_token cookie.
 * Returns null if the cookie is not present.
 */
function getCsrfToken(): string | null {
  const match = document.cookie.match(new RegExp(`(?:^|;\\s*)${CSRF_TOKEN_COOKIE}=([^;]*)`))
  return match ? match[1] : null
}

export class HttpClient {
  /**
   * Attempts to refresh the access token.
   * Returns true if successful, false otherwise.
   */
  private async refreshToken(): Promise<boolean> {
    try {
      const fingerprint = await getDeviceFingerprint()

      const response = await fetch(`${API_BASE}/auth/refresh`, {
        method: 'POST',
        credentials: 'include',
        headers: {
          'Content-Type': 'application/json',
          'X-Device-Fingerprint': fingerprint,
        },
      })

      // Capture trace ID from response for error display
      const traceId = response.headers.get(X_TRACE_ID_HEADER)
      if (traceId) {
        lastTraceId = traceId
      }

      if (!response.ok) {
        return false
      }

      const data: RefreshResponse = await response.json()
      setTokenExpiry(data.expires_at)
      return true
    } catch {
      return false
    }
  }

  async request<T>(endpoint: string, options: RequestInit = {}, isRetry = false): Promise<T> {
    // For non-auth endpoints, ensure we have a valid token
    const isAuthEndpoint = endpoint.startsWith('/auth/')
    if (!isAuthEndpoint) {
      const hadSession = getTokenExpiry() !== null
      const tokenValid = await ensureValidToken(() => this.refreshToken())
      if (!tokenValid) {
        clearTokenExpiry()
        clearSessionUser()
        redirect(hadSession ? '/login?session=expired' : '/login')
        throw new Error('Session expired')
      }
    }

    // Build headers with CSRF token for state-changing methods
    const method = options.method?.toUpperCase() || 'GET'
    const needsCsrf = ['POST', 'PUT', 'PATCH', 'DELETE'].includes(method)
    let csrfToken = needsCsrf ? getCsrfToken() : null

    // A mutating request with no CSRF cookie (e.g. it expired) would be rejected
    // by the server with a guaranteed 403. Proactively refresh to mint a fresh
    // CSRF cookie instead of firing a request we know will fail. Only on the
    // initial attempt; the 403 csrf_invalid path below remains the fallback.
    if (needsCsrf && !csrfToken && !isRetry) {
      const refreshed = await coordinateRefresh(() => this.refreshToken())
      if (refreshed) {
        csrfToken = getCsrfToken()
      }
    }

    const headers: Record<string, string> = {
      'Content-Type': 'application/json',
      ...(options.headers as Record<string, string>),
    }

    if (csrfToken) {
      headers[X_CSRF_TOKEN_HEADER] = csrfToken
    }

    const response = await fetch(`${API_BASE}${endpoint}`, {
      ...options,
      credentials: 'include',
      headers,
    })

    // Capture trace ID from response for error display
    const traceId = response.headers.get(X_TRACE_ID_HEADER)
    if (traceId) {
      lastTraceId = traceId
    }

    // Handle 401 - try to refresh token before giving up
    const isLoginOrRegister =
      endpoint.startsWith('/auth/login') || endpoint.startsWith('/auth/register')
    if (response.status === 401 && !isLoginOrRegister && !isRetry) {
      // Attempt to refresh the token. Route through coordinateRefresh so
      // parallel 401s (and other tabs) share one rotation instead of each
      // presenting the same refresh cookie and tripping reuse detection.
      const hadSession = getTokenExpiry() !== null
      const refreshed = await coordinateRefresh(() => this.refreshToken())
      if (refreshed) {
        // Retry the original request with the new token
        return this.request<T>(endpoint, options, true)
      }
      // Refresh failed - session is truly expired
      clearTokenExpiry()
      clearSessionUser()
      redirect(hadSession ? '/login?session=expired' : '/login')
      throw new Error('Session expired')
    }

    // Handle 403 with CSRF error - attempt refresh and retry once
    if (response.status === 403 && !isRetry && needsCsrf) {
      try {
        const errorBody: ApiError = await response.clone().json()
        if (getErrorCode(errorBody) === 'csrf_invalid') {
          // Refresh to get a new CSRF token (single-flighted across tabs)
          const refreshed = await coordinateRefresh(() => this.refreshToken())
          if (refreshed) {
            // Retry the original request with the new CSRF token
            return this.request<T>(endpoint, options, true)
          }
        }
      } catch {
        // If we can't parse the error, continue with normal error handling
      }
    }

    // Handle 403 with email_not_verified - redirect to verification page
    if (response.status === 403) {
      try {
        const errorBody: ApiError = await response.clone().json()
        if (getErrorCode(errorBody) === 'email_not_verified') {
          redirect('/verify-email')
          throw new Error('Email not verified')
        }
      } catch (e) {
        // If this was our own "Email not verified" error, re-throw it
        if (e instanceof Error && e.message === 'Email not verified') {
          throw e
        }
        // Otherwise, continue with normal error handling
      }
    }

    if (!response.ok) {
      let errorBody: ApiError | null = null
      try {
        errorBody = await response.json()
      } catch {
        // Could not parse JSON
      }

      const errorMessage =
        getErrorMessage(errorBody) ?? `Request failed with status ${response.status}`

      // If the error has error_code (structured API error), throw a typed Error
      // that still carries error_code, attempts_remaining, etc. so callers can
      // inspect them while `instanceof Error` checks keep working.
      if (errorBody && 'error_code' in errorBody) {
        throw new ApiErrorException(errorBody)
      }

      // Otherwise, throw a plain Error with the message
      const errorWithTrace = lastTraceId
        ? `${errorMessage} (trace: ${lastTraceId})`
        : String(errorMessage)
      throw new Error(errorWithTrace)
    }

    // Handle 204 No Content
    if (response.status === 204) {
      return undefined as T
    }

    return response.json()
  }

  /**
   * Makes a request with the device fingerprint header.
   * Used for auth endpoints that require fingerprint.
   */
  async requestWithFingerprint<T>(endpoint: string, options: RequestInit = {}): Promise<T> {
    const fingerprint = await getDeviceFingerprint()
    return this.request<T>(endpoint, {
      ...options,
      headers: {
        ...options.headers,
        'X-Device-Fingerprint': fingerprint,
      },
    })
  }
}
