/**
 * The user AuthGuard fetched from /auth/me on this page load.
 *
 * Dashboard panels are separate Astro islands nested in DashboardApp's slot,
 * so they can't receive the guard's user as a prop or through Svelte context.
 * They share this module instead. The guard only renders its slot (and so only
 * hydrates the panel islands) after setting the user here, so a panel can read
 * it synchronously on mount instead of calling /auth/me a second time. That
 * ordering is also why this needs no reactivity.
 */

import type { User } from '../types'

let currentUser: User | null = null

/** The authenticated user for this page load, or null before/without one. */
export function getCurrentUser(): User | null {
  return currentUser
}

/** Called by AuthGuard after /auth/me resolves (or with null when it fails). */
export function setCurrentUser(user: User | null): void {
  currentUser = user
}
