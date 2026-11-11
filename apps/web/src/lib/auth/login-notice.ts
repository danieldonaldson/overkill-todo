/**
 * Messages the login page shows for the markers other flows put in its URL
 * (`/login?error=…`, `?message=…`, `?session=…`).
 *
 * Only known codes map to text. Anything else in `error` gets a generic
 * message, never the raw value: rendering query-string text would let anyone
 * craft a login link that shows a message of their choosing.
 */
export interface LoginNotice {
  kind: 'error' | 'info'
  text: string
}

const ERROR_MESSAGES: Record<string, string> = {
  oauth_cancelled: 'Sign-in was cancelled. You can try again or use your email and password.',
  oauth_failed: 'We couldn’t sign you in with that provider. Please try again.',
  oauth_email_conflict:
    'An account with this email already exists. Sign in with your password, then link the provider from Settings.',
  oauth_already_linked: 'That provider account is already linked to a different user.',
}

const GENERIC_ERROR = 'Something went wrong. Please try again.'

export function loginNoticeFromSearch(search: string): LoginNotice | null {
  const params = new URLSearchParams(search)

  const error = params.get('error')
  if (error) {
    return { kind: 'error', text: ERROR_MESSAGES[error] ?? GENERIC_ERROR }
  }
  if (params.get('message') === 'email_verified') {
    return { kind: 'info', text: 'Your email is verified. Sign in to continue.' }
  }
  if (params.get('session') === 'expired') {
    return { kind: 'info', text: 'Your session has expired. Please sign in again.' }
  }
  return null
}
