//! DTOs for the audit log (`GET /api/v1/admin/audit-events`, OT-008).
//!
//! Kinds and actors are closed enums, so the API description lists every value.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::schema_enums::{OAuthProviderName, RoleName};
use crate::ports::outgoing::{AuditAccount, AuditActorView, AuditEntry};
use crate::use_cases::user::{ActivityActor, ActivityEntry, ActivityPage};
use domain::entities::{AuditDetails, AuditKind, OAuthProvider};
use domain::value_objects::UserRole;

/// Default and maximum page sizes for the audit log.
pub const DEFAULT_AUDIT_LIMIT: i64 = 50;
pub const MAX_AUDIT_LIMIT: i64 = 100;

/// The kind of an audit event, as listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditKindName {
    RoleChanged,
    PasswordChanged,
    PasswordSet,
    PasswordReset,
    SignInMethodLinked,
    SignInMethodUnlinked,
    DeletionRequested,
    DeletionCancelled,
    AccountDeleted,
    DataExported,
    SessionsRevokedAfterReuse,
    /// A kind this server release doesn't know (written by a newer one).
    Other,
}

impl From<Option<AuditKind>> for AuditKindName {
    fn from(kind: Option<AuditKind>) -> Self {
        match kind {
            Some(AuditKind::RoleChanged) => Self::RoleChanged,
            Some(AuditKind::PasswordChanged) => Self::PasswordChanged,
            Some(AuditKind::PasswordSet) => Self::PasswordSet,
            Some(AuditKind::PasswordReset) => Self::PasswordReset,
            Some(AuditKind::SignInMethodLinked) => Self::SignInMethodLinked,
            Some(AuditKind::SignInMethodUnlinked) => Self::SignInMethodUnlinked,
            Some(AuditKind::DeletionRequested) => Self::DeletionRequested,
            Some(AuditKind::DeletionCancelled) => Self::DeletionCancelled,
            Some(AuditKind::AccountDeleted) => Self::AccountDeleted,
            Some(AuditKind::DataExported) => Self::DataExported,
            Some(AuditKind::SessionsRevokedAfterReuse) => Self::SessionsRevokedAfterReuse,
            None => Self::Other,
        }
    }
}

/// A kind to filter the audit log by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditKindFilter {
    RoleChanged,
    PasswordChanged,
    PasswordSet,
    PasswordReset,
    SignInMethodLinked,
    SignInMethodUnlinked,
    DeletionRequested,
    DeletionCancelled,
    AccountDeleted,
    DataExported,
    SessionsRevokedAfterReuse,
}

impl From<AuditKindFilter> for AuditKind {
    fn from(kind: AuditKindFilter) -> Self {
        match kind {
            AuditKindFilter::RoleChanged => Self::RoleChanged,
            AuditKindFilter::PasswordChanged => Self::PasswordChanged,
            AuditKindFilter::PasswordSet => Self::PasswordSet,
            AuditKindFilter::PasswordReset => Self::PasswordReset,
            AuditKindFilter::SignInMethodLinked => Self::SignInMethodLinked,
            AuditKindFilter::SignInMethodUnlinked => Self::SignInMethodUnlinked,
            AuditKindFilter::DeletionRequested => Self::DeletionRequested,
            AuditKindFilter::DeletionCancelled => Self::DeletionCancelled,
            AuditKindFilter::AccountDeleted => Self::AccountDeleted,
            AuditKindFilter::DataExported => Self::DataExported,
            AuditKindFilter::SessionsRevokedAfterReuse => Self::SessionsRevokedAfterReuse,
        }
    }
}

/// Who acted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuditActorKind {
    /// A signed-in account.
    User,
    /// An automatic action.
    System,
    /// The operator's bootstrap command.
    Operator,
}

/// An account named by an event, as it is now.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditAccountResponse {
    pub id: Uuid,
    /// The account's current email; `null` once the account is deleted.
    pub email: Option<String>,
    pub deleted: bool,
}

impl From<AuditAccount> for AuditAccountResponse {
    fn from(account: AuditAccount) -> Self {
        Self {
            id: *account.id.as_uuid(),
            deleted: account.email.is_none(),
            email: account.email,
        }
    }
}

#[derive(Debug, Serialize, ToSchema)]
pub struct AuditActorResponse {
    pub kind: AuditActorKind,
    /// The acting account, present only when `kind` is `user`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub account: Option<AuditAccountResponse>,
}

impl From<AuditActorView> for AuditActorResponse {
    fn from(actor: AuditActorView) -> Self {
        match actor {
            AuditActorView::User(account) => Self {
                kind: AuditActorKind::User,
                account: Some(account.into()),
            },
            AuditActorView::System => Self {
                kind: AuditActorKind::System,
                account: None,
            },
            AuditActorView::Operator => Self {
                kind: AuditActorKind::Operator,
                account: None,
            },
        }
    }
}

const fn role_name(role: UserRole) -> RoleName {
    match role {
        UserRole::User => RoleName::User,
        UserRole::Admin => RoleName::Admin,
        UserRole::SuperAdmin => RoleName::SuperAdmin,
    }
}

const fn provider_name(provider: OAuthProvider) -> OAuthProviderName {
    match provider {
        OAuthProvider::Google => OAuthProviderName::Google,
        OAuthProvider::Apple => OAuthProviderName::Apple,
    }
}

/// The event's details: for a role change, the old and new role; for a
/// sign-in method, the provider.
#[derive(Debug, Default, Serialize, ToSchema)]
pub struct AuditDetailsResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub from_role: Option<RoleName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub to_role: Option<RoleName>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub provider: Option<OAuthProviderName>,
}

