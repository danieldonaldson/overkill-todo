import { describe, expect, it } from 'vitest'

import { loginNoticeFromSearch } from './login-notice'

describe('loginNoticeFromSearch', () => {
  it('returns null without a marker', () => {
    expect(loginNoticeFromSearch('')).toBeNull()
    expect(loginNoticeFromSearch('?foo=bar')).toBeNull()
  })

  it.each(['oauth_cancelled', 'oauth_failed', 'oauth_email_conflict', 'oauth_already_linked'])(
    'maps the %s error code to an error notice',
    (code) => {
      const notice = loginNoticeFromSearch(`?error=${code}`)
      expect(notice?.kind).toBe('error')
      expect(notice?.text).not.toContain(code)
    },
  )

  it('never echoes an unknown error value', () => {
    const notice = loginNoticeFromSearch('?error=Call%20555-0100%20to%20restore%20your%20account')
    expect(notice).toEqual({ kind: 'error', text: 'Something went wrong. Please try again.' })
  })

  it('shows info notices for verified email and expired session', () => {
    expect(loginNoticeFromSearch('?message=email_verified')?.kind).toBe('info')
    expect(loginNoticeFromSearch('?session=expired')?.kind).toBe('info')
  })

  it('prefers an error over other markers', () => {
    expect(loginNoticeFromSearch('?session=expired&error=oauth_failed')?.kind).toBe('error')
  })
})
