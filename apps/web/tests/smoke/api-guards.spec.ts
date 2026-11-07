import { expect, test } from '@playwright/test'
import { APP_BASE_URL } from './targets'

// Part 5's spot-checks, in their unauthenticated form. Read-only by
// construction: the mutating verbs below are rejected by the CSRF layer before
// any handler runs, and even if that layer regressed the request carries no
// session, so it would then be rejected by auth. Neither path can change data.

test.describe('unauthenticated access is refused uniformly', () => {
  const PROTECTED = [
    '/api/v1/auth/me',
    '/api/v1/users/me/export',
    '/api/v1/admin/users',
  ]

  for (const path of PROTECTED) {
    test(`GET ${path} without a session is 401 token_missing`, async ({ request }) => {
      const res = await request.get(`${APP_BASE_URL}${path}`)
      expect(res.status()).toBe(401)
      // Generic and identical across endpoints: no enumeration signal.
      expect(await res.json()).toEqual({
        code: 'token_missing',
        message: 'Missing authentication token',
      })
    })
  }
})

test.describe('CSRF is enforced ahead of authentication', () => {
  // CSRF is evaluated before auth, so an unauthenticated mutation returns 403
  // rather than 401. That ordering is deliberate — and worth knowing when
  // reading logs, where a 403 here can look like a permissions problem.
  const MUTATIONS = [
    { method: 'post' as const, path: '/api/v1/users/password/change' },
    { method: 'delete' as const, path: '/api/v1/users/me' },
  ]

  for (const { method, path } of MUTATIONS) {
    test(`${method.toUpperCase()} ${path} without X-CSRF-Token is 403 csrf_invalid`, async ({
      request,
    }) => {
      const res = await request[method](`${APP_BASE_URL}${path}`)
      expect(res.status()).toBe(403)
      expect(await res.json()).toEqual({
        code: 'csrf_invalid',
        message: 'Invalid or missing CSRF token',
      })
    })
  }
})
