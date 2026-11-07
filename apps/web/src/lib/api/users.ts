import type {
  AccountDeletionResponse,
  AccountExportResponse,
  ChangePasswordRequest,
  RequestDeletionRequest,
  SetPasswordRequest,
} from '../types'
import { HttpClient } from './client'

// Password endpoints plus the self-service data-export / account-deletion
// surface (`/api/v1/users/me*`).
export function createUsersApi(client: HttpClient) {
  return {
    changePassword: (data: ChangePasswordRequest): Promise<void> =>
      client.request('/users/password/change', {
        method: 'POST',
        body: JSON.stringify(data),
      }),

    setPassword: (data: SetPasswordRequest): Promise<void> =>
      client.request('/users/password/set', {
        method: 'POST',
        body: JSON.stringify(data),
      }),

    /** Download a machine-readable export of all personal data (GDPR). */
    exportData: (): Promise<AccountExportResponse> =>
      client.request('/users/me/export', { method: 'GET' }),

    /** Schedule account deletion after a 14-day grace period. */
    requestDeletion: (data: RequestDeletionRequest): Promise<AccountDeletionResponse> =>
      client.request('/users/me', {
        method: 'DELETE',
        body: JSON.stringify(data),
      }),

    /** Cancel a pending account deletion during the grace period. */
    cancelDeletion: (): Promise<AccountDeletionResponse> =>
      client.request('/users/me/cancel-deletion', { method: 'POST' }),
  }
}
