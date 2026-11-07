import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../auth/fingerprint', () => ({
  getDeviceFingerprint: vi.fn(() => Promise.resolve('device-binding-123')),
}))

import { createOAuthApi, oauthStartUrl } from './oauth'
import type { HttpClient } from './client'

function fakeClient() {
  const request = vi.fn(() => Promise.resolve(undefined))
  const client = { request } as unknown as HttpClient
  return { client, request }
}

describe('oauth api', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('builds OAuth start URLs without the device binding', () => {
    expect(oauthStartUrl('google', 'login')).toBe('/api/v1/auth/oauth/google?mode=login')
    expect(oauthStartUrl('google', 'link')).toBe('/api/v1/auth/oauth/google?mode=link')
  })

  it('stores the device binding before redirecting to login', async () => {
    const { client, request } = fakeClient()
    const redirect = vi.fn()
    const api = createOAuthApi(client, redirect)

    await api.startLogin('google')

    expect(request).toHaveBeenCalledWith('/auth/oauth/device-fingerprint', {
      method: 'POST',
      body: JSON.stringify({ device_fingerprint: 'device-binding-123' }),
    })
    expect(redirect).toHaveBeenCalledWith('/api/v1/auth/oauth/google?mode=login')
  })

  it('stores the device binding before redirecting to link', async () => {
    const { client, request } = fakeClient()
    const redirect = vi.fn()
    const api = createOAuthApi(client, redirect)

    await api.startLink('google')

    expect(request).toHaveBeenCalledWith('/auth/oauth/device-fingerprint', {
      method: 'POST',
      body: JSON.stringify({ device_fingerprint: 'device-binding-123' }),
    })
    expect(redirect).toHaveBeenCalledWith('/api/v1/auth/oauth/google?mode=link')
  })

  it('unlinks a provider via DELETE /auth/oauth/{provider}/unlink', async () => {
    const { client, request } = fakeClient()
    const api = createOAuthApi(client, vi.fn())

    await api.unlink('google')

    expect(request).toHaveBeenCalledWith('/auth/oauth/google/unlink', {
      method: 'DELETE',
    })
  })
})
