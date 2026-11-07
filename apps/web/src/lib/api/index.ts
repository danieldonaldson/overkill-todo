// Composes the web API endpoint groups into a single lazily-initialized client.
// The app is a static SPA calling the Rust API; add an endpoint group per feature.

import { HttpClient } from './client'
import { createAuthApi } from './auth'
import { createOAuthApi } from './oauth'
import { createUsersApi } from './users'
import { createAdminApi } from './admin'

export { getLastTraceId } from './client'

class ApiClient {
  private client: HttpClient

  auth: ReturnType<typeof createAuthApi>
  oauth: ReturnType<typeof createOAuthApi>
  users: ReturnType<typeof createUsersApi>
  admin: ReturnType<typeof createAdminApi>

  constructor() {
    this.client = new HttpClient()
    this.auth = createAuthApi(this.client)
    this.oauth = createOAuthApi(this.client)
    this.users = createUsersApi(this.client)
    this.admin = createAdminApi(this.client)
  }
}

let _apiInstance: ApiClient | null = null

function getApiClient(): ApiClient {
  if (!_apiInstance) {
    _apiInstance = new ApiClient()
  }
  return _apiInstance
}

// Proxy that forwards property access to the lazily-initialized client, so the
// client (which touches browser globals) is only constructed on first use.
export const api: ApiClient = new Proxy({} as ApiClient, {
  get(_target, prop) {
    const client = getApiClient()
    return client[prop as keyof ApiClient]
  },
})
