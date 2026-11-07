//! Use case for requesting a password reset.

use crate::application::ports::UserRepository;
use crate::application::ports::{
    CodeGenerator, EmailService, PasswordHasher, PasswordResetRepository,
};
use crate::domain::entities::PasswordResetCode;
use crate::domain::errors::DomainError;
use std::sync::Arc;

/// Use case for requesting a password reset.
///
/// Generates a 6-digit code, stores it, and sends it to the user via email.
pub struct RequestPasswordResetUseCase {
    user_repo: Arc<dyn UserRepository>,
    password_reset_repo: Arc<dyn PasswordResetRepository>,
    email_service: Arc<dyn EmailService>,
    code_generator: Arc<dyn CodeGenerator>,
    password_hasher: Arc<dyn PasswordHasher>,
}

impl RequestPasswordResetUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        password_reset_repo: Arc<dyn PasswordResetRepository>,
        email_service: Arc<dyn EmailService>,
        code_generator: Arc<dyn CodeGenerator>,
        password_hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            user_repo,
            password_reset_repo,
            email_service,
            code_generator,
            password_hasher,
        }
    }

    /// Requests a password reset for the given email.
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if the email exists and code was sent
    /// - `Ok(false)` if the email doesn't exist (for open enumeration policy)
    ///
    /// # Errors
    /// Returns an error if repository operations or email service fails.
    #[tracing::instrument(
        name = "usecase.request_password_reset",
        skip(self),
        fields(email = %email)
    )]
    pub async fn execute(&self, email: &str) -> Result<bool, DomainError> {
        // Find the user by email
        let Some(user) = self.user_repo.find_by_email(email).await? else {
            // Email doesn't exist - return false per our open enumeration policy
            return Ok(false);
        };

        // Don't let a re-request silently clear an active brute-force lockout or
        // bypass the resend cooldown: `save` upserts a fresh code with
        // attempts=0, so issuing one while the current code is locked would reset
        // the counter and defeat the lockout (mirrors resend_verification). When
        // suppressed we still report success — surfacing a distinct error would
        // leak that the email exists, violating the open-enumeration policy.
        if let Some(existing) = self.password_reset_repo.find_by_user(user.id()).await?
            && (existing.seconds_until_unlock() > 0 || !existing.can_resend())
        {
            tracing::info!(
                user_id = %user.id(),
                "Password reset re-request suppressed (locked or within cooldown)"
            );
            return Ok(true);
        }

        // Generate a 6-digit code
        let code = self.code_generator.generate_code(user.email().as_str());

        // Hash the code with Argon2
        let code_hash = self.password_hasher.hash(&code).await?;

        // Create the reset code entity (replaces any existing code for this user)
        let reset_code = PasswordResetCode::new(*user.id(), code_hash);

        // Save to database (upsert - replaces existing code)
        self.password_reset_repo.save(&reset_code).await?;

        // Send the email
        self.email_service
            .send_password_reset_email(user.email().as_str(), &code, user.first_name())
            .await?;

        tracing::info!(user_id = %user.id(), "Password reset code sent");

        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::code_generator::MockCodeGenerator;
    use crate::application::ports::outgoing::email_service::MockEmailService;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::application::ports::outgoing::password_reset_repository::MockPasswordResetRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::User;
    use crate::domain::value_objects::Email;

    fn create_test_user() -> User {
        let mut user = User::new(
            Email::new("test@example.com").unwrap(),
            Some("password_hash".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        );
        user.mark_email_verified();
        user
    }

    #[tokio::test]
    async fn test_request_reset_success() {
        let user = create_test_user();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        // No existing code → no lockout/cooldown suppression, proceed to send.
        mock_reset_repo
            .expect_find_by_user()
            .returning(|_| Ok(None));
        mock_reset_repo.expect_save().returning(|_| Ok(()));

        let mut mock_email_service = MockEmailService::new();
        mock_email_service
            .expect_send_password_reset_email()
            .returning(|_, _, _| Ok(()));

        let mut mock_code_generator = MockCodeGenerator::new();
        mock_code_generator
            .expect_generate_code()
            .returning(|_| "123456".to_string());

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_hash()
            .returning(|_| Ok("hashed".to_string()));

        let use_case = RequestPasswordResetUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_email_service),
            Arc::new(mock_code_generator),
            Arc::new(mock_hasher),
        );

        let result = use_case.execute("test@example.com").await;
        assert!(result.is_ok());
        assert!(result.unwrap()); // true = email found
    }

    #[tokio::test]
    async fn test_request_reset_user_not_found() {
        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(|_| Ok(None));

        let mock_reset_repo = MockPasswordResetRepository::new();
        let mock_email_service = MockEmailService::new();
        let mock_code_generator = MockCodeGenerator::new();
        let mock_hasher = MockPasswordHasher::new();

        let use_case = RequestPasswordResetUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_email_service),
            Arc::new(mock_code_generator),
            Arc::new(mock_hasher),
        );

        let result = use_case.execute("unknown@example.com").await;
        assert!(result.is_ok());
        assert!(!result.unwrap()); // false = email not found
    }

    /// A re-request while the existing code is locked must NOT mint a fresh
    /// code (which would reset attempts and clear the lockout). It reports
    /// success without sending, per the enumeration policy.
    #[tokio::test]
    async fn test_request_reset_suppressed_while_locked() {
        use crate::domain::entities::password_reset_code::MAX_ATTEMPTS;
        use crate::domain::value_objects::ResetCodeId;
        use chrono::{Duration, Utc};

        let user = create_test_user();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        // Locked code sent just now: within the 15-min lock window.
        let locked = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            *create_test_user().id(),
            "hash".to_string(),
            Utc::now() + Duration::minutes(15),
            MAX_ATTEMPTS,
            Utc::now(),
            Utc::now(),
        );

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(locked.clone())));
        // Must NOT mint a new code or send an email while locked.
        mock_reset_repo.expect_save().never();

        let mut mock_email_service = MockEmailService::new();
        mock_email_service
            .expect_send_password_reset_email()
            .never();

        let use_case = RequestPasswordResetUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_email_service),
            Arc::new(MockCodeGenerator::new()),
            Arc::new(MockPasswordHasher::new()),
        );

        let result = use_case.execute("test@example.com").await;
        assert!(result.is_ok());
        assert!(result.unwrap()); // reports success without sending
    }
}
