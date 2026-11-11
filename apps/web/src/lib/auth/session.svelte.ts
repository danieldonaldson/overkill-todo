/**
 * The tab's single reactive record of the signed-in account (design D3).
 *
 * Module-level runes state, so every island on the page shares one instance.
 * It starts from the sessionStorage cache in `session.ts` (stale is fine) and
 * is confirmed in the background by one `/auth/me` per document
 * (`revalidate()`). Writes go through to the cache so a full reload starts
 * from the latest known account.
 *
 * Failure handling stays where it is: for a 401 or a 403 `email_not_verified`
 * the HttpClient has already cleared the cache and redirected, so the store
 * only handles "the server can't be reached" (network error or 5xx), which
 * leaves the Member on the page with `status = 'unreachable'`.
 */

import { api } from '../api'
import { ApiErrorException } from '../api/errors'
import { navigateReplace } from '../navigation'
import type { User } from '../types'
import { clearSessionUser, getSessionUser, setSessionUser } from './session'
import { clearTokenExpiry, getTokenExpiry } from './token'

export type SessionStatus = 'unconfirmed' | 'confirmed' | 'unreachable'

/** How long sign-out waits for the server before navigating anyway (D6). */
export const SIGN_OUT_TIMEOUT_MS = 1000

const state = $state<{ user: User | null; status: SessionStatus }>({
  user: getSessionUser(),
  status: 'unconfirmed',
})

/** The in-flight or settled `/auth/me` check for this document, if started. */
let check: Promise<void> | null = null
/** Set by `signOut()` so a late `/auth/me` answer can't restore the user. */
let signedOut = false

/** True for failures that mean "couldn't reach the server", not "refused". */
function isUnreachable(err: unknown): boolean {
  return err instanceof TypeError || (err instanceof ApiErrorException && (err.status ?? 0) >= 500)
}

async function runCheck(): Promise<void> {
  try {
    const user = await api.auth.me()
    if (signedOut) return
    state.user = user
    setSessionUser(user)
    state.status = 'confirmed'
    if (!user.email_verified) {
      navigateReplace(`/verify-email?email=${encodeURIComponent(user.email)}`)
    }
  } catch (err) {
    if (!signedOut && isUnreachable(err)) {
      state.status = 'unreachable'
    }
    // Anything else (401, 403 unverified): HttpClient has already redirected.
  }
}

export const session = {
  /** The signed-in account, or null while unknown (no cache, not yet checked). */
  get user(): User | null {
    return state.user
  },

  get status(): SessionStatus {
    return state.status
  },

  /**
   * Backstop for the head-script session-hint check (D2), which Astro doesn't
   * re-run on client-side navigation into the dashboard. With no session hint,
   * replaces the page with /login and returns false; otherwise starts the
   * background check and returns true. Call on mount of the dashboard header.
   */
  init(): boolean {
    if (typeof window === 'undefined') return false
    if (getTokenExpiry() === null) {
      navigateReplace('/login')
      return false
    }
    void session.revalidate()
    return true
  },

  /** Confirm the account with the server, at most once per document. */
  revalidate(): Promise<void> {
    check ??= runCheck()
    return check
  },

  /** Repeat the check (the "unreachable" notice's Retry button). */
  retry(): Promise<void> {
    check = null
    return session.revalidate()
  },

  /** Replace the account, e.g. with a server response after an action. */
  set(user: User): void {
    state.user = user
    setSessionUser(user)
  },

  /** Update fields of the known account. A no-op while the account is unknown. */
  patch(partial: Partial<User>): void {
    if (!state.user) return
    session.set({ ...state.user, ...partial })
  },

  /**
   * Sign out (D6): forget the account and the session hint locally first, give
   * the server up to {@link SIGN_OUT_TIMEOUT_MS} to end the session (the request
   * is `keepalive`, so it can still land after we leave), then go to /login.
   */
  async signOut(): Promise<void> {
    signedOut = true
    state.user = null
    clearSessionUser()
    clearTokenExpiry()

    let timer: ReturnType<typeof setTimeout> | undefined
    const timeout = new Promise<void>((resolve) => {
      timer = setTimeout(resolve, SIGN_OUT_TIMEOUT_MS)
    })
    const logout = api.auth.logout({ keepalive: true }).catch(() => {
      // The local session is already gone; the server's ends with its cookie.
    })
    await Promise.race([logout, timeout])
    clearTimeout(timer)

    // Clear again: a logout POST without a CSRF cookie first runs a token
    // refresh, which writes the session hint back. If the timeout won, that
    // hint would survive and the dashboard would open optimistically again.
    clearSessionUser()
    clearTokenExpiry()
    navigateReplace('/login')
  },
}

/** Reset to a fresh document's state, re-reading the cache. For tests. */
export function resetSession(): void {
  state.user = getSessionUser()
  state.status = 'unconfirmed'
  check = null
  signedOut = false
}
