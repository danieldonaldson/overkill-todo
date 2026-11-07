import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('../auth/token', () => ({
  setTokenExpiry: vi.fn(),
  clearTokenExpiry: vi.fn(),
}))

import { setTokenExpiry, clearTokenExpiry } from '../auth/token'
import { createAuthApi } from './auth'
import type { HttpClient } from './client'

const authResponse = {
  expires_at: 1_700_000_100,
  user: {
    id: 'user-1',
    email: 'test@example.com',
    first_name: '',
    last_name: '',
    email_verified: true,
    has_password: true,
    oauth_providers: [],
  },
}

function fakeClient(fingerprintResult: unknown = authResponse, requestResult: unknown = null) {
  const request = vi.fn(() => Promise.resolve(requestResult))
  const requestWithFingerprint = vi.fn(() => Promise.resolve(fingerprintResult))
  const client = { request, requestWithFingerprint } as unknown as HttpClient
  return { client, request, requestWithFingerprint }
}

describe('auth api', () => {
  beforeEach(() => {
    vi.clearAllMocks()
  })

  it('logs in via fingerprinted POST /auth/login and records token expiry', async () => {
    const { client, requestWithFingerprint } = fakeClient()
    const api = createAuthApi(client)

    const result = await api.login({ email: 'test@example.com', password: 'password123' })

    expect(requestWithFingerprint).toHaveBeenCalledWith('/auth/login', {
      method: 'POST',
      body: JSON.stringify({ email: 'test@example.com', password: 'password123' }),
    })
    expect(setTokenExpiry).toHaveBeenCalledWith(1_700_000_100)
    expect(result).toEqual(authResponse)
  })

  it('initiates registration via POST /auth/register without a fingerprint', async () => {
    const { client, request, requestWithFingerprint } = fakeClient(undefined, {
      success: true,
      message: 'check your email',
    })
    const api = createAuthApi(client)

    await api.register({ email: 'new@example.com' })

    expect(request).toHaveBeenCalledWith('/auth/register', {
      method: 'POST',
      body: JSON.stringify({ email: 'new@example.com' }),
    })
    expect(requestWithFingerprint).not.toHaveBeenCalled()
    expect(setTokenExpiry).not.toHaveBeenCalled()
  })

  it('completes signup via fingerprinted POST /auth/complete-signup and records expiry', async () => {
    const { client, requestWithFingerprint } = fakeClient()
    const api = createAuthApi(client)

    await api.completeSignup({
      signup_token: 'signup-tok',
      email: 'new@example.com',
      password: 'password123',
    })

    expect(requestWithFingerprint).toHaveBeenCalledWith('/auth/complete-signup', {
      method: 'POST',
      body: JSON.stringify({
        signup_token: 'signup-tok',
        email: 'new@example.com',
        password: 'password123',
      }),
    })
    expect(setTokenExpiry).toHaveBeenCalledWith(1_700_000_100)
  })

  it('fetches the current user via GET /auth/me', async () => {
    const { client, request } = fakeClient(undefined, authResponse.user)
    const api = createAuthApi(client)

    const user = await api.me()

    expect(request).toHaveBeenCalledWith('/auth/me')
    expect(user).toEqual(authResponse.user)
  })

  it('logs out via POST /auth/logout and clears token expiry', async () => {
    const { client, request } = fakeClient()
    const api = createAuthApi(client)

    await api.logout()

    expect(request).toHaveBeenCalledWith('/auth/logout', { method: 'POST' })
    expect(clearTokenExpiry).toHaveBeenCalledTimes(1)
  })

  it('refreshes via fingerprinted POST /auth/refresh and records expiry', async () => {
    const { client, requestWithFingerprint } = fakeClient({ expires_at: 1_700_000_200 })
    const api = createAuthApi(client)

    const result = await api.refresh()

    expect(requestWithFingerprint).toHaveBeenCalledWith('/auth/refresh', { method: 'POST' })
    expect(setTokenExpiry).toHaveBeenCalledWith(1_700_000_200)
    expect(result).toEqual({ expires_at: 1_700_000_200 })
  })

  it('requests a password reset via POST /auth/forgot-password', async () => {
    const { client, request } = fakeClient()
    const api = createAuthApi(client)

    await api.forgotPassword({ email: 'test@example.com' })

    expect(request).toHaveBeenCalledWith('/auth/forgot-password', {
      method: 'POST',
      body: JSON.stringify({ email: 'test@example.com' }),
    })
  })

  it('verifies a reset code via POST /auth/verify-reset-code', async () => {
    const { client, request } = fakeClient(undefined, { reset_token: 'reset-tok' })
    const api = createAuthApi(client)

    const result = await api.verifyResetCode({ email: 'test@example.com', code: '123456' })

    expect(request).toHaveBeenCalledWith('/auth/verify-reset-code', {
      method: 'POST',
      body: JSON.stringify({ email: 'test@example.com', code: '123456' }),
    })
    expect(result).toEqual({ reset_token: 'reset-tok' })
  })

  it('stores token expiry after password reset auto-login', async () => {
    const { client, requestWithFingerprint } = fakeClient()
    const api = createAuthApi(client)

    await api.resetPassword({ reset_token: 'reset-token', new_password: 'newpassword123' })

    expect(requestWithFingerprint).toHaveBeenCalledWith('/auth/reset-password', {
      method: 'POST',
      body: JSON.stringify({
        reset_token: 'reset-token',
        new_password: 'newpassword123',
      }),
    })
    expect(setTokenExpiry).toHaveBeenCalledWith(1_700_000_100)
  })

  it('records expiry when verify-email auto-logs-in an existing user', async () => {
    const { client, requestWithFingerprint } = fakeClient({ expires_at: 1_700_000_300 })
    const api = createAuthApi(client)

    await api.verifyEmail({ email: 'test@example.com', code: '654321' })

    expect(requestWithFingerprint).toHaveBeenCalledWith('/auth/verify-email', {
      method: 'POST',
      body: JSON.stringify({ email: 'test@example.com', code: '654321' }),
    })
    expect(setTokenExpiry).toHaveBeenCalledWith(1_700_000_300)
  })

  it('does not record expiry when verify-email returns a signup token (new user)', async () => {
    const { client } = fakeClient({ signup_token: 'signup-tok' })
    const api = createAuthApi(client)

    await api.verifyEmail({ email: 'new@example.com', code: '111222' })

    expect(setTokenExpiry).not.toHaveBeenCalled()
  })

  it('resends verification via POST /auth/resend-verification', async () => {
    const { client, request } = fakeClient(undefined, { cooldown_seconds: 60 })
    const api = createAuthApi(client)

    const result = await api.resendVerification({ email: 'test@example.com' })

    expect(request).toHaveBeenCalledWith('/auth/resend-verification', {
      method: 'POST',
      body: JSON.stringify({ email: 'test@example.com' }),
    })
    expect(result).toEqual({ cooldown_seconds: 60 })
  })
})
