// Named analytics events (OT-005, design D5/D6). The union below is the spec's
// event table and nothing else: an unknown name, a missing property or a
// free-text value fails `astro check`, so user IDs, emails, form content and
// server messages can't be sent by accident.
//
// `track` is fire-and-forget. It does nothing when the tracker isn't loaded
// (analytics off, Do Not Track / Global Privacy Control, blocked, or still
// loading), never throws and never returns a promise, so no form waits on it.

import { ApiErrorException } from './api/errors'
import type { OAuthProvider } from './types'

export type SignupRejectedReason = 'invalid' | 'captcha' | 'rate-limited' | 'server'
// The spec's `unverified` reason is left out on purpose: the backend answers an
// unfinished signup exactly like a wrong password, so it can't be told apart
// (and mustn't be, or sign-in would reveal which emails have accounts).
export type SigninRejectedReason = 'credentials' | 'captcha' | 'rate-limited' | 'server'

export type AnalyticsEvent =
  | { name: 'signup-submitted' }
  | { name: 'signup-rejected'; props: { reason: SignupRejectedReason } }
  | { name: 'email-verified'; props: { via: 'code' | 'link' } }
  | { name: 'signin-succeeded' }
  | { name: 'signin-rejected'; props: { reason: SigninRejectedReason } }
  | { name: 'oauth-started'; props: { provider: OAuthProvider } }
  | { name: 'password-reset-requested' }
  | { name: 'password-reset-completed' }
  | { name: 'account-deletion-requested' }

interface UmamiTracker {
  track(name: string, data?: Record<string, string>): unknown
}

/** Record a named event, if analytics is running. Never throws or waits. */
export function track(event: AnalyticsEvent): void {
  try {
    const umami = (globalThis as { umami?: UmamiTracker }).umami
    if (!umami) return
    const sent = 'props' in event ? umami.track(event.name, event.props) : umami.track(event.name)
    // The tracker returns a promise; swallow its rejection so a failed send
    // never surfaces as an unhandled rejection.
    if (sent instanceof Promise) sent.catch(() => {})
  } catch {
    // Analytics must never break a form.
  }
}

type Outcome = 'invalid' | 'credentials' | 'captcha' | 'rate-limited' | 'server'

// One mapping from a failed API call to a fixed reason (D6). Only the status and
// the machine-readable code are read, never the message.
function outcomeOf(error: unknown): Outcome {
  if (!(error instanceof ApiErrorException) || error.status === undefined) return 'server'
  if (error.code === 'bot_check_failed') return 'captcha'
  if (error.status === 429) return 'rate-limited'
  if (error.status === 401) return 'credentials'
  if (error.status >= 500) return 'server'
  return 'invalid'
}

/** `reason` for `signup-rejected`: network errors and 5xx are `server`. */
export function signupRejectedReason(error: unknown): SignupRejectedReason {
  const outcome = outcomeOf(error)
  return outcome === 'credentials' ? 'invalid' : outcome
}

/** `reason` for `signin-rejected`: a 400 from the login form is bad credentials too. */
export function signinRejectedReason(error: unknown): SigninRejectedReason {
  const outcome = outcomeOf(error)
  return outcome === 'invalid' ? 'credentials' : outcome
}
