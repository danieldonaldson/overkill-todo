/**
 * Carries the signup token from email verification to /set-password.
 *
 * The token lets its holder set the account's password, so it must not travel
 * in a URL (browser history, proxy and access logs, shared screenshots).
 * sessionStorage is per-tab and gone when the tab closes, which matches the
 * token's short (30 min) life.
 */
const STORAGE_KEY = 'signup_handoff'

export interface SignupHandoff {
  token: string
  email: string
}

export function saveSignupHandoff(handoff: SignupHandoff): void {
  sessionStorage.setItem(STORAGE_KEY, JSON.stringify(handoff))
}

export function readSignupHandoff(): SignupHandoff | null {
  if (typeof sessionStorage === 'undefined') return null
  try {
    const raw = sessionStorage.getItem(STORAGE_KEY)
    if (!raw) return null
    const parsed: unknown = JSON.parse(raw)
    if (
      parsed &&
      typeof parsed === 'object' &&
      typeof (parsed as SignupHandoff).token === 'string' &&
      typeof (parsed as SignupHandoff).email === 'string'
    ) {
      return parsed as SignupHandoff
    }
  } catch {
    // Corrupt or inaccessible storage: treat as no handoff.
  }
  return null
}

export function clearSignupHandoff(): void {
  if (typeof sessionStorage !== 'undefined') {
    sessionStorage.removeItem(STORAGE_KEY)
  }
}
