import type { AdminUserList, AdminUserRow, UserRole } from '../types'
import { HttpClient } from './client'

/** Build a `?limit=&offset=` suffix, omitting absent params (and the `?`). */
function pageQuery(params?: { limit?: number; offset?: number }): string {
  const query = new URLSearchParams()
  if (params?.limit != null) query.set('limit', String(params.limit))
  if (params?.offset != null) query.set('offset', String(params.offset))
  const qs = query.toString()
  return qs ? `?${qs}` : ''
}

// Admin panel endpoints (`/api/v1/admin/*`). The backend enforces the admin /
// super-admin role via request extractors; these calls 403 for everyone else.
export function createAdminApi(client: HttpClient) {
  return {
    /** List users (paginated). */
    listUsers: (params?: { limit?: number; offset?: number }): Promise<AdminUserList> =>
      client.request(`/admin/users${pageQuery(params)}`, { method: 'GET' }),

    /** Change a user's role (super-admin only on the backend). */
    setRole: (id: string, role: UserRole): Promise<AdminUserRow> =>
      client.request(`/admin/users/${id}/role`, {
        method: 'PUT',
        body: JSON.stringify({ role }),
      }),
  }
}
