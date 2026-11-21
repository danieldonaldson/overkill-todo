import { describe, expect, it, vi } from 'vitest'

import { createAdminApi } from './admin'
import type { HttpClient } from './client'

function fakeClient() {
  const request = vi.fn(() => Promise.resolve(null as unknown))
  const client = { request } as unknown as HttpClient
  return { client, request }
}

describe('admin api', () => {
  it('lists users with pagination query', async () => {
    const { client, request } = fakeClient()
    await createAdminApi(client).listUsers({ limit: 10, offset: 20 })
    expect(request).toHaveBeenCalledWith('/admin/users?limit=10&offset=20', {
      method: 'GET',
      route: '/admin/users',
    })
  })

  it('lists users without query when no params', async () => {
    const { client, request } = fakeClient()
    await createAdminApi(client).listUsers()
    expect(request).toHaveBeenCalledWith('/admin/users', { method: 'GET', route: '/admin/users' })
  })

  it('sets a role via PUT /admin/users/{id}/role', async () => {
    const { client, request } = fakeClient()
    await createAdminApi(client).setRole('u1', 'admin')
    expect(request).toHaveBeenCalledWith('/admin/users/u1/role', {
      method: 'PUT',
      route: '/admin/users/{id}/role',
      body: JSON.stringify({ role: 'admin' }),
    })
  })

  it('lists audit events with filters, omitting empty ones', async () => {
    const { client, request } = fakeClient()
    const admin = createAdminApi(client)
    await admin.listAuditEvents({ limit: 50, offset: 0, account_id: 'u1', kind: 'role_changed' })
    expect(request).toHaveBeenCalledWith(
      '/admin/audit-events?limit=50&offset=0&account_id=u1&kind=role_changed',
      { method: 'GET', route: '/admin/audit-events' }
    )
    await admin.listAuditEvents({ account_id: '' })
    expect(request).toHaveBeenLastCalledWith('/admin/audit-events', {
      method: 'GET',
      route: '/admin/audit-events',
    })
  })
})
