//! Use case for cancelling a pending account-deletion request.
//!
//! Available during the grace period set by [`RequestAccountDeletionUseCase`].
//! Clears `deletion_scheduled_at` so [`PurgeScheduledAccountsUseCase`] no longer
//! picks the account up.

use std::sync::Arc;

use crate::application::ports::UserRepository;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{AccountStatus, UserId};

/// Use case for cancelling a scheduled account deletion.
pub struct CancelAccountDeletionUseCase {
    user_repository: Arc<dyn UserRepository>,
}

impl CancelAccountDeletionUseCase {
    #[must_use]
    pub fn new(user_repository: Arc<dyn UserRepository>) -> Self {
        Self { user_repository }
    }

    /// Cancels a pending deletion for the authenticated user.
    ///
    /// Idempotent: cancelling when nothing is scheduled is a no-op success.
    /// Refuses once the account is already anonymized (past the grace period).
    ///
    /// # Errors
    /// - `NotFound` if the user doesn't exist.
    /// - `Validation` if the account is already deleted/anonymized.
    #[tracing::instrument(name = "usecase.cancel_account_deletion", skip_all, fields(user_id = %user_id))]
    pub async fn execute(&self, user_id: &UserId) -> Result<(), DomainError> {
        let mut user = self
            .user_repository
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Nothing scheduled: succeed without a write (and a version bump).
        if user.status() == AccountStatus::Active {
            return Ok(());
        }

        // Refuses an already-deleted account.
        user.cancel_deletion()?;
        self.user_repository.save(&user).await?;

        tracing::info!(user_id = %user_id, "Scheduled account deletion cancelled");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::{User, UserBuilder};
    use chrono::Utc;

    fn make_user(status: AccountStatus) -> User {
        UserBuilder::new()
            .password_hash(Some("hash"))
            .status(status)
            .build()
    }

    fn pending() -> AccountStatus {
        AccountStatus::PendingDeletion {
            scheduled_for: Utc::now() + chrono::Duration::days(14),
        }
    }

    #[tokio::test]
    async fn cancels_a_pending_deletion() {
        let user = make_user(pending());
        let user_id = *user.id();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        repo.expect_save()
            .times(1)
            .withf(|u| u.deletion_scheduled_at().is_none())
            .returning(|_| Ok(()));

        let uc = CancelAccountDeletionUseCase::new(Arc::new(repo));
        assert!(uc.execute(&user_id).await.is_ok());
    }

    #[tokio::test]
    async fn nothing_scheduled_is_noop() {
        let user = make_user(AccountStatus::Active);
        let user_id = *user.id();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        repo.expect_save().never();

        let uc = CancelAccountDeletionUseCase::new(Arc::new(repo));
        assert!(uc.execute(&user_id).await.is_ok());
    }

    #[tokio::test]
    async fn already_deleted_is_rejected() {
        let user = make_user(AccountStatus::Deleted {
            deleted_at: Utc::now(),
        });
        let user_id = *user.id();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        repo.expect_save().never();

        let uc = CancelAccountDeletionUseCase::new(Arc::new(repo));
        let err = uc.execute(&user_id).await.unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }
}
