//! Reading the audit log (OT-008).
//!
//! Writing has no port of its own: an event is stored by the repository method
//! that makes its action's primary write (for example
//! [`super::UserRepository::save_role_change`]), in the same transaction.

use async_trait::async_trait;
use jiff::Timestamp;

use crate::domain::entities::{AuditDetails, AuditKind};
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

#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait AuditLogRepository: Send + Sync {
    /// Events matching `filter`, newest first.
    async fn list(
        &self,
        filter: AuditFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<AuditEntry>, DomainError>;

    /// How many events match `filter`.
    async fn count(&self, filter: AuditFilter) -> Result<i64, DomainError>;
}
