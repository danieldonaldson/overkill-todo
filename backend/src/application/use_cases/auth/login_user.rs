//! Use case for user login (user lookup by email).

use crate::application::ports::UserRepository;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use std::sync::Arc;

/// Use case for user login (user lookup by email).
///
/// Returns the user if found; password verification is done by the caller.
///
/// # Brute-force protection (deliberately not here)
///
/// There is intentionally no per-account `failed_login`/`locked_until` counter on
/// this path. Password login is meant to be throttled at the edge — a rate limit on
/// `/api/v1/auth/*` at your proxy plus Cloudflare Turnstile on the form — which suits
/// high-entropy, Argon2id-verified passwords. (The low-entropy 6-digit reset/verify
/// codes keep their own atomic in-app attempt cap; see `verify_email.rs`.)
///
/// The starter's deploy configs ship no edge rate limit and Turnstile is off until
/// configured: add both before going live, or add an in-app per-account/IP lockout.
pub struct LoginUserUseCase {
    user_repository: Arc<dyn UserRepository>,
}

impl LoginUserUseCase {
    /// Creates a new instance with the given repository.
    pub fn new(user_repository: Arc<dyn UserRepository>) -> Self {
        Self { user_repository }
    }

    /// Finds a user by email for authentication.
    ///
    /// Returns `None` if no user exists with the given email.
    /// The caller is responsible for verifying the password.
    ///
    /// # Errors
    /// Returns an error if the repository fails to retrieve the user.
    #[tracing::instrument(name = "usecase.login_user", skip(self), fields(email = %email))]
    pub async fn execute(&self, email: &str) -> Result<Option<User>, DomainError> {
        // Find user by email
        let user = self.user_repository.find_by_email(email).await?;
        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::value_objects::Email;

    #[tokio::test]
    async fn test_login_user_found() {
        let existing_user = User::new(
            Email::new("test@example.com").unwrap(),
            Some("hashed_password".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );

        let mut mock_repo = MockUserRepository::new();
        let user_clone = existing_user.clone();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(Some(user_clone.clone())));

        let use_case = LoginUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_ok());
        let user = result.unwrap();
        assert!(user.is_some());
        assert_eq!(user.unwrap().email().as_str(), "test@example.com");
    }

    #[tokio::test]
    async fn test_login_user_not_found() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(|_| Ok(None));

        let use_case = LoginUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("notfound@example.com").await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_login_user_repository_error() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("Database error".into())));

        let use_case = LoginUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_err());
    }
}
