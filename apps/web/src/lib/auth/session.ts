/**
 * Per-tab cache of the last user fetched from /api/v1/auth/me (sessionStorage).
 *
 * The storage layer under the reactive session store (`session.svelte.ts`),
 * which starts from it so a reload or a new page in the tab shows the last
 * known account at once; the server still checks every API call, and a 401
 * clears it. A user confirmed by /auth/me stays trusted, with no new check,
 * until the access token it was confirmed under expires. GuestGuard uses its presence to decide whether a session is worth
 * verifying. Dashboard components read the store, not this module.
 */

import type { User } from '../types'
import { getTokenExpiry } from './token'

const AUTH_SESSION_KEY = 'overkill-todo-auth-user'
/**
 * Unix seconds until which the cached user counts as confirmed: the access
 * token's expiry when /auth/me last answered. Kept beside the user, not in it,
 * so writes after actions (`setSessionUser`) leave it alone.
 */
const CONFIRMED_UNTIL_KEY = 'overkill-todo-auth-user-confirmed-until'

/**
 * Get the cached user from session storage.
 * Returns null if no user is cached or if parsing fails.
 */
export function getSessionUser(): User | null {
  try {
    const data = sessionStorage.getItem(AUTH_SESSION_KEY)
    return data ? JSON.parse(data) : null
  } catch {
    return null
  }
}

/**
 * Cache the user in session storage.
 * Called after a successful /api/v1/auth/me fetch.
 */
export function setSessionUser(user: User): void {
  sessionStorage.setItem(AUTH_SESSION_KEY, JSON.stringify(user))
}

/**
 * Cache a user the server just returned from /auth/me, and treat it as
 * confirmed until the current access token expires.
 */
export function confirmSessionUser(user: User): void {
  setSessionUser(user)
  const expiry = getTokenExpiry()
  if (expiry === null) {
    sessionStorage.removeItem(CONFIRMED_UNTIL_KEY)
  } else {
    sessionStorage.setItem(CONFIRMED_UNTIL_KEY, String(expiry))
  }
}

/**
 * Was the cached user confirmed by /auth/me under an access token that has not
 * expired yet? Also false once the tab has lost its session hint.
 */
export function isSessionUserConfirmed(): boolean {
  if (getSessionUser() === null || getTokenExpiry() === null) return false
  const until = Number(sessionStorage.getItem(CONFIRMED_UNTIL_KEY))
  return Number.isFinite(until) && Math.floor(Date.now() / 1000) < until
}

/**
 * Clear the cached user from session storage.
 * Called on logout or auth errors.
 */
export function clearSessionUser(): void {
  sessionStorage.removeItem(AUTH_SESSION_KEY)
  sessionStorage.removeItem(CONFIRMED_UNTIL_KEY)
}
