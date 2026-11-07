//! Account-lifecycle repository port (driven port).
//!
//! Split out of [`UserRepository`](super::user_repository::UserRepository):
//! that trait mixed plain user CRUD with cross-aggregate *orchestration* — the GDPR
//! account-deletion flow spans `users`, `refresh_tokens`, `user_oauth_accounts`, and
//! the verification/reset code tables in a single transaction. Keeping those
//! multi-table operations on their own port stops the user trait from being an
//! 11-method god-trait and lets a deletion worker depend only on what it uses.
//!
//! ## Why no generic unit-of-work abstraction (decided against)
//!
//! A transaction-threading "unit of work" was considered and rejected as a misfit:
//! [`anonymize_account`](AccountLifecycleRepository::anonymize_account) — the only
//! step needing DB atomicity — already runs its whole multi-table scrub in one
//! transaction inside the adapter. The wider deletion flow
//! ([`PurgeScheduledAccountsUseCase`](crate::application::use_cases::PurgeScheduledAccountsUseCase):
//! tear down external resources → anonymize) spans external calls
//! that cannot join a DB transaction, so it is — correctly — an ordered, idempotent,
//! retry-on-next-pass saga guarded by `ON DELETE RESTRICT` FKs, not a unit of work.
//! Threading an sqlx `Transaction` through these domain ports would add an
//! abstraction that fights both sqlx and mockall for no correctness gain.

use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use async_trait::async_trait;

/// Cross-aggregate account-deletion / anonymization operations.
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
#[async_trait]
pub trait AccountLifecycleRepository: Send + Sync {
    /// Finds all users whose deletion grace period has expired.
    ///
    /// Returns users where `deletion_scheduled_at` is in the past and `deleted_at`
    /// is NULL (not yet processed).
    async fn find_ready_for_deletion(&self) -> Result<Vec<User>, DomainError>;

    /// Hard-deletes a user row by ID.
    async fn delete(&self, id: &UserId) -> Result<(), DomainError>;

    /// Anonymizes a user and deletes their auth-related data.
    ///
    /// Performs the data-scrub half of account deletion in one transaction:
    /// 1. Updates the user record with anonymized data
    /// 2. Deletes refresh tokens
    /// 3. Deletes OAuth accounts
    /// 4. Deletes email verification codes
    /// 5. Deletes password reset codes
    ///
    /// It does **not** touch rows that hold handles to live external resources;
    /// those must be torn down by their own use cases first. The orchestration
    /// that sequences teardown-then-anonymize lives in
    /// [`PurgeScheduledAccountsUseCase`](crate::application::use_cases::PurgeScheduledAccountsUseCase).
    async fn anonymize_account(&self, user: &User) -> Result<(), DomainError>;
}
