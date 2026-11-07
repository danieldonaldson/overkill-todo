import type { OAuthProvider } from '../types'
import { HttpClient } from './client'
import { getDeviceFingerprint } from '../auth/fingerprint'
import { API_BASE } from '../config'

type OAuthMode = 'login' | 'link'

async function storeOAuthDeviceFingerprint(client: HttpClient): Promise<void> {
  const fingerprint = await getDeviceFingerprint()
  await client.request<void>('/auth/oauth/device-fingerprint', {
    method: 'POST',
    body: JSON.stringify({ device_fingerprint: fingerprint }),
  })
}

export function oauthStartUrl(provider: OAuthProvider, mode: OAuthMode): string {
  const params = new URLSearchParams({ mode })
  return `${API_BASE}/auth/oauth/${provider}?${params.toString()}`
}

export function createOAuthApi(
  client: HttpClient,
  redirect: (url: string) => void = (url) => {
    window.location.href = url
  }
) {
  return {
    /**
     * Redirect the browser to the provider's OAuth authorization flow (login).
     * Backend: GET /api/v1/auth/oauth/{provider}?mode=login
     */
    startLogin: async (provider: OAuthProvider): Promise<void> => {
      await storeOAuthDeviceFingerprint(client)
      redirect(oauthStartUrl(provider, 'login'))
    },

    /**
     * Start the OAuth flow for linking a provider to the current account.
     * The user must already be logged in.
     * Backend: GET /api/v1/auth/oauth/{provider}?mode=link
     */
    startLink: async (provider: OAuthProvider): Promise<void> => {
      await storeOAuthDeviceFingerprint(client)
      redirect(oauthStartUrl(provider, 'link'))
    },

    /**
     * Unlink an OAuth provider from the current account.
     * Backend: DELETE /api/v1/auth/oauth/{provider}/unlink
     */
    unlink: (provider: OAuthProvider): Promise<void> =>
      client.request(`/auth/oauth/${provider}/unlink`, {
        method: 'DELETE',
      }),
  }
}
