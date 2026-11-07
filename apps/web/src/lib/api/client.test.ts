import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest'

vi.mock('../navigation', () => ({
  redirect: vi.fn(),
}))
vi.mock('../auth/fingerprint', () => ({
  getDeviceFingerprint: vi.fn(() => Promise.resolve('test-fingerprint')),
}))
vi.mock('../auth/session', () => ({
  clearSessionUser: vi.fn(),
}))
vi.mock('../auth/token', () => ({
  ensureValidToken: vi.fn(() => Promise.resolve(true)),
  coordinateRefresh: vi.fn((fn: () => Promise<boolean>) => fn()),
  getTokenExpiry: vi.fn(() => 1_700_000_000),
  setTokenExpiry: vi.fn(),
  clearTokenExpiry: vi.fn(),
}))

import { getApiBaseUrl, getLastTraceId, HttpClient } from './client'
import { clearTokenExpiry, ensureValidToken, getTokenExpiry, setTokenExpiry } from '../auth/token'
import { clearSessionUser } from '../auth/session'
import { redirect } from '../navigation'

interface FakeResponseInit {
  status?: number
  ok?: boolean
  body?: unknown
  traceId?: string | null
}

function fakeResponse({ status = 200, ok, body = {}, traceId = null }: FakeResponseInit = {}) {
  const resolvedOk = ok ?? (status >= 200 && status < 300)
  return {
    ok: resolvedOk,
    status,
    headers: { get: (name: string) => (name === 'x-trace-id' ? traceId : null) },
    json: () => Promise.resolve(body),
    clone: () => fakeResponse({ status, ok: resolvedOk, body, traceId }),
  }
}

const mockFetch = vi.fn()

beforeEach(() => {
  vi.stubGlobal('fetch', mockFetch)
  document.cookie = 'csrf_token=; expires=Thu, 01 Jan 1970 00:00:00 GMT; path=/'
  ;(ensureValidToken as Mock).mockResolvedValue(true)
  ;(getTokenExpiry as Mock).mockReturnValue(1_700_000_000)
})

afterEach(() => {
  vi.unstubAllGlobals()
  mockFetch.mockReset()
  vi.clearAllMocks()
})

describe('getApiBaseUrl', () => {
  it('returns an empty string so the browser builds the URL from relative paths', () => {
    expect(getApiBaseUrl()).toBe('')
  })
})

