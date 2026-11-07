//! OAuth unlink use case - removes OAuth provider from user account.

use std::sync::Arc;

use crate::application::ports::OAuthAccountRepository;
use crate::application::ports::UserRepository;
use crate::domain::entities::OAuthProvider;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;

/// OAuth unlink use case - removes OAuth provider link from account.
///
/// Includes lockout protection: prevents unlinking if it would leave
/// the user with no way to sign in.
pub struct OAuthUnlinkUseCase {
    user_repo: Arc<dyn UserRepository>,
    oauth_repo: Arc<dyn OAuthAccountRepository>,
}

impl OAuthUnlinkUseCase {
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        oauth_repo: Arc<dyn OAuthAccountRepository>,
    ) -> Self {
        Self {
            user_repo,
            oauth_repo,
        }
    }

    /// Execute the unlink operation.
    ///
    /// # Errors
    ///
    /// - `NotFound` - User or OAuth link not found
    /// - `Validation` - Cannot unlink last sign-in method
    #[tracing::instrument(
        name = "usecase.oauth_unlink",
        skip(self),
        fields(user_id = %user_id, provider = %provider.as_str())
    )]
    pub async fn execute(
        &self,
        user_id: &UserId,
        provider: OAuthProvider,
    ) -> Result<(), DomainError> {
        // Get user to check if they have a password
        let user = self
            .user_repo
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Count OAuth accounts
        let oauth_count = self.oauth_repo.count_by_user_id(user_id).await?;

        // Check if unlinking would leave user with no auth methods
        // User needs at least one of: password OR one remaining OAuth provider
        let has_password = user.has_password();

        if !has_password && oauth_count <= 1 {
            tracing::warn!(
                user_id = %user_id,
                provider = %provider.as_str(),
                has_password = has_password,
                oauth_count = oauth_count,
                "Attempted to unlink last sign-in method"
            );
            return Err(DomainError::Validation(
                "Cannot unlink your last sign-in method. Set a password first.".into(),
            ));
        }

        // Perform unlink
        let deleted = self
            .oauth_repo
            .delete_by_user_and_provider(user_id, provider)
            .await?;

        if !deleted {
            return Err(DomainError::NotFound("OAuth provider not linked".into()));
        }

        tracing::info!(
            user_id = %user_id,
            provider = %provider.as_str(),
            "Unlinked OAuth account"
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::oauth_account_repository::MockOAuthAccountRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::User;
    use crate::domain::value_objects::Email;

    fn create_test_user(has_password: bool) -> User {
        User::new(
            Email::new("test@example.com").unwrap(),
            if has_password {
                Some("password_hash".to_string())
            } else {
                None
            },
            "Test".to_string(),
            "User".to_string(),
        )
    }

    #[tokio::test]
    async fn test_unlink_success_with_password() {
        let user = create_test_user(true);
        let user_id = *user.id();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_oauth_repo = MockOAuthAccountRepository::new();
        mock_oauth_repo
            .expect_count_by_user_id()
            .returning(|_| Ok(1));
        mock_oauth_repo
            .expect_delete_by_user_and_provider()
            .returning(|_, _| Ok(true));

        let use_case = OAuthUnlinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case.execute(&user_id, OAuthProvider::Google).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_unlink_success_with_multiple_oauth() {
        let user = create_test_user(false); // No password
        let user_id = *user.id();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_oauth_repo = MockOAuthAccountRepository::new();
        mock_oauth_repo
            .expect_count_by_user_id()
            .returning(|_| Ok(2)); // Has 2 OAuth providers
        mock_oauth_repo
            .expect_delete_by_user_and_provider()
            .returning(|_, _| Ok(true));

        let use_case = OAuthUnlinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case.execute(&user_id, OAuthProvider::Google).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_unlink_blocked_last_auth_method() {
        let user = create_test_user(false); // No password
        let user_id = *user.id();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_oauth_repo = MockOAuthAccountRepository::new();
        mock_oauth_repo
            .expect_count_by_user_id()
            .returning(|_| Ok(1)); // Only 1 OAuth provider

        let use_case = OAuthUnlinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case.execute(&user_id, OAuthProvider::Google).await;
        assert!(matches!(result, Err(DomainError::Validation(_))));
    }

    #[tokio::test]
    async fn test_unlink_user_not_found() {
        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo.expect_find_by_id().returning(|_| Ok(None));

        let mock_oauth_repo = MockOAuthAccountRepository::new();

        let use_case = OAuthUnlinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case
            .execute(&UserId::new(), OAuthProvider::Google)
            .await;
        assert!(matches!(result, Err(DomainError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_unlink_provider_not_linked() {
        let user = create_test_user(true);
        let user_id = *user.id();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_oauth_repo = MockOAuthAccountRepository::new();
        mock_oauth_repo
            .expect_count_by_user_id()
            .returning(|_| Ok(1));
        mock_oauth_repo
            .expect_delete_by_user_and_provider()
            .returning(|_, _| Ok(false)); // Provider wasn't linked

        let use_case = OAuthUnlinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_oauth_repo));

        let result = use_case.execute(&user_id, OAuthProvider::Google).await;
        assert!(matches!(result, Err(DomainError::NotFound(_))));
    }
}
