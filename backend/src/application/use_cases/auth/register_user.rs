//! Use case for initiating user registration (email-only step).
//!
//! This is step 1 of the email-first signup flow:
//! 1. User provides email → user record created (no password)
//! 2. User verifies email → signup token issued
//! 3. User sets password using signup token → account complete

use crate::application::ports::UserRepository;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::Email;
use std::sync::Arc;

/// Use case for initiating user registration.
///
/// Creates a user record without a password. The password will be set
/// after email verification via the `CompleteSignupUseCase`.
pub struct RegisterUserUseCase {
    user_repository: Arc<dyn UserRepository>,
}

impl RegisterUserUseCase {
    /// Creates a new instance with the given repository.
    pub fn new(user_repository: Arc<dyn UserRepository>) -> Self {
        Self { user_repository }
    }

    /// Initiates registration for a new user (email-only, no password).
    ///
    /// # Arguments
    ///
    /// * `email` - The user's email address
    ///
    /// # Returns
    ///
    /// The user (new or existing incomplete signup) to send verification email to.
    ///
    /// # Errors
    ///
    /// - [`DomainError::Conflict`] if a verified user with password already exists
    /// - [`DomainError::Validation`] if the email format is invalid
    #[tracing::instrument(name = "usecase.register_user", skip(self), fields(email = %email))]
    pub async fn execute(&self, email: &str) -> Result<User, DomainError> {
        // Validate and create email value object
        let email_vo = Email::new(email)?;

        // Check if user already exists
        if let Some(existing_user) = self.user_repository.find_by_email(email).await? {
            // If user has password and is verified, they're a complete user - conflict
            if existing_user.has_password() && existing_user.email_verified() {
                return Err(DomainError::Conflict(
                    "User with this email already exists".into(),
                ));
            }

            // User exists but hasn't completed signup (no password or not verified)
            // Reuse their record - they'll get a new verification code
            tracing::info!(
                user_id = %existing_user.id(),
                "Reusing existing incomplete signup"
            );

            return Ok(existing_user);
        }

        // Create new user without password
        let user = User::new(
            email_vo,
            None, // No password yet - set after email verification
            String::new(),
            String::new(),
        );

        // Persist
        self.user_repository.save(&user).await?;

        tracing::info!(user_id = %user.id(), "New user created for signup");

        Ok(user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::UserParts;
    use crate::domain::value_objects::{UserId, UserRole};
    use chrono::Utc;

    fn create_incomplete_user() -> User {
        User::from_persistence(UserParts {
            id: UserId::new(),
            email: Email::new("test@example.com").unwrap(),
            password_hash: None, // No password
            first_name: String::new(),
            last_name: String::new(),
            email_verified: false, // Not verified
            pending_email: None,
            pending_email_requested_at: None,
            deleted_at: None,
            deletion_scheduled_at: None,
            role: UserRole::User,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 0,
        })
    }

    fn create_complete_user() -> User {
        User::from_persistence(UserParts {
            id: UserId::new(),
            email: Email::new("test@example.com").unwrap(),
            password_hash: Some("password_hash".to_string()), // Has password
            first_name: "John".to_string(),
            last_name: "Doe".to_string(),
            email_verified: true, // Verified
            pending_email: None,
            pending_email_requested_at: None,
            deleted_at: None,
            deletion_scheduled_at: None,
            role: UserRole::User,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            version: 0,
        })
    }

    #[tokio::test]
    async fn test_register_new_user_success() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(|_| Ok(None));
        mock_repo.expect_save().times(1).returning(|_| Ok(()));

        let use_case = RegisterUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_ok());
        let user = result.unwrap();
        assert_eq!(user.email().as_str(), "test@example.com");
        assert!(!user.has_password()); // No password yet
        assert!(!user.email_verified()); // Not verified yet
    }

    #[tokio::test]
    async fn test_register_reuses_incomplete_signup() {
        let incomplete_user = create_incomplete_user();
        let user_id = *incomplete_user.id();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(Some(incomplete_user.clone())));
        // Note: save() should NOT be called - we reuse existing user

        let use_case = RegisterUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_ok());
        let user = result.unwrap();
        assert_eq!(user.id(), &user_id); // Same user returned
    }

    #[tokio::test]
    async fn test_register_conflict_on_complete_user() {
        let complete_user = create_complete_user();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(Some(complete_user.clone())));

        let use_case = RegisterUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_err());
        assert!(matches!(result, Err(DomainError::Conflict(_))));
    }

    #[tokio::test]
    async fn test_register_invalid_email() {
        let mock_repo = MockUserRepository::new();

        let use_case = RegisterUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("invalid-email").await;

        assert!(result.is_err());
        assert!(matches!(result, Err(DomainError::Validation(_))));
    }

    #[tokio::test]
    async fn test_register_repository_error() {
        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(|_| Ok(None));
        mock_repo
            .expect_save()
            .times(1)
            .returning(|_| Err(DomainError::Persistence("Database error".into())));

        let use_case = RegisterUserUseCase::new(Arc::new(mock_repo));
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_err());
    }
}
