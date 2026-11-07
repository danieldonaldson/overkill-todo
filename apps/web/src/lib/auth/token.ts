/**
 * Token manager for handling JWT expiry and proactive refresh.
 *
 * The JWT itself is stored in an HttpOnly cookie (not accessible via JS),
 * but we track the expiry time to know when to proactively refresh.
 */

const TOKEN_EXPIRY_KEY = 'jwt_expiry'
// Monotonic counter (shared across tabs via localStorage) bumped on every
// successful refresh. Used to detect "a peer already refreshed" reliably — unlike
// the expiry timestamp, which collides for fixed-TTL tokens refreshed in the same
// second.
const REFRESH_GENERATION_KEY = 'jwt_refresh_generation'
const REFRESH_THRESHOLD_SECONDS = 60 // Refresh when less than 60 seconds remaining

// Web Lock name used to serialize refreshes across all same-origin tabs.
const REFRESH_LOCK_NAME = 'app-token-refresh'

// In-tab single-flight: collapse concurrent refresh callers in this tab onto
// one promise. Cross-tab coordination is handled separately via the Web Lock.
let isRefreshing = false
let refreshPromise: Promise<boolean> | null = null
// Abort handle for the in-flight refresh, so it can be cancelled even after the
// Web Lock has been acquired (resetTokenManagerState uses this).
let refreshAbort: AbortController | null = null

/**
 * Stores the JWT expiry timestamp.
 * Call this after login/register/refresh to track when the token expires.
 *
 * @param expiryTimestamp Unix timestamp in seconds when the JWT expires
 */
export function setTokenExpiry(expiryTimestamp: number): void {
  if (typeof localStorage === 'undefined') return

  try {
    localStorage.setItem(TOKEN_EXPIRY_KEY, expiryTimestamp.toString())
  } catch {
    // localStorage might be full or disabled
  }
}

/**
 * Gets the stored JWT expiry timestamp.
 *
 * @returns The expiry timestamp in seconds, or null if not set
 */
export function getTokenExpiry(): number | null {
  if (typeof localStorage === 'undefined') return null

  const stored = localStorage.getItem(TOKEN_EXPIRY_KEY)
  if (!stored) return null

  const expiry = parseInt(stored, 10)
  return isNaN(expiry) ? null : expiry
}

/**
 * Clears the stored token expiry.
 * Call this on logout or when the session is invalidated.
 */
export function clearTokenExpiry(): void {
  if (typeof localStorage === 'undefined') return
  localStorage.removeItem(TOKEN_EXPIRY_KEY)
}

/**
 * Reads the current refresh generation counter (0 if unset/unavailable).
 */
function getRefreshGeneration(): number {
  if (typeof localStorage === 'undefined') return 0
  const stored = localStorage.getItem(REFRESH_GENERATION_KEY)
  const value = stored === null ? 0 : parseInt(stored, 10)
  return isNaN(value) ? 0 : value
}

/**
 * Increments the refresh generation counter so other tabs can detect that a
 * refresh just completed. Called after a successful rotation.
 */
function bumpRefreshGeneration(): void {
  if (typeof localStorage === 'undefined') return
  try {
    localStorage.setItem(REFRESH_GENERATION_KEY, (getRefreshGeneration() + 1).toString())
  } catch {
    // localStorage might be full or disabled
  }
}

/**
 * Checks if the JWT has expired.
 */
export function isTokenExpired(): boolean {
  const expiry = getTokenExpiry()
  if (expiry === null) return true

  const now = Math.floor(Date.now() / 1000)
  return now >= expiry
}

/**
 * Checks if the JWT is about to expire (within REFRESH_THRESHOLD_SECONDS).
 */
export function isTokenExpiringSoon(): boolean {
  const expiry = getTokenExpiry()
  if (expiry === null) return false

  const now = Math.floor(Date.now() / 1000)
  return expiry - now <= REFRESH_THRESHOLD_SECONDS
}

/**
 * Gets the remaining time until the JWT expires.
 *
 * @returns Seconds until expiry, or 0 if expired/unknown
 */
export function getTimeUntilExpiry(): number {
  const expiry = getTokenExpiry()
  if (expiry === null) return 0

  const now = Math.floor(Date.now() / 1000)
  return Math.max(0, expiry - now)
}

