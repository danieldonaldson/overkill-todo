import { describe, expect, it, vi } from 'vitest'

import { createUsersApi } from './users'
import type { HttpClient } from './client'
import type { AccountDeletionResponse, AccountExportResponse } from '../types'

function fakeClient(resolved: unknown = null) {
  const request = vi.fn(() => Promise.resolve(resolved))
  const client = { request } as unknown as HttpClient
  return { client, request }
}

describe('users api', () => {
  it('changes password via POST /users/password/change', async () => {
    const { client, request } = fakeClient()
    const api = createUsersApi(client)

    await api.changePassword({
      current_password: 'oldpassword123',
      new_password: 'newpassword123',
    })

    expect(request).toHaveBeenCalledWith('/users/password/change', {
      method: 'POST',
      body: JSON.stringify({
        current_password: 'oldpassword123',
        new_password: 'newpassword123',
      }),
    })
  })

  it('sets a password via POST /users/password/set', async () => {
    const { client, request } = fakeClient()
    const api = createUsersApi(client)

    await api.setPassword({ new_password: 'newpassword123' })

    expect(request).toHaveBeenCalledWith('/users/password/set', {
      method: 'POST',
      body: JSON.stringify({ new_password: 'newpassword123' }),
    })
  })

  it('exports account data via GET /users/me/export and returns the payload', async () => {
    const exportPayload: AccountExportResponse = {
      id: 'user-1',
      email: 'test@example.com',
      first_name: 'Test',
      last_name: 'User',
      email_verified: true,
      has_password: true,
      role: 'user',
      created_at: '2026-01-01T00:00:00Z',
      updated_at: '2026-01-02T00:00:00Z',
      linked_oauth_accounts: [],
      exported_at: '2026-06-20T00:00:00Z',
    }
    const { client, request } = fakeClient(exportPayload)
    const api = createUsersApi(client)

    const result = await api.exportData()

    expect(request).toHaveBeenCalledWith('/users/me/export', { method: 'GET' })
    expect(result).toEqual(exportPayload)
  })

  it('requests deletion via DELETE /users/me with password and confirmation', async () => {
    const response: AccountDeletionResponse = {
      deletion_scheduled_at: '2026-07-04T00:00:00Z',
    }
    const { client, request } = fakeClient(response)
    const api = createUsersApi(client)

    const result = await api.requestDeletion({
      password: 'mypassword123',
      confirmation: 'DELETE',
    })

    expect(request).toHaveBeenCalledWith('/users/me', {
      method: 'DELETE',
      body: JSON.stringify({
        password: 'mypassword123',
        confirmation: 'DELETE',
      }),
    })
    expect(result).toEqual(response)
  })

  it('requests deletion without a password for OAuth-only accounts', async () => {
    const { client, request } = fakeClient({})
    const api = createUsersApi(client)

    await api.requestDeletion({ confirmation: 'DELETE' })

    expect(request).toHaveBeenCalledWith('/users/me', {
      method: 'DELETE',
      body: JSON.stringify({ confirmation: 'DELETE' }),
    })
  })

  it('cancels a pending deletion via POST /users/me/cancel-deletion', async () => {
    const response: AccountDeletionResponse = {}
    const { client, request } = fakeClient(response)
    const api = createUsersApi(client)

    const result = await api.cancelDeletion()

    expect(request).toHaveBeenCalledWith('/users/me/cancel-deletion', { method: 'POST' })
    expect(result).toEqual(response)
  })
})
