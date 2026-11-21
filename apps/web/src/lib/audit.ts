// Human labels for audit events (OT-008), shared by every view of them.

import type {
  AuditEvent,
  AuditKindFilter,
  AuditKindName,
  OAuthProviderName,
  UserRole,
} from './types'

/** One label per kind. A kind added to the API fails type-checking until it has one. */
export const AUDIT_KIND_LABELS: Record<AuditKindName, string> = {
  role_changed: 'Role changed',
  password_changed: 'Password changed',
  password_set: 'Password set',
  password_reset: 'Password reset',
  sign_in_method_linked: 'Sign-in method linked',
  sign_in_method_unlinked: 'Sign-in method unlinked',
  deletion_requested: 'Account deletion requested',
  deletion_cancelled: 'Account deletion cancelled',
  account_deleted: 'Account deleted',
  data_exported: 'Data exported',
  sessions_revoked_after_reuse: 'All sessions signed out',
  other: 'Other event',
}

/** The kinds the audit log can be filtered by, in display order. */
export const AUDIT_KIND_FILTERS: Record<AuditKindFilter, string> = {
  role_changed: AUDIT_KIND_LABELS.role_changed,
  password_changed: AUDIT_KIND_LABELS.password_changed,
  password_set: AUDIT_KIND_LABELS.password_set,
  password_reset: AUDIT_KIND_LABELS.password_reset,
  sign_in_method_linked: AUDIT_KIND_LABELS.sign_in_method_linked,
  sign_in_method_unlinked: AUDIT_KIND_LABELS.sign_in_method_unlinked,
  deletion_requested: AUDIT_KIND_LABELS.deletion_requested,
  deletion_cancelled: AUDIT_KIND_LABELS.deletion_cancelled,
  account_deleted: AUDIT_KIND_LABELS.account_deleted,
  data_exported: AUDIT_KIND_LABELS.data_exported,
  sessions_revoked_after_reuse: AUDIT_KIND_LABELS.sessions_revoked_after_reuse,
}

const PROVIDER_LABELS: Record<OAuthProviderName, string> = {
  google: 'Google',
  apple: 'Apple',
}

const ROLE_LABELS: Record<UserRole, string> = {
  user: 'Member',
  admin: 'Admin',
  super_admin: 'Super Admin',
}

/** The event's details as a phrase, or `null` when it has none. */
export function auditDetails(event: AuditEvent): string | null {
  const { from_role, to_role, provider } = event.details
  if (from_role && to_role) return `${ROLE_LABELS[from_role]} → ${ROLE_LABELS[to_role]}`
  if (provider) return PROVIDER_LABELS[provider]
  return null
}

/** How an account named by an event is shown to a Super Admin. */
export function auditAccountLabel(account: AuditEvent['subject']): string {
  return account.deleted || !account.email ? 'Deleted account' : account.email
}

/** Who acted, as shown to a Super Admin. */
export function auditActorLabel(actor: AuditEvent['actor']): string {
  switch (actor.kind) {
    case 'user':
      return actor.account ? auditAccountLabel(actor.account) : 'Unknown account'
    case 'system':
      return 'System'
    case 'operator':
      return 'Operator'
  }
}
