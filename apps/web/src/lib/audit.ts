// Human labels for audit events (OT-008), shared by every view of them.

import type { AuditEvent, AuditKindFilter, AuditKindName, UserRole } from './types'

/** One label per kind. A kind added to the API fails type-checking until it has one. */
export const AUDIT_KIND_LABELS: Record<AuditKindName, string> = {
  role_changed: 'Role changed',
  other: 'Other event',
}

/** The kinds the audit log can be filtered by, in display order. */
export const AUDIT_KIND_FILTERS: Record<AuditKindFilter, string> = {
  role_changed: AUDIT_KIND_LABELS.role_changed,
}

const ROLE_LABELS: Record<UserRole, string> = {
  user: 'Member',
  admin: 'Admin',
  super_admin: 'Super Admin',
}

/** The event's details as a phrase, or `null` when it has none. */
export function auditDetails(event: AuditEvent): string | null {
  const { from_role, to_role } = event.details
  if (from_role && to_role) return `${ROLE_LABELS[from_role]} → ${ROLE_LABELS[to_role]}`
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
