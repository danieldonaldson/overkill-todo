//! Use case for retrieving the current authenticated user.

use crate::application::ports::{OAuthAccountRepository, UserRepository};
use crate::domain::entities::{OAuthAccount, OAuthProvider, User};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use std::sync::Arc;

/// The current user and the OAuth providers linked to their account.
#[derive(Debug)]
pub struct CurrentUser {
    pub user: User,
    pub oauth_providers: Vec<OAuthProvider>,
}

/// Use case for retrieving the current authenticated user.
pub struct GetCurrentUserUseCase {
    user_repository: Arc<dyn UserRepository>,
    oauth_account_repository: Arc<dyn OAuthAccountRepository>,
}

impl GetCurrentUserUseCase {
    /// Creates a new instance with the given repositories.
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        oauth_account_repository: Arc<dyn OAuthAccountRepository>,
    ) -> Self {
        Self {
            user_repository,
            oauth_account_repository,
        }
    }

    /// Retrieves the user by their ID, with their linked OAuth providers.
    ///
    /// Returns `None` if no user exists with the given ID.
    ///
    /// # Errors
    /// Returns an error if either repository lookup fails.
    #[tracing::instrument(name = "usecase.get_current_user", skip_all)]
    pub async fn execute(&self, user_id: &UserId) -> Result<Option<CurrentUser>, DomainError> {
        // Both lookups key off the same id and are independent, so run them
        // concurrently: this is a hot (per-page-load) path.
        let (user, oauth_accounts) = tokio::try_join!(
            self.user_repository.find_by_id(user_id),
            self.oauth_account_repository.find_by_user_id(user_id),
        )?;

        Ok(user.map(|user| CurrentUser {
            user,
            oauth_providers: oauth_accounts.iter().map(OAuthAccount::provider).collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::oauth_account_repository::MockOAuthAccountRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::value_objects::Email;

    fn no_oauth_accounts() -> Arc<MockOAuthAccountRepository> {
        let mut oauth = MockOAuthAccountRepository::new();
        oauth.expect_find_by_user_id().returning(|_| Ok(Vec::new()));
        Arc::new(oauth)
    }

    #[tokio::test]
    async fn test_get_current_user_found() {
        let existing_user = User::new(
            Email::new("test@example.com").unwrap(),
            Some("hashed_password".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );
        let user_id = *existing_user.id();

        let mut mock_repo = MockUserRepository::new();
        let user_clone = existing_user.clone();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case = GetCurrentUserUseCase::new(Arc::new(mock_repo), no_oauth_accounts());
        let result = use_case.execute(&user_id).await;

        assert!(result.is_ok());
        let user = result.unwrap();
        assert!(user.is_some());
        assert_eq!(user.unwrap().user.email().as_str(), "test@example.com");
    }

    #[tokio::test]
    async fn test_get_current_user_not_found() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(|_| Ok(None));

        let use_case = GetCurrentUserUseCase::new(Arc::new(mock_repo), no_oauth_accounts());
        let user_id = UserId::new();
        let result = use_case.execute(&user_id).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_get_current_user_repository_error() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("Database error".into())));

        let use_case = GetCurrentUserUseCase::new(Arc::new(mock_repo), no_oauth_accounts());
        let user_id = UserId::new();
        let result = use_case.execute(&user_id).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_current_user_includes_linked_providers() {
        let existing_user = User::new(
            Email::new("test@example.com").unwrap(),
            None,
            "John".to_string(),
            "Doe".to_string(),
        );
        let user_id = *existing_user.id();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(existing_user.clone())));
        let mut oauth = MockOAuthAccountRepository::new();
        oauth
            .expect_find_by_user_id()
            .withf(move |id| *id == user_id)
            .returning(move |_| {
                Ok(vec![OAuthAccount::new(
                    user_id,
                    OAuthProvider::Google,
                    "google-sub".into(),
                    None,
                )])
            });

        let use_case = GetCurrentUserUseCase::new(Arc::new(mock_repo), Arc::new(oauth));
        let current = use_case.execute(&user_id).await.unwrap().unwrap();

        assert_eq!(current.oauth_providers, vec![OAuthProvider::Google]);
    }
}
