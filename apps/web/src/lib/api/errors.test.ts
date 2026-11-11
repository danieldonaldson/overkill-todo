import { describe, expect, it } from 'vitest'

import { ApiErrorException, getErrorCode, getErrorMessage, isApiError, isApiStatus } from './errors'

describe('isApiError', () => {
  it('accepts objects carrying a recognized string field', () => {
    expect(isApiError({ code: 'RATE_LIMITED' })).toBe(true)
    expect(isApiError({ message: 'nope' })).toBe(true)
    expect(isApiError({ error_code: 'CODE_INVALID' })).toBe(true)
    expect(isApiError({ error: 'bad' })).toBe(true)
    expect(isApiError({ email: 'a@b.com' })).toBe(true)
  })

  it('accepts objects carrying a recognized number field', () => {
    expect(isApiError({ attempts_remaining: 2 })).toBe(true)
    expect(isApiError({ cooldown_seconds: 30 })).toBe(true)
  })

  it('rejects non-objects and null', () => {
    expect(isApiError(null)).toBe(false)
    expect(isApiError(undefined)).toBe(false)
    expect(isApiError('error')).toBe(false)
    expect(isApiError(42)).toBe(false)
  })

  it('rejects an empty object or one with only unrelated/wrong-typed fields', () => {
    expect(isApiError({})).toBe(false)
    expect(isApiError({ foo: 'bar' })).toBe(false)
    // recognized key but wrong primitive type must not pass
    expect(isApiError({ code: 123 })).toBe(false)
    expect(isApiError({ attempts_remaining: '2' })).toBe(false)
  })
})

describe('getErrorCode', () => {
  it('prefers the canonical code over the structured error_code', () => {
    expect(getErrorCode({ code: 'CANON', error_code: 'STRUCT' })).toBe('CANON')
  })

  it('falls back to error_code when code is absent', () => {
    expect(getErrorCode({ error_code: 'STRUCT' })).toBe('STRUCT')
  })

  it('returns undefined for null/undefined or a code-less body', () => {
    expect(getErrorCode(null)).toBeUndefined()
    expect(getErrorCode(undefined)).toBeUndefined()
    expect(getErrorCode({ message: 'no code here' })).toBeUndefined()
  })
})

describe('getErrorMessage', () => {
  it('prefers the canonical message over the alternate error field', () => {
    expect(getErrorMessage({ message: 'canonical', error: 'alt' })).toBe('canonical')
  })

  it('falls back to the alternate error field', () => {
    expect(getErrorMessage({ error: 'alt' })).toBe('alt')
  })

  it('returns undefined when no message is present', () => {
    expect(getErrorMessage(null)).toBeUndefined()
    expect(getErrorMessage({ code: 'X' })).toBeUndefined()
  })
})

describe('ApiErrorException', () => {
  it('is a real Error with the resolved message and copied structured fields', () => {
    const err = new ApiErrorException({
      error: 'Too many attempts',
      error_code: 'LOCKED',
      attempts_remaining: 0,
      email: 'a@b.com',
    })

    expect(err).toBeInstanceOf(Error)
    expect(err.name).toBe('ApiErrorException')
    expect(err.message).toBe('Too many attempts')
    expect(err.error_code).toBe('LOCKED')
    expect(err.attempts_remaining).toBe(0)
    expect(err.email).toBe('a@b.com')
    expect('error_code' in err).toBe(true)
  })

  it('falls back to a generic message when the body has none', () => {
    const err = new ApiErrorException({ code: 'X' })
    expect(err.message).toBe('Request failed')
  })
})

describe('ApiErrorException status', () => {
  it('carries the status and an explicit message', () => {
    const err = new ApiErrorException(
      { code: 'forbidden', message: 'Admins only' },
      { status: 403, message: 'Admins only (trace: t-1)' }
    )
    expect(err.status).toBe(403)
    expect(err.code).toBe('forbidden')
    expect(err.message).toBe('Admins only (trace: t-1)')
  })

  it('accepts a missing body', () => {
    const err = new ApiErrorException(null, { status: 500 })
    expect(err.status).toBe(500)
    expect(err.message).toBe('Request failed')
  })

  it('keeps the real status even if the body has a status field', () => {
    const body = { message: 'x', status: 200 } as unknown as { message: string }
    expect(new ApiErrorException(body, { status: 503 }).status).toBe(503)
  })
})

describe('isApiStatus', () => {
  it('matches an ApiErrorException with that status only', () => {
    const err = new ApiErrorException({ code: 'forbidden' }, { status: 403 })
    expect(isApiStatus(err, 403)).toBe(true)
    expect(isApiStatus(err, 500)).toBe(false)
  })

  it('rejects other errors and values', () => {
    expect(isApiStatus(new Error('403'), 403)).toBe(false)
    expect(isApiStatus(new TypeError('Failed to fetch'), 403)).toBe(false)
    expect(isApiStatus({ status: 403 }, 403)).toBe(false)
    expect(isApiStatus(null, 403)).toBe(false)
    expect(isApiStatus(new ApiErrorException({ code: 'x' }), 403)).toBe(false)
  })
})
