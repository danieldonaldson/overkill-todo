//! The audit log port (OT-008): reading events, and recording a standalone one.
//!
//! Most events are written by the repository method that makes their action's
//! primary write (for example [`super::UserRepository::save_role_change`]), in
//! the same transaction. [`AuditLogRepository::record`] is only for an action
//! that writes nothing else (data export).

use async_trait::async_trait;
use jiff::Timestamp;

use crate::domain::entities::{Actor, AuditDetails, AuditEvent, AuditKind};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;

/// Which events to list. Both conditions must hold when both are set.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuditFilter {
    /// Events where this account was affected or acted.
    pub account: Option<UserId>,
    pub kind: Option<AuditKind>,
}

/// An account named by an event, as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditAccount {
    pub id: UserId,
    /// The current email, or `None` once the account is deleted (anonymised).
    /// Never stored in the event itself.
    pub email: Option<String>,
}

/// Who acted, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditActorView {
    User(AuditAccount),
    System,
    Operator,
}

/// One stored event, as read back for the audit log page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEntry {
    pub id: i64,
    pub occurred_at: Timestamp,
    /// `None` for a kind this release doesn't know (written by a newer one).
    pub kind: Option<AuditKind>,
    pub subject: AuditAccount,
    pub actor: AuditActorView,
    pub details: AuditDetails,
}

/// One event about a single account, as read back for that account's own
/// Security activity. Carries no emails: the actor is reduced further before
/// it leaves the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectEntry {
    pub id: i64,
    pub occurred_at: Timestamp,
    /// `None` for a kind this release doesn't know (written by a newer one).
    pub kind: Option<AuditKind>,
    pub actor: Actor,
    pub details: AuditDetails,
}

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AuditLogRepository: Send + Sync {
    /// Stores `event` on its own, for an action with no other write.
    async fn record(&self, event: &AuditEvent) -> Result<(), DomainError>;

    /// Events matching `filter`, newest first.
    async fn list(
        &self,
        filter: AuditFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<AuditEntry>, DomainError>;

    /// How many events match `filter`.
    async fn count(&self, filter: AuditFilter) -> Result<i64, DomainError>;

    /// Up to `limit` events whose affected account is `subject`, newest
    /// first, older than `before_id` when given (keyset paging).
    async fn list_for_subject(
        &self,
        subject: UserId,
        limit: i64,
        before_id: Option<i64>,
    ) -> Result<Vec<SubjectEntry>, DomainError>;
}
