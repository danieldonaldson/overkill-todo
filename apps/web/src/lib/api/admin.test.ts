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
    expect(request).toHaveBeenCalledWith('/admin/users?limit=10&offset=20', { method: 'GET' })
  })

  it('lists users without query when no params', async () => {
    const { client, request } = fakeClient()
    await createAdminApi(client).listUsers()
    expect(request).toHaveBeenCalledWith('/admin/users', { method: 'GET' })
  })

  it('sets a role via PUT /admin/users/{id}/role', async () => {
    const { client, request } = fakeClient()
    await createAdminApi(client).setRole('u1', 'admin')
    expect(request).toHaveBeenCalledWith('/admin/users/u1/role', {
      method: 'PUT',
      body: JSON.stringify({ role: 'admin' }),
    })
  })
})
