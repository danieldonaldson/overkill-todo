//! Use case for self-service account-deletion requests.
//!
//! A user requests deletion → `deletion_scheduled_at` is set ~14 days out (the
//! grace period) → [`PurgeScheduledAccountsUseCase`] finalizes it after the grace
//! period elapses (external teardown + anonymize). This use case only schedules;
//! it never tears anything down synchronously, so a mistaken request is fully
//! recoverable via [`CancelAccountDeletionUseCase`] during the grace window.

use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::application::ports::{PasswordHasher, UserRepository};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;

/// Use case for requesting account deletion (soft, grace-period-gated).
pub struct RequestAccountDeletionUseCase {
    user_repository: Arc<dyn UserRepository>,
    password_hasher: Arc<dyn PasswordHasher>,
}

impl RequestAccountDeletionUseCase {
    #[must_use]
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        password_hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            user_repository,
            password_hasher,
        }
    }

    /// Schedules deletion for the authenticated user.
    ///
    /// Re-authentication: password users must supply their current password
    /// (defence against a hijacked session triggering deletion); OAuth-only users
    /// have no password to verify and so are gated only by the authenticated
    /// session. Idempotent — re-requesting while already scheduled returns the
    /// existing date without moving it.
    ///
    /// # Errors
    /// - `NotFound` if the user doesn't exist.
    /// - `Validation` if a password user omitted their password.
    /// - `Authorization` if the supplied password is wrong.
    #[tracing::instrument(name = "usecase.request_account_deletion", skip(self, password), fields(user_id = %user_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        password: Option<&str>,
    ) -> Result<DateTime<Utc>, DomainError> {
        let mut user = self
            .user_repository
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Idempotent: already scheduled → return the existing date.
        if let Some(scheduled_at) = user.deletion_scheduled_at() {
            return Ok(scheduled_at);
        }

        // Re-authenticate password users.
        if let Some(password_hash) = user.password_hash() {
            let password = password.ok_or_else(|| {
                DomainError::Validation("Password is required to delete your account".into())
            })?;
            if !self.password_hasher.verify(password, password_hash).await? {
                return Err(DomainError::Authorization("Password is incorrect".into()));
            }
        }

        user.schedule_deletion();
        let scheduled_at = user
            .deletion_scheduled_at()
            .ok_or_else(|| DomainError::Persistence("deletion schedule not set".into()))?;
        self.user_repository.save(&user).await?;

        tracing::info!(user_id = %user_id, %scheduled_at, "Account deletion scheduled");
        Ok(scheduled_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::{User, UserBuilder};

    fn make_user(password_hash: Option<&str>, scheduled: bool) -> User {
        let mut user = UserBuilder::new().password_hash(password_hash).build();
        if scheduled {
            user.schedule_deletion();
        }
        user
    }

    #[tokio::test]
    async fn password_user_schedules_with_correct_password() {
        let user = make_user(Some("hash"), false);
        let user_id = *user.id();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user.clone())));
        repo.expect_save().times(1).returning(|_| Ok(()));

        let mut hasher = MockPasswordHasher::new();
        hasher.expect_verify().times(1).returning(|_, _| Ok(true));

        let uc = RequestAccountDeletionUseCase::new(Arc::new(repo), Arc::new(hasher));
        let scheduled = uc.execute(&user_id, Some("pw")).await.unwrap();
        assert!(scheduled > Utc::now());
    }

    #[tokio::test]
    async fn password_user_requires_password() {
        let user = make_user(Some("hash"), false);
        let user_id = *user.id();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        repo.expect_save().never();

        let uc =
            RequestAccountDeletionUseCase::new(Arc::new(repo), Arc::new(MockPasswordHasher::new()));
        let err = uc.execute(&user_id, None).await.unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[tokio::test]
    async fn wrong_password_is_rejected() {
        let user = make_user(Some("hash"), false);
        let user_id = *user.id();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        repo.expect_save().never();

        let mut hasher = MockPasswordHasher::new();
        hasher.expect_verify().returning(|_, _| Ok(false));

        let uc = RequestAccountDeletionUseCase::new(Arc::new(repo), Arc::new(hasher));
        let err = uc.execute(&user_id, Some("bad")).await.unwrap_err();
        assert!(matches!(err, DomainError::Authorization(_)));
    }

    #[tokio::test]
    async fn oauth_only_user_needs_no_password() {
        let user = make_user(None, false);
        let user_id = *user.id();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        repo.expect_save().times(1).returning(|_| Ok(()));

        let uc =
            RequestAccountDeletionUseCase::new(Arc::new(repo), Arc::new(MockPasswordHasher::new()));
        assert!(uc.execute(&user_id, None).await.is_ok());
    }

    #[tokio::test]
    async fn already_scheduled_is_idempotent() {
        let user = make_user(Some("hash"), true);
        let user_id = *user.id();
        let existing = user.deletion_scheduled_at().unwrap();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        // No re-auth, no save when already scheduled.
        repo.expect_save().never();

        let uc =
            RequestAccountDeletionUseCase::new(Arc::new(repo), Arc::new(MockPasswordHasher::new()));
        assert_eq!(uc.execute(&user_id, None).await.unwrap(), existing);
    }

    #[tokio::test]
    async fn missing_user_is_not_found() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id().returning(|_| Ok(None));

        let uc =
            RequestAccountDeletionUseCase::new(Arc::new(repo), Arc::new(MockPasswordHasher::new()));
        let err = uc.execute(&UserId::new(), None).await.unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }
}
