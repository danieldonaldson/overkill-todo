use crate::domain::entities::{AuditEvent, User};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{UserId, UserRole};
use async_trait::async_trait;
use jiff::Timestamp;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionAuthorizationState {
    pub role: UserRole,
    pub tokens_valid_after: Option<Timestamp>,
}

/// Repository trait for user persistence operations.
///
/// This trait defines the contract for storing and retrieving users.
/// Implementations handle the actual database interaction.
///
/// # Implementors
///
/// - [`crate::infrastructure::persistence::postgres::PostgresUserRepository`]
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// Persists a new user to the database.
    async fn save(&self, user: &User) -> Result<(), DomainError>;

    /// [`Self::save`] and `event` in one transaction: the event exists if and
    /// only if the save took effect (OT-008).
    async fn save_audited(&self, user: &User, event: &AuditEvent) -> Result<(), DomainError>;

    /// Finds a user by their unique ID.
    async fn find_by_id(&self, id: &UserId) -> Result<Option<User>, DomainError>;

    /// Finds a user by their email address.
    async fn find_by_email(&self, email: &str) -> Result<Option<User>, DomainError>;

    /// Finds a user by their email address, but only if email is verified.
    ///
    /// Used for OAuth hybrid auto-linking - only link to verified email accounts.
    async fn find_by_verified_email(&self, email: &str) -> Result<Option<User>, DomainError>;

    /// Retrieves all users with pagination.
    async fn find_all(&self, limit: i64, offset: i64) -> Result<Vec<User>, DomainError>;

    /// Counts the users [`Self::find_all`] pages through (not deleted).
    async fn count_all(&self) -> Result<i64, DomainError>;

    /// Atomically persists a role change and invalidates all existing session
    /// tokens. When `removing_super_admin` is true, the implementation must
    /// serialize the last-super-admin check with other role changes and reject
    /// the update if it would remove the final active super-admin.
    ///
    /// `event` is stored in the same transaction: it exists if and only if the
    /// role change took effect (OT-008).
    async fn save_role_change(
        &self,
        user: &User,
        removing_super_admin: bool,
        event: &AuditEvent,
    ) -> Result<(), DomainError>;

    /// Reads the persisted role and revocation cutoff used to validate a
    /// session's authorization state.
    async fn session_authorization_state(
        &self,
        id: &UserId,
    ) -> Result<Option<SessionAuthorizationState>, DomainError>;

    /// Marks a user's email as verified.
    async fn mark_email_verified(&self, id: &UserId) -> Result<(), DomainError>;

    /// Updates a user's password hash, storing `event` in the same transaction.
    async fn update_password(
        &self,
        id: &UserId,
        password_hash: &str,
        event: &AuditEvent,
    ) -> Result<(), DomainError>;

    /// Moves the user's session cutoff (`tokens_valid_after`) forward to
    /// `cutoff`, never back. Access tokens issued at or before that second are
    /// rejected by routes that check session freshness.
    async fn revoke_sessions_issued_before(
        &self,
        id: &UserId,
        cutoff: Timestamp,
    ) -> Result<(), DomainError>;
}
