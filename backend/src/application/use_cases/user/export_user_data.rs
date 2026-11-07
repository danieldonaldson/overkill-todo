//! Use case for exporting a user's personal data.
//!
//! Assembles the personal data the system holds about a user — their profile and
//! linked OAuth accounts — for the GDPR data-portability right. Returns the
//! domain entities; the handler maps them to the machine-readable response DTO.

use std::sync::Arc;

use crate::application::ports::{OAuthAccountRepository, UserRepository};
use crate::domain::entities::{OAuthAccount, User};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;

/// Use case for exporting the authenticated user's personal data.
pub struct ExportUserDataUseCase {
    user_repository: Arc<dyn UserRepository>,
    oauth_account_repository: Arc<dyn OAuthAccountRepository>,
}

impl ExportUserDataUseCase {
    #[must_use]
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        oauth_account_repository: Arc<dyn OAuthAccountRepository>,
    ) -> Self {
        Self {
            user_repository,
            oauth_account_repository,
        }
    }

    /// Gathers the user profile and their linked OAuth accounts.
    ///
    /// # Errors
    /// - `NotFound` if the user doesn't exist.
    /// - `Persistence` on a repository failure.
    #[tracing::instrument(name = "usecase.export_user_data", skip(self), fields(user_id = %user_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
    ) -> Result<(User, Vec<OAuthAccount>), DomainError> {
        let user = self
            .user_repository
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        let oauth_accounts = self
            .oauth_account_repository
            .find_by_user_id(user_id)
            .await?;

        tracing::info!(user_id = %user_id, oauth_count = oauth_accounts.len(), "User data exported");
        Ok((user, oauth_accounts))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::oauth_account_repository::MockOAuthAccountRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::oauth_account::OAuthProvider;
    use crate::domain::entities::{User, UserBuilder};

    fn make_user() -> User {
        UserBuilder::new().password_hash(Some("hash")).build()
    }

    #[tokio::test]
    async fn exports_user_and_oauth_accounts() {
        let user = make_user();
        let user_id = *user.id();
        let account = OAuthAccount::new(
            user_id,
            OAuthProvider::Google,
            "sub-123".to_string(),
            Some("test@example.com".to_string()),
        );

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user.clone())));

        let mut oauth_repo = MockOAuthAccountRepository::new();
        oauth_repo
            .expect_find_by_user_id()
            .times(1)
            .returning(move |_| Ok(vec![account.clone()]));

        let uc = ExportUserDataUseCase::new(Arc::new(user_repo), Arc::new(oauth_repo));
        let (exported_user, accounts) = uc.execute(&user_id).await.unwrap();
        assert_eq!(exported_user.email().as_str(), "test@example.com");
        assert_eq!(accounts.len(), 1);
    }

    #[tokio::test]
    async fn missing_user_is_not_found() {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_id().returning(|_| Ok(None));

        let uc = ExportUserDataUseCase::new(
            Arc::new(user_repo),
            Arc::new(MockOAuthAccountRepository::new()),
        );
        let err = uc.execute(&UserId::new()).await.unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }
}
