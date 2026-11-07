/**
 * Session storage for auth state persistence across View Transitions.
 *
 * This allows the AuthGuard to skip /api/v1/auth/me calls when navigating
 * between pages, since View Transitions preserve the browser tab but
 * still trigger component onMount.
 */

import type { User } from '../types'

const AUTH_SESSION_KEY = 'app-auth-user'

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