/**
 * Coordinates a token refresh so that at most one rotation happens at a time,
 * both within this tab and across every same-origin tab.
 *
 * Refresh tokens rotate on every use, and the backend treats a second
 * presentation of an already-rotated token as a stolen-token signal and
 * revokes ALL of the user's sessions. So two concurrent refreshes of the same
 * cookie — whether from parallel requests in one tab or from separate tabs —
 * would log the user out everywhere. We prevent that with:
 *   1. an in-tab single-flight (shared promise), and
 *   2. a cross-tab Web Lock that serializes refreshes between tabs.
 *
 * Inside the lock we re-check the stored expiry: if it advanced to a still-valid
 * token while we waited, another party already refreshed and we reuse their
 * result instead of rotating the cookie a second time.
 *
 * @param refreshFn Function that performs the network refresh. Returns true on success.
 * @returns true if the token is valid (or was refreshed), false if refresh failed
 */
export async function coordinateRefresh(refreshFn: () => Promise<boolean>): Promise<boolean> {
  // If this tab is already refreshing, wait for that to complete.
  if (isRefreshing && refreshPromise) {
    return refreshPromise
  }

  isRefreshing = true
  const controller = new AbortController()
  refreshAbort = controller
  refreshPromise = (async () => {
    try {
      return await withRefreshLock(refreshFn, controller.signal)
    } catch {
      // Aborted (e.g. resetTokenManagerState) or the lock request rejected —
      // treat as a failed refresh rather than letting the rejection escape.
      return false
    } finally {
      isRefreshing = false
      refreshPromise = null
      if (refreshAbort === controller) refreshAbort = null
    }
  })()

  return refreshPromise
}

/** Rejects when `signal` aborts, otherwise settles with `promise`. */
function abortable<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  if (signal.aborted) {
    return Promise.reject(new DOMException('Refresh aborted', 'AbortError'))
  }
  return new Promise<T>((resolve, reject) => {
    const onAbort = () => reject(new DOMException('Refresh aborted', 'AbortError'))
    signal.addEventListener('abort', onAbort, { once: true })
    promise.then(
      (value) => {
        signal.removeEventListener('abort', onAbort)
        resolve(value)
      },
      (error) => {
        signal.removeEventListener('abort', onAbort)
        reject(error)
      },
    )
  })
}

async function withRefreshLock(
  refreshFn: () => Promise<boolean>,
  signal: AbortSignal,
): Promise<boolean> {
  const generationBeforeLock = getRefreshGeneration()

  const run = async (): Promise<boolean> => {
    // Another tab may have refreshed while we waited for the lock. Detect this
    // via the generation counter (not the expiry timestamp, which collides for
    // fixed-TTL tokens refreshed in the same second). If a peer bumped the
    // generation and the token is now valid, reuse their result rather than
    // rotating the cookie again (which would trip stolen-token detection).
    const generationAfterLock = getRefreshGeneration()
    if (generationAfterLock !== generationBeforeLock && !isTokenExpiringSoon()) {
      return true
    }
    // Race against the abort signal so an aborted refresh releases the lock
    // instead of holding it until the network call settles.
    const ok = await abortable(refreshFn(), signal)
    if (ok) {
      // Publish the successful rotation so peers waiting on the lock skip theirs.
      bumpRefreshGeneration()
    }
    return ok
  }

  // Web Locks serialize the refresh across same-origin tabs. The signal also
  // cancels a still-pending acquisition. Older browsers without the API fall
  // back to the in-tab single-flight only.
  if (typeof navigator !== 'undefined' && navigator.locks) {
    return navigator.locks.request(REFRESH_LOCK_NAME, { signal }, run)
  }

  return run()
}

/**
 * Ensures the token is valid by refreshing if necessary.
 *
 * Call this before making authenticated API requests.
 * It will automatically refresh the token if it's expiring soon, routing
 * through {@link coordinateRefresh} so concurrent callers and other tabs share
 * a single rotation.
 *
 * @param refreshFn Function to call to refresh the token. Should return true on success.
 * @returns true if the token is valid (or was refreshed), false if refresh failed
 */
export async function ensureValidToken(refreshFn: () => Promise<boolean>): Promise<boolean> {
  // If token is not expiring soon, no action needed
  if (!isTokenExpiringSoon()) {
    return true
  }

  return coordinateRefresh(refreshFn)
}

/**
 * Resets the internal state.
 * Useful for testing.
 *
 * Aborts any in-flight refresh so a wedged or never-settling refresh releases
 * the cross-tab Web Lock instead of blocking the next caller.
 */
export function resetTokenManagerState(): void {
  refreshAbort?.abort()
  refreshAbort = null
  isRefreshing = false
  refreshPromise = null
}
