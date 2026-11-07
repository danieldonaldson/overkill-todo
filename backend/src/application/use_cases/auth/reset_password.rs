//! Use case for resetting a user's password.

use crate::application::ports::UserRepository;
use crate::application::ports::{JwtService, PasswordResetRepository, RefreshTokenRepository};
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use std::sync::Arc;

/// Use case for resetting a user's password.
///
/// Validates the reset token (from `validate_reset_code`) and updates the password.
/// Returns the user for auto-login functionality.
pub struct ResetPasswordUseCase {
    user_repo: Arc<dyn UserRepository>,
    password_reset_repo: Arc<dyn PasswordResetRepository>,
    refresh_token_repo: Arc<dyn RefreshTokenRepository>,
    jwt_service: Arc<dyn JwtService>,
}

impl ResetPasswordUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        password_reset_repo: Arc<dyn PasswordResetRepository>,
        refresh_token_repo: Arc<dyn RefreshTokenRepository>,
        jwt_service: Arc<dyn JwtService>,
    ) -> Self {
        Self {
            user_repo,
            password_reset_repo,
            refresh_token_repo,
            jwt_service,
        }
    }

    /// Resets the user's password using the reset token.
    ///
    /// # Arguments
    ///
    /// * `reset_token` - The JWT token from `validate_reset_code`
    /// * `new_password_hash` - The new password hash (already hashed by the handler)
    ///
    /// # Returns
    ///
    /// The user whose password was reset (for auto-login)
    ///
    /// # Errors
    ///
    /// - `Authorization` if the token is invalid or expired
    /// - `NotFound` if the user doesn't exist
    #[tracing::instrument(
        name = "usecase.reset_password",
        skip(self, reset_token, new_password_hash)
    )]
    pub async fn execute(
        &self,
        reset_token: &str,
        new_password_hash: &str,
    ) -> Result<User, DomainError> {
        // Validate and decode the reset token, recovering the reset-code row it
        // is bound to.
        let (user_id, reset_code_id) = self.jwt_service.validate_reset_token(reset_token)?;

        // Atomically consume the reset-code row before touching the password.
        // If no row is consumed the token has already been used (or was
        // superseded by a newer code), so the reset must be rejected — this is
        // what makes the token single-use rather than replayable until expiry.
        let consumed = self
            .password_reset_repo
            .consume(&user_id, &reset_code_id)
            .await?;

        if !consumed {
            return Err(DomainError::Authorization(
                "Reset token has already been used or is no longer valid".into(),
            ));
        }

        // Find the user by ID from token
        let user = self
            .user_repo
            .find_by_id(&user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Update the password
        self.user_repo
            .update_password(user.id(), new_password_hash)
            .await?;

        // Revoke every existing refresh token for this user. Password reset is the
        // canonical account-recovery action — the user may be resetting *because*
        // a session was stolen — so any pre-existing session must be evicted. The
        // handler mints a fresh session for the resetting device *after* this
        // returns, so the new token is never caught by this sweep.
        let revoked = self
            .refresh_token_repo
            .revoke_all_for_user(&user_id)
            .await?;
        tracing::info!(user_id = %user.id(), revoked, "Revoked existing sessions on password reset");

        tracing::info!(user_id = %user.id(), "Password reset successfully");

        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::jwt_service::MockJwtService;
    use crate::application::ports::outgoing::password_reset_repository::MockPasswordResetRepository;
    use crate::application::ports::outgoing::refresh_token_repository::MockRefreshTokenRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::value_objects::{Email, ResetCodeId, UserId};

    fn create_test_user() -> User {
        let mut user = User::new(
            Email::new("test@example.com").unwrap(),
            Some("old_password_hash".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );
        user.mark_email_verified();
        user
    }

    #[tokio::test]
    async fn test_reset_password_success() {
        let user = create_test_user();
        let user_id = *user.id();
        let reset_code_id = ResetCodeId::new();

        let mut mock_jwt = MockJwtService::new();
        mock_jwt
            .expect_validate_reset_token()
            .returning(move |_| Ok((user_id, reset_code_id)));

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));
        mock_user_repo
            .expect_update_password()
            .returning(|_, _| Ok(()));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo.expect_consume().returning(|_, _| Ok(true));

        // A successful reset must revoke every pre-existing session for the user.
        let mut mock_refresh_repo = MockRefreshTokenRepository::new();
        mock_refresh_repo
            .expect_revoke_all_for_user()
            .times(1)
            .returning(|_| Ok(2));

        let use_case = ResetPasswordUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_refresh_repo),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("valid_token", "new_password_hash").await;
        assert!(result.is_ok());
        let returned_user = result.unwrap();
        assert_eq!(returned_user.id(), &user_id);
    }

    #[tokio::test]
    async fn test_reset_password_replayed_token_rejected() {
        let user_id = UserId::new();
        let reset_code_id = ResetCodeId::new();

        let mut mock_jwt = MockJwtService::new();
        mock_jwt
            .expect_validate_reset_token()
            .returning(move |_| Ok((user_id, reset_code_id)));

        // The row was already consumed by a prior reset, so no row is deleted.
        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo.expect_consume().returning(|_, _| Ok(false));

        // The password must never be touched once the token is rejected.
        let mock_user_repo = MockUserRepository::new();

        // No sessions are revoked when the reset is rejected (no expectation set
        // ⇒ any call panics).
        let mock_refresh_repo = MockRefreshTokenRepository::new();

        let use_case = ResetPasswordUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_refresh_repo),
            Arc::new(mock_jwt),
        );

        let result = use_case
            .execute("replayed_token", "new_password_hash")
            .await;
        match result {
            Err(DomainError::Authorization(_)) => {}
            _ => panic!("Expected Authorization error"),
        }
    }

    #[tokio::test]
    async fn test_reset_password_invalid_token() {
        let mut mock_jwt = MockJwtService::new();
        mock_jwt
            .expect_validate_reset_token()
            .returning(|_| Err(DomainError::Authorization("Invalid token".into())));

        let mock_user_repo = MockUserRepository::new();
        let mock_reset_repo = MockPasswordResetRepository::new();
        let mock_refresh_repo = MockRefreshTokenRepository::new();

        let use_case = ResetPasswordUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_refresh_repo),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("invalid_token", "new_password_hash").await;
        assert!(result.is_err());
        match result {
            Err(DomainError::Authorization(_)) => {}
            _ => panic!("Expected Authorization error"),
        }
    }

    #[tokio::test]
    async fn test_reset_password_user_not_found() {
        let user_id = UserId::new();
        let reset_code_id = ResetCodeId::new();

        let mut mock_jwt = MockJwtService::new();
        mock_jwt
            .expect_validate_reset_token()
            .returning(move |_| Ok((user_id, reset_code_id)));

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo.expect_find_by_id().returning(|_| Ok(None));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo.expect_consume().returning(|_, _| Ok(true));

        // User vanished after the code was consumed: no password write, no
        // session revocation.
        let mock_refresh_repo = MockRefreshTokenRepository::new();

        let use_case = ResetPasswordUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_refresh_repo),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("valid_token", "new_password_hash").await;
        assert!(result.is_err());
        match result {
            Err(DomainError::NotFound(_)) => {}
            _ => panic!("Expected NotFound error"),
        }
    }
}
