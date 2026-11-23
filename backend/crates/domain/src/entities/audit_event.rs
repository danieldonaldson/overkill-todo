//! Audit events (OT-008): a record of a privileged or security-relevant
//! account action, stored in the same all-or-nothing step as the action.
//!
//! Every part is a closed type. Nothing free-form can be attached, so an event
//! can't carry an email, name, code, token or client address: people are
//! identified only by account.

use crate::domain::entities::OAuthProvider;
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
    PasswordChanged,
    /// A first password on an account that had none (OAuth-only).
    PasswordSet,
    /// A password replaced with a reset code.
    PasswordReset,
    SignInMethodLinked,
    SignInMethodUnlinked,
    DeletionRequested,
    DeletionCancelled,
    /// The purge anonymised the account at the end of its grace period.
    AccountDeleted,
    DataExported,
    /// Every session signed out because a revoked session token was reused.
    SessionsRevokedAfterReuse,
}

impl AuditKind {
    /// Every kind, in a stable order (for filters and the API description).
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::RoleChanged,
            Self::PasswordChanged,
            Self::PasswordSet,
            Self::PasswordReset,
            Self::SignInMethodLinked,
            Self::SignInMethodUnlinked,
            Self::DeletionRequested,
            Self::DeletionCancelled,
            Self::AccountDeleted,
            Self::DataExported,
            Self::SessionsRevokedAfterReuse,
        ]
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::RoleChanged => "role_changed",
            Self::PasswordChanged => "password_changed",
            Self::PasswordSet => "password_set",
            Self::PasswordReset => "password_reset",
            Self::SignInMethodLinked => "sign_in_method_linked",
            Self::SignInMethodUnlinked => "sign_in_method_unlinked",
            Self::DeletionRequested => "deletion_requested",
            Self::DeletionCancelled => "deletion_cancelled",
            Self::AccountDeleted => "account_deleted",
            Self::DataExported => "data_exported",
            Self::SessionsRevokedAfterReuse => "sessions_revoked_after_reuse",
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
    SignInMethod { provider: OAuthProvider },
}

impl AuditDetails {
    /// Whether these details belong on an event of `kind`.
    const fn fits(self, kind: AuditKind) -> bool {
        match kind {
            AuditKind::RoleChanged => matches!(self, Self::RoleChanged { .. }),
            AuditKind::SignInMethodLinked | AuditKind::SignInMethodUnlinked => {
                matches!(self, Self::SignInMethod { .. })
            }
            AuditKind::PasswordChanged
            | AuditKind::PasswordSet
            | AuditKind::PasswordReset
            | AuditKind::DeletionRequested
            | AuditKind::DeletionCancelled
            | AuditKind::AccountDeleted
            | AuditKind::DataExported
            | AuditKind::SessionsRevokedAfterReuse => matches!(self, Self::None),
        }
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

    /// A kind with no details, done by the account itself.
    const fn by_self(kind: AuditKind, subject: UserId) -> Self {
        Self {
            kind,
            subject,
            actor: Actor::User(subject),
            details: AuditDetails::None,
        }
    }

    /// The Member changed their own password.
    #[must_use]
    pub const fn password_changed(subject: UserId) -> Self {
        Self::by_self(AuditKind::PasswordChanged, subject)
    }

    /// The Member set a first password on an account without one.
    #[must_use]
    pub const fn password_set(subject: UserId) -> Self {
        Self::by_self(AuditKind::PasswordSet, subject)
    }

    /// The account's password was reset with a code sent to its email, so the
    /// account itself is the actor.
    #[must_use]
    pub const fn password_reset(subject: UserId) -> Self {
        Self::by_self(AuditKind::PasswordReset, subject)
    }

    /// `provider` was linked to the account as a way to sign in.
    #[must_use]
    pub const fn sign_in_method_linked(subject: UserId, provider: OAuthProvider) -> Self {
        Self {
            details: AuditDetails::SignInMethod { provider },
            ..Self::by_self(AuditKind::SignInMethodLinked, subject)
        }
    }

    /// `provider` was unlinked from the account.
    #[must_use]
    pub const fn sign_in_method_unlinked(subject: UserId, provider: OAuthProvider) -> Self {
        Self {
            details: AuditDetails::SignInMethod { provider },
            ..Self::by_self(AuditKind::SignInMethodUnlinked, subject)
        }
    }

    #[must_use]
    pub const fn deletion_requested(subject: UserId) -> Self {
        Self::by_self(AuditKind::DeletionRequested, subject)
    }

    #[must_use]
    pub const fn deletion_cancelled(subject: UserId) -> Self {
        Self::by_self(AuditKind::DeletionCancelled, subject)
    }

    /// The purge deleted (anonymised) the account.
    #[must_use]
    pub const fn account_deleted(subject: UserId) -> Self {
        Self {
            actor: Actor::System,
            ..Self::by_self(AuditKind::AccountDeleted, subject)
        }
    }

    #[must_use]
    pub const fn data_exported(subject: UserId) -> Self {
        Self::by_self(AuditKind::DataExported, subject)
    }

    /// The token-reuse defence signed out every session of the account.
    #[must_use]
    pub const fn sessions_revoked_after_reuse(subject: UserId) -> Self {
        Self {
            actor: Actor::System,
            ..Self::by_self(AuditKind::SessionsRevokedAfterReuse, subject)
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

    /// Every constructor builds an event `new` would accept.
    #[test]
    fn every_constructor_fits_its_kind() {
        let id = UserId::new();
        let google = OAuthProvider::Google;
        for event in [
            AuditEvent::role_changed(id, Actor::Operator, UserRole::User, UserRole::Admin),
            AuditEvent::password_changed(id),
            AuditEvent::password_set(id),
            AuditEvent::password_reset(id),
            AuditEvent::sign_in_method_linked(id, google),
            AuditEvent::sign_in_method_unlinked(id, google),
            AuditEvent::deletion_requested(id),
            AuditEvent::deletion_cancelled(id),
            AuditEvent::account_deleted(id),
            AuditEvent::data_exported(id),
            AuditEvent::sessions_revoked_after_reuse(id),
        ] {
            let rebuilt = AuditEvent::new(
                event.kind(),
                event.subject(),
                event.actor(),
                event.details(),
            );
            assert_eq!(rebuilt.unwrap(), event);
        }
    }

    #[test]
    fn automatic_actions_have_the_system_as_actor() {
        let id = UserId::new();
        assert_eq!(AuditEvent::account_deleted(id).actor(), Actor::System);
        assert_eq!(
            AuditEvent::sessions_revoked_after_reuse(id).actor(),
            Actor::System
        );
        assert_eq!(AuditEvent::password_reset(id).actor(), Actor::User(id));
    }

    #[test]
    fn sign_in_details_only_fit_sign_in_kinds() {
        let details = AuditDetails::SignInMethod {
            provider: OAuthProvider::Google,
        };
        for kind in AuditKind::all() {
            let fits = AuditEvent::new(*kind, UserId::new(), Actor::System, details).is_ok();
            assert_eq!(
                fits,
                matches!(
                    kind,
                    AuditKind::SignInMethodLinked | AuditKind::SignInMethodUnlinked
                ),
                "{}",
                kind.as_str()
            );
        }
        assert!(
            AuditEvent::new(
                AuditKind::PasswordChanged,
                UserId::new(),
                Actor::System,
                AuditDetails::None
            )
            .is_ok()
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
