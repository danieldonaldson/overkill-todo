/**
 * Per-tab cache of the last user fetched from /api/v1/auth/me (sessionStorage).
 *
 * The storage layer under the reactive session store (`session.svelte.ts`),
 * which starts from it so a reload or a new page in the tab shows the last
 * known account at once; the server still checks every API call, and a 401
 * clears it. GuestGuard uses its presence to decide whether a session is worth
 * verifying. Dashboard components read the store, not this module.
 */

import type { User } from '../types'

const AUTH_SESSION_KEY = 'overkill-todo-auth-user'

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
 * Clear the cached user from session storage.
 * Called on logout or auth errors.
 */
export function clearSessionUser(): void {
  sessionStorage.removeItem(AUTH_SESSION_KEY)
}
