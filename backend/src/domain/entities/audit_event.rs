//! Audit events (OT-008): a record of a privileged or security-relevant
//! account action, stored in the same all-or-nothing step as the action.
//!
//! Every part is a closed type. Nothing free-form can be attached, so an event
//! can't carry an email, name, code, token or client address: people are
//! identified only by account.

use crate::domain::errors::DomainError;
use crate::domain::value_objects::{UserId, UserRole};

/// The kind of action an event records.
///
/// Adding a variant: the coverage guard `atomicity_tests` in
/// `tests/audit_log_integration.rs` must name the `#[sqlx::test]`s proving the
/// new kind is stored if and only if its action took effect, or the
/// integration tests stop compiling. Widen the `audit_events_kind_check`
/// constraint in a new migration too.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditKind {
    RoleChanged,
}

impl AuditKind {
    /// Every kind, in a stable order (for filters and the API description).
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::RoleChanged]
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RoleChanged => "role_changed",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Self::all().iter().copied().find(|k| k.as_str() == s)
    }
}

/// Who carried out the action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Actor {
    /// A signed-in account (the Member themselves, or staff).
    User(UserId),
    /// An automatic action (a background job, the token-reuse defence).
    System,
    /// The operator's bootstrap command (`just grant-admin`).
    Operator,
}

impl Actor {
    #[must_use]
    pub const fn kind_str(self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::System => "system",
            Self::Operator => "operator",
        }
    }

    /// The acting account, which only a [`Actor::User`] has.
    #[must_use]
    pub const fn user_id(self) -> Option<UserId> {
        match self {
            Self::User(id) => Some(id),
            Self::System | Self::Operator => None,
        }
    }
}

/// The only details an event may hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDetails {
    None,
    RoleChanged { from: UserRole, to: UserRole },
}

impl AuditDetails {
    /// Whether these details belong on an event of `kind`.
    const fn fits(self, kind: AuditKind) -> bool {
        matches!(
            (kind, self),
            (AuditKind::RoleChanged, Self::RoleChanged { .. })
        )
    }
}

/// One audited action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditEvent {
    kind: AuditKind,
    subject: UserId,
    actor: Actor,
    details: AuditDetails,
}

impl AuditEvent {
    /// Builds an event, checking that `details` belong to `kind`.
    ///
    /// # Errors
    /// [`DomainError::Validation`] when the details don't fit the kind.
    pub fn new(
        kind: AuditKind,
        subject: UserId,
        actor: Actor,
        details: AuditDetails,
    ) -> Result<Self, DomainError> {
        if !details.fits(kind) {
            return Err(DomainError::Validation(format!(
                "audit details {details:?} don't fit kind {}",
                kind.as_str()
            )));
        }
        Ok(Self {
            kind,
            subject,
            actor,
            details,
        })
    }

    /// A role change of `subject` from `from` to `to`.
    #[must_use]
    pub const fn role_changed(subject: UserId, actor: Actor, from: UserRole, to: UserRole) -> Self {
        Self {
            kind: AuditKind::RoleChanged,
            subject,
            actor,
            details: AuditDetails::RoleChanged { from, to },
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AuditKind {
        self.kind
    }

    #[must_use]
    pub const fn subject(&self) -> UserId {
        self.subject
    }

    #[must_use]
    pub const fn actor(&self) -> Actor {
        self.actor
    }

    #[must_use]
    pub const fn details(&self) -> AuditDetails {
        self.details
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_user_actor_has_an_account() {
        let id = UserId::new();
        assert_eq!(Actor::User(id).user_id(), Some(id));
        assert_eq!(Actor::System.user_id(), None);
        assert_eq!(Actor::Operator.user_id(), None);
        assert_eq!(Actor::User(id).kind_str(), "user");
        assert_eq!(Actor::System.kind_str(), "system");
        assert_eq!(Actor::Operator.kind_str(), "operator");
    }

    #[test]
    fn role_changed_details_only_fit_a_role_change() {
        let details = AuditDetails::RoleChanged {
            from: UserRole::User,
            to: UserRole::Admin,
        };
        let event = AuditEvent::new(
            AuditKind::RoleChanged,
            UserId::new(),
            Actor::System,
            details,
        )
        .unwrap();
        assert_eq!(event.details(), details);

        let err = AuditEvent::new(
            AuditKind::RoleChanged,
            UserId::new(),
            Actor::System,
            AuditDetails::None,
        )
        .unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[test]
    fn role_changed_constructor_records_both_roles() {
        let subject = UserId::new();
        let actor = Actor::User(UserId::new());
        let event = AuditEvent::role_changed(subject, actor, UserRole::User, UserRole::Admin);
        assert_eq!(event.kind(), AuditKind::RoleChanged);
        assert_eq!(event.subject(), subject);
        assert_eq!(event.actor(), actor);
        assert_eq!(
            event.details(),
            AuditDetails::RoleChanged {
                from: UserRole::User,
                to: UserRole::Admin
            }
        );
    }

    #[test]
    fn kinds_round_trip_through_their_names() {
        for kind in AuditKind::all() {
            assert_eq!(AuditKind::parse(kind.as_str()), Some(*kind));
        }
        assert_eq!(AuditKind::parse("unknown"), None);
    }
}
