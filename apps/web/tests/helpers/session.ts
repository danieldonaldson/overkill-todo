import type { Page } from '@playwright/test'

// Mirrors src/lib/auth/token.ts TOKEN_EXPIRY_KEY and src/lib/auth/session.ts
// AUTH_SESSION_KEY. The session hint is what public/scripts/dashboard-session-hint.js
// checks before paint; without it every dashboard page redirects to /login.
export const TOKEN_EXPIRY_KEY = 'jwt_expiry'
export const AUTH_SESSION_KEY = 'overkill-todo-auth-user'

// Per-tab marker so the seed runs once per tab, not on every full load:
// addInitScript re-runs on each navigation and reload, which would otherwise
// undo a sign-out or a refresh that rewrote the hint.
const SEEDED_KEY = '__e2e_session_seeded'

export const TEST_USER = {
  id: 'test-user',
  email: 'test@example.com',
  first_name: '',
  last_name: '',
  email_verified: true,
  has_password: true,
  oauth_providers: [] as string[],
}

interface SeedOptions {
  /** Unix seconds for the stored JWT expiry. Defaults to an hour from now. */
  expiry?: number
  /** User to cache in sessionStorage, as an earlier visit in this tab would. */
  cachedUser?: object
}

/**
 * Makes the browser look signed in before any page script runs: sets the
 * session hint (and optionally the per-tab cached user) once per tab.
 * Call before the first page.goto.
 */
export async function seedSession(page: Page, options: SeedOptions = {}): Promise<void> {
  const expiry = options.expiry ?? Math.floor(Date.now() / 1000) + 3600
  await page.addInitScript(
    ({ expiry, cachedUser, keys }) => {
      try {
        if (sessionStorage.getItem(keys.seeded)) return
        sessionStorage.setItem(keys.seeded, '1')
        localStorage.setItem(keys.expiry, String(expiry))
        if (cachedUser) sessionStorage.setItem(keys.user, JSON.stringify(cachedUser))
      } catch {
        // about:blank and other opaque origins have no storage.
      }
    },
    {
      expiry,
      cachedUser: options.cachedUser ?? null,
      keys: { seeded: SEEDED_KEY, expiry: TOKEN_EXPIRY_KEY, user: AUTH_SESSION_KEY },
    }
  )
}
