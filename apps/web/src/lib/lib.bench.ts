// Hot client-side helpers: form validation, error narrowing, date formatting,
// class merging, and the HTTP client's per-request path (fetch stubbed, so this
// measures our code, not the network). Run with `just bench-web`.
import { bench, describe } from 'vitest'
import { HttpClient } from './api/client'
import { getErrorCode, getErrorMessage, isApiError } from './api/errors'
import { setTokenExpiry } from './auth/token'
import { formatDate, formatDateTime } from './format'
import { cn } from './utils'
import { email, maxLength, minLength, required, validateAll } from './utils/validation'

describe('validation', () => {
  const schema = {
    email: [required(), email(), maxLength(254)],
    password: [required(), minLength(8), maxLength(256)],
    first_name: [maxLength(100)],
  }
  const valid = { email: 'alice@example.com', password: 'correct horse battery', first_name: 'A' }
  const invalid = { email: 'alice@', password: 'short', first_name: '' }

  bench('validateAll signup (valid)', () => {
    validateAll(valid, schema)
  })
  bench('validateAll signup (invalid)', () => {
    validateAll(invalid, schema)
  })
})

describe('api errors', () => {
  const structured = { error_code: 'invalid_code', error: 'Wrong code', attempts_remaining: 2 }
  const plain = { code: 'csrf_invalid', message: 'Invalid CSRF token' }

  bench('isApiError + code + message', () => {
    for (const body of [structured, plain, null, 'nope']) {
      if (isApiError(body)) {
        getErrorCode(body)
        getErrorMessage(body)
      }
    }
  })
})

describe('format', () => {
  // One admin page of timestamps.
  const dates = Array.from({ length: 50 }, (_, i) =>
    new Date(Date.UTC(2026, 0, 1 + i, 12)).toISOString()
  )

  bench('formatDate x50', () => {
    for (const d of dates) formatDate(d)
  })
  bench('formatDateTime x50', () => {
    for (const d of dates) formatDateTime(d)
  })
})

describe('cn', () => {
  bench('button variant merge', () => {
    cn(
      'inline-flex items-center justify-center rounded-md text-sm font-medium px-4 py-2',
      'bg-primary text-primary-foreground hover:bg-primary/90',
      false && 'opacity-50',
      'px-6 rounded-lg'
    )
  })
})

describe('HttpClient.request', () => {
  // Set up in the describe body: Vitest bench mode does not run beforeAll/
  // afterAll hooks. Bench files run isolated, so the stubs don't leak.
  const body = JSON.stringify({ users: [], limit: 50, offset: 0 })
  const client = new HttpClient()
  // A live session, so the client goes straight to fetch without refreshing.
  setTokenExpiry(Math.floor(Date.now() / 1000) + 3600)
  document.cookie = 'csrf_token=bench-csrf-token; path=/'
  globalThis.fetch = async () =>
    new Response(body, {
      status: 200,
      headers: { 'content-type': 'application/json', 'x-trace-id': 'bench' },
    })

  // `throws`: a failing request must fail the run, not report NaN.
  bench(
    'GET (authenticated)',
    async () => {
      await client.request('/admin/users?limit=50', { method: 'GET' })
    },
    { throws: true }
  )
  bench(
    'PUT (CSRF header)',
    async () => {
      await client.request('/admin/users/u1/role', {
        method: 'PUT',
        body: JSON.stringify({ role: 'admin' }),
      })
    },
    { throws: true }
  )
})
