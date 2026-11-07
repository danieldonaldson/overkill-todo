//! Use case for completing user signup by setting a password.
//!
//! This is step 3 of the email-first signup flow:
//! 1. User provides email → verification code sent
//! 2. User verifies email → signup token issued
//! 3. User sets password using signup token → account complete

use crate::application::ports::JwtService;
use crate::application::ports::UserRepository;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use std::sync::Arc;

/// Use case for completing signup by setting a password.
///
/// Validates the signup token (from email verification) and sets the user's password.
/// Returns the user for auto-login functionality.
pub struct CompleteSignupUseCase {
    user_repo: Arc<dyn UserRepository>,
    jwt_service: Arc<dyn JwtService>,
}

impl CompleteSignupUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(user_repo: Arc<dyn UserRepository>, jwt_service: Arc<dyn JwtService>) -> Self {
        Self {
            user_repo,
            jwt_service,
        }
    }

    /// Completes signup by setting the user's password.
    ///
    /// # Arguments
    ///
    /// * `signup_token` - The JWT token from email verification (30 min expiry)
    /// * `email` - Email from request (verified against user for security)
    /// * `password_hash` - The new password hash (already hashed by the handler)
    ///
    /// # Returns
    ///
    /// The user whose password was set (for auto-login)
    ///
    /// # Errors
    ///
    /// - `Authorization` if the token is invalid or expired
    /// - `Validation` if email doesn't match or user already has password
    /// - `NotFound` if the user doesn't exist
    #[tracing::instrument(
        name = "usecase.complete_signup",
        skip(self, signup_token, password_hash),
        fields(email = %email)
    )]
    pub async fn execute(
        &self,
        signup_token: &str,
        email: &str,
        password_hash: &str,
    ) -> Result<User, DomainError> {
        // Validate and decode the signup token
        let user_id = self.jwt_service.validate_signup_token(signup_token)?;

        // Find the user by ID from token
        let user = self
            .user_repo
            .find_by_id(&user_id)
            .await?
            .ok_or_else(|| DomainError::NotFound("User not found".into()))?;

        // Verify email matches (prevents token theft)
        if user.email().as_str() != email {
            return Err(DomainError::Validation("Email mismatch".into()));
        }

        // User must not already have a password
        if user.has_password() {
            return Err(DomainError::Validation(
                "Account already has a password set".into(),
            ));
        }

        // Set the password
        self.user_repo
            .update_password(user.id(), password_hash)
            .await?;

        // Reload user to get updated state
        let updated_user = self
            .user_repo
            .find_by_id(user.id())
            .await?
            .ok_or_else(|| DomainError::Persistence("User not found after update".into()))?;

        tracing::info!(user_id = %user.id(), "Signup completed, password set");

        Ok(updated_user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::jwt_service::MockJwtService;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::UserBuilder;
    use crate::domain::value_objects::UserId;

    const EMAIL: &str = "signup@example.com";

    fn user_with_password(id: UserId, password_hash: Option<&str>) -> User {
        UserBuilder::new()
            .id(id)
            .email(EMAIL)
            .password_hash(password_hash)
            .name("", "")
            .build()
    }

    fn jwt_returning(user_id: UserId) -> MockJwtService {
        let mut jwt = MockJwtService::new();
        jwt.expect_validate_signup_token()
            .returning(move |_| Ok(user_id));
        jwt
    }

    #[tokio::test]
    async fn sets_password_for_valid_token() {
        let user_id = UserId::new();

        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user_with_password(user_id, None))));
        repo.expect_update_password()
            .times(1)
            .withf(move |id, hash| id == &user_id && hash == "new-hash")
            .returning(|_, _| Ok(()));

        let uc = CompleteSignupUseCase::new(Arc::new(repo), Arc::new(jwt_returning(user_id)));
        let result = uc.execute("token", EMAIL, "new-hash").await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap().email().as_str(), EMAIL);
    }

    #[tokio::test]
    async fn rejects_invalid_signup_token() {
        let mut jwt = MockJwtService::new();
        jwt.expect_validate_signup_token()
            .returning(|_| Err(DomainError::Authorization("bad token".into())));

        let uc = CompleteSignupUseCase::new(Arc::new(MockUserRepository::new()), Arc::new(jwt));
        let err = uc.execute("token", EMAIL, "new-hash").await.unwrap_err();
        assert!(matches!(err, DomainError::Authorization(_)));
    }

    #[tokio::test]
    async fn rejects_when_user_missing() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id().returning(|_| Ok(None));
        repo.expect_update_password().never();

        let uc = CompleteSignupUseCase::new(Arc::new(repo), Arc::new(jwt_returning(user_id)));
        let err = uc.execute("token", EMAIL, "new-hash").await.unwrap_err();
        assert!(matches!(err, DomainError::NotFound(_)));
    }

    /// The token's user id is valid but the request's email doesn't match the
    /// account on it — reject so a stolen token can't be redeemed for a different
    /// address.
    #[tokio::test]
    async fn rejects_email_mismatch() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user_with_password(user_id, None))));
        repo.expect_update_password().never();

        let uc = CompleteSignupUseCase::new(Arc::new(repo), Arc::new(jwt_returning(user_id)));
        let err = uc
            .execute("token", "someone-else@example.com", "new-hash")
            .await
            .unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }

    #[tokio::test]
    async fn rejects_when_password_already_set() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user_with_password(user_id, Some("existing")))));
        repo.expect_update_password().never();

        let uc = CompleteSignupUseCase::new(Arc::new(repo), Arc::new(jwt_returning(user_id)));
        let err = uc.execute("token", EMAIL, "new-hash").await.unwrap_err();
        assert!(matches!(err, DomainError::Validation(_)));
    }
}
