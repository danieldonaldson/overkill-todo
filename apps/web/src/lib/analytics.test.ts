import { afterEach, describe, expect, it, vi } from 'vitest'

import { signinRejectedReason, signupRejectedReason, track } from './analytics'
import { ApiErrorException } from './api/errors'

type WithUmami = { umami?: { track: (...args: unknown[]) => unknown } }

function apiError(status: number, code?: string) {
  return new ApiErrorException(code ? { code, message: 'secret server text' } : null, { status })
}

describe('track', () => {
  afterEach(() => {
    delete (globalThis as WithUmami).umami
  })

  it('does nothing when the tracker is not loaded', () => {
    expect(() => track({ name: 'signup-submitted' })).not.toThrow()
  })

  it('sends the name, and the properties when the event has them', () => {
    const spy = vi.fn()
    ;(globalThis as WithUmami).umami = { track: spy }

    track({ name: 'signin-succeeded' })
    track({ name: 'email-verified', props: { via: 'link' } })

    expect(spy).toHaveBeenNthCalledWith(1, 'signin-succeeded')
    expect(spy).toHaveBeenNthCalledWith(2, 'email-verified', { via: 'link' })
  })

  it('swallows a throwing tracker and a rejected send', async () => {
    ;(globalThis as WithUmami).umami = {
      track: () => {
        throw new Error('blocked')
      },
    }
    expect(() => track({ name: 'signup-submitted' })).not.toThrow()

    const rejected = Promise.reject(new Error('offline'))
    ;(globalThis as WithUmami).umami = { track: () => rejected }
    expect(track({ name: 'signup-submitted' })).toBeUndefined()
    await rejected.catch(() => {})
  })

  it('accepts only the spec’s events and values', () => {
    // Type-level: these lines fail `astro check` if the union ever loosens.
    // @ts-expect-error unknown event name
    track({ name: 'page-scrolled' })
    // @ts-expect-error free text where a fixed value belongs
    track({ name: 'signup-rejected', props: { reason: 'Email already taken' } })
    // @ts-expect-error extra identifying property
    track({ name: 'signin-succeeded', props: { email: 'a@example.com' } })
    // @ts-expect-error missing required property
    track({ name: 'email-verified' })
    expect(true).toBe(true)
  })
})

describe('rejection reasons', () => {
  const cases: [string, unknown, string, string][] = [
    // label, error, signup reason, signin reason
    ['bot check failed', apiError(400, 'bot_check_failed'), 'captcha', 'captcha'],
    ['validation error', apiError(400, 'validation_error'), 'invalid', 'credentials'],
    ['422', apiError(422), 'invalid', 'credentials'],
    ['bad credentials', apiError(401, 'invalid_credentials'), 'invalid', 'credentials'],
    ['rate limited', apiError(429), 'rate-limited', 'rate-limited'],
    ['server error', apiError(500, 'internal_error'), 'server', 'server'],
    ['bad gateway', apiError(502), 'server', 'server'],
    ['network failure', new TypeError('Failed to fetch'), 'server', 'server'],
    ['anything else', 'weird', 'server', 'server'],
  ]

  for (const [label, error, signup, signin] of cases) {
    it(`${label} → signup ${signup}, signin ${signin}`, () => {
      expect(signupRejectedReason(error)).toBe(signup)
      expect(signinRejectedReason(error)).toBe(signin)
    })
  }
})
