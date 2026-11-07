//! Use case for changing a user's password.
//!
//! This is for users who already have a password and want to change it.
//! For OAuth-only users without a password, use [`SetPasswordUseCase`].

use std::sync::Arc;

use crate::application::ports::PasswordHasher;
use crate::application::ports::UserRepository;
use crate::application::ports::{RefreshTokenRepository, TokenService};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;

/// Use case for changing an existing password.
///
/// Requires the current password for verification before allowing the change.
pub struct ChangePasswordUseCase {
    user_repository: Arc<dyn UserRepository>,
    password_hasher: Arc<dyn PasswordHasher>,
    refresh_token_repository: Arc<dyn RefreshTokenRepository>,
    token_service: Arc<dyn TokenService>,
}

impl ChangePasswordUseCase {
    /// Creates a new instance with the given dependencies.
    #[must_use]
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        password_hasher: Arc<dyn PasswordHasher>,
        refresh_token_repository: Arc<dyn RefreshTokenRepository>,
        token_service: Arc<dyn TokenService>,
    ) -> Self {
        Self {
            user_repository,
            password_hasher,
            refresh_token_repository,
            token_service,
        }
    }

    /// Changes the user's password after verifying the current password.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The ID of the user changing their password
    /// * `current_password` - The current password for verification
    /// * `new_password` - The new password to set
    /// * `current_refresh_token` - The plaintext refresh token of the device
    ///   making the request, if any. Every *other* device's session is revoked;
    ///   this one is preserved so the user is not logged out of their own
    ///   session. When `None` (no refresh cookie), all sessions are revoked.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - The user doesn't exist
    /// - The user doesn't have a password (OAuth-only users)
    /// - The current password is incorrect
    /// - The new password is the same as the current password
    #[tracing::instrument(name = "usecase.change_password", skip(self, current_password, new_password, current_refresh_token), fields(user_id = %user_id))]
    pub async fn execute(
        &self,
        user_id: &UserId,
        current_password: &str,
        new_password: &str,
        current_refresh_token: Option<&str>,
    ) -> Result<(), DomainError> {
        // Find the user
        let mut user = self
            .user_repository
            .find_by_id(user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Verify user has a password (not OAuth-only)
        let password_hash = user.password_hash().ok_or_else(|| {
            DomainError::Validation(
                "Cannot change password for OAuth-only accounts. Use 'Set Password' instead."
                    .into(),
            )
        })?;

        // Verify current password
        if !self
            .password_hasher
            .verify(current_password, password_hash)
            .await?
        {
            return Err(DomainError::Authorization(
                "Current password is incorrect".into(),
            ));
        }

        // Don't allow setting the same password
        if current_password == new_password {
            return Err(DomainError::Validation(
                "New password must be different from current password".into(),
            ));
        }

        // Hash new password
        let new_password_hash = self.password_hasher.hash(new_password).await?;

        // Update user
        user.update_password(new_password_hash);
        self.user_repository.save(&user).await?;

        // Log out every *other* device: a password change should evict any session
        // the user no longer controls, while keeping the current one alive so the
        // change isn't self-defeating (the user staying logged in is expected UX).
        // The current device is identified by the hash of its refresh token.
        let revoked = match current_refresh_token {
            Some(token) => {
                let keep_hash = self.token_service.hash_token(token);
                self.refresh_token_repository
                    .revoke_all_for_user_except(user_id, &keep_hash)
                    .await?
            }
            // No refresh cookie on the request: nothing to preserve, so revoke
            // everything (the current device's short-lived access JWT keeps
            // working until it expires).
            None => {
                self.refresh_token_repository
                    .revoke_all_for_user(user_id)
                    .await?
            }
        };

        tracing::info!(user_id = %user_id, revoked, "User password changed; other sessions revoked");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::application::ports::outgoing::refresh_token_repository::MockRefreshTokenRepository;
    use crate::application::ports::outgoing::token_service::MockTokenService;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::{User, UserBuilder};

    /// A refresh-token-repository mock that expects no revocation calls. Used by
    /// the failure-path tests where the password is never changed.
    fn no_revocation_repo() -> MockRefreshTokenRepository {
        MockRefreshTokenRepository::new()
    }

    fn create_user_with_password() -> User {
        UserBuilder::new().build()
    }

    fn create_oauth_only_user() -> User {
        UserBuilder::new()
            .email("oauth@example.com")
            .password_hash(None)
            .name("OAuth", "User")
            .build()
    }

    #[tokio::test]
    async fn test_change_password_success() {
        let user = create_user_with_password();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_verify()
            .times(1)
            .returning(|_, _| Ok(true));
        mock_hasher
            .expect_hash()
            .times(1)
            .returning(|_| Ok("new_hashed_password".to_string()));

        // The current device's token is hashed and preserved; all other sessions
        // are revoked.
        let mut mock_token_service = MockTokenService::new();
        mock_token_service
            .expect_hash_token()
            .times(1)
            .returning(|_| "current_token_hash".to_string());
        let mut mock_refresh_repo = MockRefreshTokenRepository::new();
        mock_refresh_repo
            .expect_revoke_all_for_user_except()
            .times(1)
            .returning(|_, _| Ok(1));

        let use_case = ChangePasswordUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_refresh_repo),
            Arc::new(mock_token_service),
        );
        let result = use_case
            .execute(
                &user_id,
                "old_password",
                "new_secure_password",
                Some("current-refresh-token"),
            )
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_change_password_without_refresh_token_revokes_all() {
        let user = create_user_with_password();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_verify()
            .times(1)
            .returning(|_, _| Ok(true));
        mock_hasher
            .expect_hash()
            .times(1)
            .returning(|_| Ok("new_hashed_password".to_string()));

        // With no current refresh token, every session is revoked (and the token
        // service is never consulted).
        let mock_token_service = MockTokenService::new();
        let mut mock_refresh_repo = MockRefreshTokenRepository::new();
        mock_refresh_repo
            .expect_revoke_all_for_user()
            .times(1)
            .returning(|_| Ok(3));

        let use_case = ChangePasswordUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_refresh_repo),
            Arc::new(mock_token_service),
        );
        let result = use_case
            .execute(&user_id, "old_password", "new_secure_password", None)
            .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_change_password_user_not_found() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(|_| Ok(None));

        let use_case = ChangePasswordUseCase::new(
            Arc::new(mock_repo),
            Arc::new(MockPasswordHasher::new()),
            Arc::new(no_revocation_repo()),
            Arc::new(MockTokenService::new()),
        );
        let result = use_case
            .execute(&UserId::new(), "old", "new", Some("tok"))
            .await;

        assert!(matches!(result, Err(DomainError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_change_password_oauth_only_user() {
        let user = create_oauth_only_user();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case = ChangePasswordUseCase::new(
            Arc::new(mock_repo),
            Arc::new(MockPasswordHasher::new()),
            Arc::new(no_revocation_repo()),
            Arc::new(MockTokenService::new()),
        );
        let result = use_case
            .execute(&user_id, "any_password", "new_password", Some("tok"))
            .await;

        assert!(matches!(result, Err(DomainError::Validation(_))));
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("OAuth-only"));
    }

    #[tokio::test]
    async fn test_change_password_wrong_current_password() {
        let user = create_user_with_password();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_verify()
            .times(1)
            .returning(|_, _| Ok(false)); // Wrong password

        let use_case = ChangePasswordUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_hasher),
            Arc::new(no_revocation_repo()),
            Arc::new(MockTokenService::new()),
        );
        let result = use_case
            .execute(&user_id, "wrong_password", "new_password", Some("tok"))
            .await;

        assert!(matches!(result, Err(DomainError::Authorization(_))));
    }

    #[tokio::test]
    async fn test_change_password_same_as_current() {
        let user = create_user_with_password();
        let user_id = *user.id();
        let user_clone = user.clone();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_verify()
            .times(1)
            .returning(|_, _| Ok(true));

        let use_case = ChangePasswordUseCase::new(
            Arc::new(mock_repo),
            Arc::new(mock_hasher),
            Arc::new(no_revocation_repo()),
            Arc::new(MockTokenService::new()),
        );
        let result = use_case
            .execute(&user_id, "same_password", "same_password", Some("tok"))
            .await;

        assert!(matches!(result, Err(DomainError::Validation(_))));
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("different"));
    }
}
