//! Use case for revoking refresh tokens.

use crate::application::ports::RefreshTokenRepository;
use crate::application::ports::TokenService;
use crate::domain::errors::DomainError;
use std::sync::Arc;

/// Use case for revoking refresh tokens.
///
/// This is used during logout to invalidate the current refresh token,
/// preventing it from being used to obtain new JWTs.
pub struct RevokeTokenUseCase {
    refresh_token_repository: Arc<dyn RefreshTokenRepository>,
    token_service: Arc<dyn TokenService>,
}

impl RevokeTokenUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        refresh_token_repository: Arc<dyn RefreshTokenRepository>,
        token_service: Arc<dyn TokenService>,
    ) -> Self {
        Self {
            refresh_token_repository,
            token_service,
        }
    }

    /// Revokes a refresh token.
    ///
    /// This operation is idempotent - if the token doesn't exist or is already
    /// revoked, it succeeds silently.
    ///
    /// # Arguments
    ///
    /// * `token` - The refresh token to revoke (plaintext)
    ///
    /// # Errors
    /// Returns an error if repository operations fail.
    #[tracing::instrument(name = "usecase.revoke_token", skip(self, token))]
    pub async fn execute(&self, token: &str) -> Result<(), DomainError> {
        let token_hash = self.token_service.hash_token(token);

        // Find the token - if it doesn't exist, that's fine (idempotent)
        if let Some(refresh_token) = self
            .refresh_token_repository
            .find_by_hash(&token_hash)
            .await?
        {
            // Only revoke if not already revoked
            if !refresh_token.is_revoked() {
                self.refresh_token_repository
                    .revoke(refresh_token.id())
                    .await?;
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::refresh_token_repository::MockRefreshTokenRepository;
    use crate::application::ports::outgoing::token_service::MockTokenService;
    use crate::domain::entities::RefreshToken;
    use crate::domain::value_objects::UserId;
    use chrono::{Duration, Utc};

    fn create_valid_refresh_token() -> RefreshToken {
        RefreshToken::new(
            UserId::new(),
            "hashed_test_token".to_string(),
            "device_fingerprint".to_string(),
            Utc::now() + Duration::days(30),
        )
    }

    fn create_mock_token_service() -> MockTokenService {
        let mut mock = MockTokenService::new();
        mock.expect_hash_token()
            .returning(|_| "hashed_test_token".to_string());
        mock
    }

    #[tokio::test]
    async fn test_revoke_existing_token() {
        let mut mock_repo = MockRefreshTokenRepository::new();
        mock_repo
            .expect_find_by_hash()
            .times(1)
            .returning(|_| Ok(Some(create_valid_refresh_token())));
        mock_repo.expect_revoke().times(1).returning(|_| Ok(()));

        let use_case =
            RevokeTokenUseCase::new(Arc::new(mock_repo), Arc::new(create_mock_token_service()));

        let result = use_case.execute("test_token").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_revoke_nonexistent_token() {
        let mut mock_repo = MockRefreshTokenRepository::new();
        mock_repo
            .expect_find_by_hash()
            .times(1)
            .returning(|_| Ok(None));
        // revoke should NOT be called
        mock_repo.expect_revoke().times(0);

        let use_case =
            RevokeTokenUseCase::new(Arc::new(mock_repo), Arc::new(create_mock_token_service()));

        let result = use_case.execute("nonexistent_token").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_revoke_already_revoked_token() {
        let mut mock_repo = MockRefreshTokenRepository::new();
        mock_repo.expect_find_by_hash().times(1).returning(|_| {
            let mut token = create_valid_refresh_token();
            token.revoke();
            Ok(Some(token))
        });
        // revoke should NOT be called since already revoked
        mock_repo.expect_revoke().times(0);

        let use_case =
            RevokeTokenUseCase::new(Arc::new(mock_repo), Arc::new(create_mock_token_service()));

        let result = use_case.execute("test_token").await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_revoke_repository_error() {
        let mut mock_repo = MockRefreshTokenRepository::new();
        mock_repo
            .expect_find_by_hash()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("Database error".into())));

        let use_case =
            RevokeTokenUseCase::new(Arc::new(mock_repo), Arc::new(create_mock_token_service()));

        let result = use_case.execute("test_token").await;
        assert!(result.is_err());
    }
}
