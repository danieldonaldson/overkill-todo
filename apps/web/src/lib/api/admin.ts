import type {
  AdminUserList,
  AdminUserRow,
  AuditEventList,
  AuditKindFilter,
  UserRole,
} from '../types'
import { HttpClient } from './client'

/** Build a query suffix, omitting absent params (and the `?` when none). */
function pageQuery(params?: Record<string, string | number | undefined | null>): string {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(params ?? {})) {
    if (value != null && value !== '') query.set(key, String(value))
  }
  const qs = query.toString()
  return qs ? `?${qs}` : ''
}

export interface AuditEventsQuery {
  limit?: number
  offset?: number
  /** Only events where this account was affected or acted. */
  account_id?: string
  kind?: AuditKindFilter
}

// Admin panel endpoints (`/api/v1/admin/*`). The backend enforces the admin /
// super-admin role via request extractors; these calls 403 for everyone else.
export function createAdminApi(client: HttpClient) {
  return {
    /** List users (paginated). */
    listUsers: (params?: { limit?: number; offset?: number }): Promise<AdminUserList> =>
      client.request(`/admin/users${pageQuery(params)}`, { method: 'GET', route: '/admin/users' }),

    /** Change a user's role (super-admin only on the backend). */
    setRole: (id: string, role: UserRole): Promise<AdminUserRow> =>
      client.request(`/admin/users/${id}/role`, {
        method: 'PUT',
        route: '/admin/users/{id}/role',
        body: JSON.stringify({ role }),
      }),

    /** List audit events, newest first (super-admin only on the backend). */
    listAuditEvents: (params?: AuditEventsQuery): Promise<AuditEventList> =>
      client.request(`/admin/audit-events${pageQuery({ ...params })}`, {
        method: 'GET',
        route: '/admin/audit-events',
      }),
  }
}
