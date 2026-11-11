//! Use case for the email-only first step of registration.

use crate::application::ports::{EmailService, UserRepository};
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::Email;
use std::sync::Arc;

/// What registration found for the submitted email.
///
/// Callers must answer both variants identically: telling them apart in the
/// response would let anyone check whether an email has an account.
#[derive(Debug)]
pub enum RegisterOutcome {
    /// A new or still-incomplete signup: send it a verification code.
    PendingVerification(User),
    /// A completed account already uses this email: notify its owner with
    /// [`RegisterUserUseCase::notify_existing`] instead of sending a code.
    AlreadyRegistered(User),
}

pub struct RegisterUserUseCase {
    user_repository: Arc<dyn UserRepository>,
    email_service: Arc<dyn EmailService>,
}

impl RegisterUserUseCase {
    pub fn new(
        user_repository: Arc<dyn UserRepository>,
        email_service: Arc<dyn EmailService>,
    ) -> Self {
        Self {
            user_repository,
            email_service,
        }
    }

    /// Creates a password-less user for `email`, or finds the existing one.
    ///
    /// # Errors
    /// Returns [`DomainError::Validation`] for a malformed email, or a
    /// persistence error from the repository.
    #[tracing::instrument(name = "usecase.register_user", skip_all)]
    pub async fn execute(&self, email: &str) -> Result<RegisterOutcome, DomainError> {
        let email_vo = Email::new(email)?;

        if let Some(existing_user) = self.user_repository.find_by_email(email).await? {
            if existing_user.has_password() && existing_user.email_verified() {
                tracing::info!(
                    user_id = %existing_user.id(),
                    "Registration attempted for an existing account"
                );
                return Ok(RegisterOutcome::AlreadyRegistered(existing_user));
            }

            tracing::info!(
                user_id = %existing_user.id(),
                "Reusing existing incomplete signup"
            );

            return Ok(RegisterOutcome::PendingVerification(existing_user));
        }

        let user = User::new(
            email_vo,
            None, // No password yet - set after email verification
            String::new(),
            String::new(),
        );

        self.user_repository.save(&user).await?;

        tracing::info!(user_id = %user.id(), "New user created for signup");

        Ok(RegisterOutcome::PendingVerification(user))
    }

    /// Emails the owner of an existing account that someone tried to register
    /// with their address.
    ///
    /// # Errors
    /// Returns the email service's error if the notice could not be sent.
    #[tracing::instrument(name = "usecase.register_user.notify_existing", skip_all, fields(user_id = %user.id()))]
    pub async fn notify_existing(&self, user: &User) -> Result<(), DomainError> {
        self.email_service
            .notify_account_exists(user.email().as_str(), user.first_name())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::email_service::MockEmailService;
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

        let use_case =
            RegisterUserUseCase::new(Arc::new(mock_repo), Arc::new(MockEmailService::new()));
        let result = use_case.execute("test@example.com").await;

        let Ok(RegisterOutcome::PendingVerification(user)) = result else {
            panic!("expected a pending signup, got {result:?}");
        };
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

        let use_case =
            RegisterUserUseCase::new(Arc::new(mock_repo), Arc::new(MockEmailService::new()));
        let result = use_case.execute("test@example.com").await;

        let Ok(RegisterOutcome::PendingVerification(user)) = result else {
            panic!("expected a pending signup, got {result:?}");
        };
        assert_eq!(user.id(), &user_id); // Same user returned
    }

    #[tokio::test]
    async fn test_register_reports_complete_user_as_already_registered() {
        let complete_user = create_complete_user();

        let mut mock_repo = MockUserRepository::new();
        mock_repo
            .expect_find_by_email()
            .times(1)
            .returning(move |_| Ok(Some(complete_user.clone())));

        let use_case =
            RegisterUserUseCase::new(Arc::new(mock_repo), Arc::new(MockEmailService::new()));
        let result = use_case.execute("test@example.com").await;

        // Not an error: the handler must answer exactly as for a new signup.
        assert!(matches!(result, Ok(RegisterOutcome::AlreadyRegistered(_))));
    }

    #[tokio::test]
    async fn notify_existing_sends_the_account_exists_email() {
        let user = create_complete_user();
        let mut email = MockEmailService::new();
        email
            .expect_notify_account_exists()
            .withf(|to, _| to == "test@example.com")
            .times(1)
            .returning(|_, _| Ok(()));

        let use_case =
            RegisterUserUseCase::new(Arc::new(MockUserRepository::new()), Arc::new(email));
        use_case.notify_existing(&user).await.unwrap();
    }

    #[tokio::test]
    async fn test_register_invalid_email() {
        let mock_repo = MockUserRepository::new();

        let use_case =
            RegisterUserUseCase::new(Arc::new(mock_repo), Arc::new(MockEmailService::new()));
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

        let use_case =
            RegisterUserUseCase::new(Arc::new(mock_repo), Arc::new(MockEmailService::new()));
        let result = use_case.execute("test@example.com").await;

        assert!(result.is_err());
    }
}