impl From<AuditDetails> for AuditDetailsResponse {
    fn from(details: AuditDetails) -> Self {
        match details {
            AuditDetails::None => Self::default(),
            AuditDetails::RoleChanged { from, to } => Self {
                from_role: Some(role_name(from)),
                to_role: Some(role_name(to)),
                ..Self::default()
            },
            AuditDetails::SignInMethod { provider } => Self {
                provider: Some(provider_name(provider)),
                ..Self::default()
            },
        }
    }
}

/// One audit event.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditEventResponse {
    pub id: i64,
    pub occurred_at: Timestamp,
    pub kind: AuditKindName,
    /// The affected account.
    pub subject: AuditAccountResponse,
    pub actor: AuditActorResponse,
    pub details: AuditDetailsResponse,
}

impl From<AuditEntry> for AuditEventResponse {
    fn from(entry: AuditEntry) -> Self {
        Self {
            id: entry.id,
            occurred_at: entry.occurred_at,
            kind: entry.kind.into(),
            subject: entry.subject.into(),
            actor: entry.actor.into(),
            details: entry.details.into(),
        }
    }
}

/// A page of audit events, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuditEventListResponse {
    pub events: Vec<AuditEventResponse>,
    /// Events matching the filter, across all pages.
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

/// Query parameters for the audit log.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ListAuditEventsQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub offset: Option<i64>,
    /// Only events where this account was affected or acted.
    #[serde(default)]
    pub account_id: Option<Uuid>,
    #[serde(default)]
    pub kind: Option<AuditKindFilter>,
}

impl ListAuditEventsQuery {
    /// Resolve to `(limit, offset)`: limit in `[1, MAX_AUDIT_LIMIT]`, offset `>= 0`.
    #[must_use]
    pub fn paginated(&self) -> (i64, i64) {
        let limit = self
            .limit
            .unwrap_or(DEFAULT_AUDIT_LIMIT)
            .clamp(1, MAX_AUDIT_LIMIT);
        let offset = self.offset.unwrap_or(0).max(0);
        (limit, offset)
    }
}

// ---------------------------------------------------------------------------
// A Member's own Security activity (`GET /api/v1/users/me/security-activity`)
// ---------------------------------------------------------------------------

/// Default and maximum page sizes for Security activity.
pub const DEFAULT_ACTIVITY_LIMIT: i64 = 20;
pub const MAX_ACTIVITY_LIMIT: i64 = 50;

/// Who acted, as a Member is told: never which staff account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ActivityActorName {
    /// The Member themselves.
    You,
    /// An automatic action.
    System,
    /// Another account or the operator.
    Administrator,
}

impl From<ActivityActor> for ActivityActorName {
    fn from(actor: ActivityActor) -> Self {
        match actor {
            ActivityActor::You => Self::You,
            ActivityActor::System => Self::System,
            ActivityActor::Administrator => Self::Administrator,
        }
    }
}

/// One event in a Member's Security activity.
#[derive(Debug, Serialize, ToSchema)]
pub struct SecurityActivityEventResponse {
    pub id: i64,
    pub occurred_at: Timestamp,
    pub kind: AuditKindName,
    pub actor: ActivityActorName,
    pub details: AuditDetailsResponse,
}

impl From<ActivityEntry> for SecurityActivityEventResponse {
    fn from(entry: ActivityEntry) -> Self {
        Self {
            id: entry.id,
            occurred_at: entry.occurred_at,
            kind: entry.kind.into(),
            actor: entry.actor.into(),
            details: entry.details.into(),
        }
    }
}

/// A page of the signed-in Member's Security activity, newest first.
#[derive(Debug, Serialize, ToSchema)]
pub struct SecurityActivityResponse {
    pub events: Vec<SecurityActivityEventResponse>,
    /// Pass as `before_id` to load the next page; absent on the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub next_before_id: Option<i64>,
}

impl From<ActivityPage> for SecurityActivityResponse {
    fn from(page: ActivityPage) -> Self {
        Self {
            events: page.entries.into_iter().map(Into::into).collect(),
            next_before_id: page.next_before_id,
        }
    }
}

/// Query parameters for Security activity.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SecurityActivityQuery {
    #[serde(default)]
    pub limit: Option<i64>,
    /// Only events older than this one (the previous page's `next_before_id`).
    #[serde(default)]
    pub before_id: Option<i64>,
}

impl SecurityActivityQuery {
    /// The page size: default 20, within `[1, MAX_ACTIVITY_LIMIT]`.
    #[must_use]
    pub fn limit(&self) -> i64 {
        self.limit
            .unwrap_or(DEFAULT_ACTIVITY_LIMIT)
            .clamp(1, MAX_ACTIVITY_LIMIT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_is_capped_and_defaulted() {
        let query = |limit, offset| ListAuditEventsQuery {
            limit,
            offset,
            account_id: None,
            kind: None,
        };
        assert_eq!(query(None, None).paginated(), (50, 0));
        assert_eq!(query(Some(1000), Some(-5)).paginated(), (100, 0));
        assert_eq!(query(Some(0), Some(20)).paginated(), (1, 20));
    }

    #[test]
    fn every_kind_has_a_name_and_a_filter() {
        for kind in AuditKind::all() {
            let name = serde_json::to_value(AuditKindName::from(Some(*kind))).unwrap();
            assert_eq!(name, kind.as_str());
            let filter: AuditKindFilter = serde_json::from_value(name).unwrap();
            assert_eq!(AuditKind::from(filter), *kind);
        }
        assert_eq!(AuditKindName::from(None), AuditKindName::Other);
    }
}