describe('HttpClient.request', () => {
  it('returns parsed JSON and sends credentials for a successful request', async () => {
    mockFetch.mockResolvedValueOnce(fakeResponse({ body: { id: 'item-1' } }))
    const client = new HttpClient()

    const result = await client.request('/items')

    expect(result).toEqual({ id: 'item-1' })
    expect(mockFetch).toHaveBeenCalledWith(
      '/api/v1/items',
      expect.objectContaining({ credentials: 'include' })
    )
  })

  it('ensures a valid token before non-auth requests', async () => {
    mockFetch.mockResolvedValueOnce(fakeResponse())
    await new HttpClient().request('/items')
    expect(ensureValidToken).toHaveBeenCalledTimes(1)
  })

  it('skips the token check for auth endpoints', async () => {
    // A csrf cookie is present so this mutating request doesn't trigger a
    // proactive CSRF refresh (which would fire its own fetch).
    document.cookie = 'csrf_token=csrf-abc; path=/'
    mockFetch.mockResolvedValueOnce(fakeResponse({ body: { ok: true } }))
    await new HttpClient().request('/auth/login', { method: 'POST' })
    expect(ensureValidToken).not.toHaveBeenCalled()
  })

  it('returns undefined for a 204 No Content response', async () => {
    // csrf cookie present so the DELETE doesn't first fire a proactive refresh.
    document.cookie = 'csrf_token=csrf-abc; path=/'
    mockFetch.mockResolvedValueOnce(fakeResponse({ status: 204 }))
    const result = await new HttpClient().request('/items/1', { method: 'DELETE' })
    expect(result).toBeUndefined()
  })

  it('attaches the CSRF token header on state-changing requests when the cookie is present', async () => {
    document.cookie = 'csrf_token=csrf-abc; path=/'
    mockFetch.mockResolvedValueOnce(fakeResponse({ body: {} }))

    await new HttpClient().request('/items', { method: 'POST' })

    const headers = mockFetch.mock.calls[0][1].headers as Record<string, string>
    expect(headers['x-csrf-token']).toBe('csrf-abc')
  })

  it('proactively refreshes to mint a CSRF cookie before a mutating request when none is present', async () => {
    // No csrf cookie (cleared in beforeEach): the client refreshes first to mint
    // one, so the first fetch is /auth/refresh and the second is the request.
    mockFetch
      .mockResolvedValueOnce(fakeResponse({ body: { expires_at: 1_700_000_100 } }))
      .mockResolvedValueOnce(fakeResponse({ body: { id: 'item-1' } }))

    const result = await new HttpClient().request('/items', { method: 'POST' })

    expect(result).toEqual({ id: 'item-1' })
    expect(mockFetch).toHaveBeenNthCalledWith(
      1,
      '/api/v1/auth/refresh',
      expect.objectContaining({ method: 'POST' })
    )
  })

  it('does not attach a CSRF header on GET requests', async () => {
    document.cookie = 'csrf_token=csrf-abc; path=/'
    mockFetch.mockResolvedValueOnce(fakeResponse())

    await new HttpClient().request('/items')

    const headers = mockFetch.mock.calls[0][1].headers as Record<string, string>
    expect(headers['x-csrf-token']).toBeUndefined()
  })

  it('redirects to the expired-session login when a proactive refresh fails', async () => {
    ;(ensureValidToken as Mock).mockResolvedValue(false)
    const client = new HttpClient()

    await expect(client.request('/items')).rejects.toThrow('Session expired')
    expect(clearTokenExpiry).toHaveBeenCalled()
    expect(clearSessionUser).toHaveBeenCalled()
    expect(redirect).toHaveBeenCalledWith('/login?session=expired')
    expect(mockFetch).not.toHaveBeenCalled()
  })

  it('refreshes and retries once on a 401, then returns the retried result', async () => {
    mockFetch
      .mockResolvedValueOnce(fakeResponse({ status: 401 }))
      .mockResolvedValueOnce(fakeResponse({ body: { expires_at: 1_700_000_100 } }))
      .mockResolvedValueOnce(fakeResponse({ body: { id: 'item-1' } }))

    const result = await new HttpClient().request('/items')

    expect(result).toEqual({ id: 'item-1' })
    expect(mockFetch).toHaveBeenNthCalledWith(
      2,
      '/api/v1/auth/refresh',
      expect.objectContaining({ method: 'POST' })
    )
    expect(setTokenExpiry).toHaveBeenCalledWith(1_700_000_100)
  })

  it('redirects to login when the 401 refresh attempt fails', async () => {
    mockFetch
      .mockResolvedValueOnce(fakeResponse({ status: 401 }))
      .mockResolvedValueOnce(fakeResponse({ status: 401 }))

    await expect(new HttpClient().request('/items')).rejects.toThrow('Session expired')
    expect(redirect).toHaveBeenCalledWith('/login?session=expired')
  })

  it('refreshes and retries once on a 403 csrf_invalid error', async () => {
    document.cookie = 'csrf_token=csrf-abc; path=/'
    mockFetch
      .mockResolvedValueOnce(fakeResponse({ status: 403, body: { code: 'csrf_invalid' } }))
      .mockResolvedValueOnce(fakeResponse({ body: { expires_at: 1_700_000_100 } }))
      .mockResolvedValueOnce(fakeResponse({ body: { id: 'item-1' } }))

    const result = await new HttpClient().request('/items', { method: 'POST' })

    expect(result).toEqual({ id: 'item-1' })
  })

  it('redirects to the verification page on a 403 email_not_verified error', async () => {
    mockFetch.mockResolvedValueOnce(
      fakeResponse({ status: 403, body: { code: 'email_not_verified' } })
    )

    await expect(new HttpClient().request('/items')).rejects.toThrow('Email not verified')
    expect(redirect).toHaveBeenCalledWith('/verify-email')
  })

  it('throws a typed Error carrying the structured fields when the body has an error_code', async () => {
    const body = { error_code: 'rate_limited', attempts_remaining: 2, message: 'Slow down' }
    mockFetch.mockResolvedValueOnce(fakeResponse({ status: 429, body }))

    const error = await new HttpClient()
      .request('/items')
      .then(() => null)
      .catch((e: unknown) => e)

    expect(error).toBeInstanceOf(Error)
    expect(error).toMatchObject(body)
    expect((error as Error).message).toBe('Slow down')
  })

  it('throws a plain error with the trace id appended for unstructured failures', async () => {
    mockFetch.mockResolvedValueOnce(
      fakeResponse({ status: 500, body: { message: 'Boom' }, traceId: 'trace-123' })
    )

    await expect(new HttpClient().request('/items')).rejects.toThrow('Boom (trace: trace-123)')
    expect(getLastTraceId()).toBe('trace-123')
  })
})

describe('HttpClient.requestWithFingerprint', () => {
  it('adds the device fingerprint header', async () => {
    // csrf cookie present so the POST doesn't first fire a proactive refresh,
    // keeping the register call as the first (and only) fetch.
    document.cookie = 'csrf_token=csrf-abc; path=/'
    mockFetch.mockResolvedValueOnce(fakeResponse({ body: {} }))

    await new HttpClient().requestWithFingerprint('/auth/register', { method: 'POST' })

    const headers = mockFetch.mock.calls[0][1].headers as Record<string, string>
    expect(headers['X-Device-Fingerprint']).toBe('test-fingerprint')
  })
})
