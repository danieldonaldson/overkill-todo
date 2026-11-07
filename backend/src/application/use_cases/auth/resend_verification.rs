//! Use case for resending an email verification code.

use crate::application::ports::UserRepository;
use crate::application::ports::{
    CodeGenerator, EmailService, EmailVerificationRepository, PasswordHasher,
};
use crate::domain::entities::EmailVerificationCode;
use crate::domain::entities::email_verification_code::RESEND_COOLDOWN_MINUTES;
use std::sync::Arc;

/// Result of a resend operation, including metadata for the frontend.
#[derive(Debug)]
pub struct ResendResult {
    /// Seconds until the next resend is allowed (for countdown timer).
    pub cooldown_seconds: i64,
}

/// Error details for resend failures.
#[derive(Debug)]
pub enum ResendVerificationError {
    /// User not found.
    NotFound(String),
    /// Email is already verified.
    AlreadyVerified,
    /// Rate limited - must wait before resending.
    CooldownActive { seconds_remaining: i64 },
    /// Code is locked from too many failed attempts; resend is refused for a
    /// short fixed window (resend must not reset the brute-force lockout).
    Locked { seconds_until_unlock: i64 },
    /// Internal error.
    Internal(String),
}

/// Use case for resending an email verification code.
///
/// Rate limited to once every 5 minutes.
pub struct ResendVerificationUseCase {
    user_repo: Arc<dyn UserRepository>,
    email_verification_repo: Arc<dyn EmailVerificationRepository>,
    email_service: Arc<dyn EmailService>,
    code_generator: Arc<dyn CodeGenerator>,
    password_hasher: Arc<dyn PasswordHasher>,
}

impl ResendVerificationUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        email_verification_repo: Arc<dyn EmailVerificationRepository>,
        email_service: Arc<dyn EmailService>,
        code_generator: Arc<dyn CodeGenerator>,
        password_hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            user_repo,
            email_verification_repo,
            email_service,
            code_generator,
            password_hasher,
        }
    }

    /// Resends a verification code to the given email.
    ///
    /// # Returns
    ///
    /// On success, returns `ResendResult` with the cooldown seconds for the frontend.
    ///
    /// # Errors
    ///
    /// Returns `ResendVerificationError` for specific error types.
    #[tracing::instrument(
        name = "usecase.resend_verification",
        skip(self),
        fields(email = %email)
    )]
    pub async fn execute(&self, email: &str) -> Result<ResendResult, ResendVerificationError> {
        // Find the user by email
        let user = self
            .user_repo
            .find_by_email(email)
            .await
            .map_err(|e| ResendVerificationError::Internal(e.to_string()))?
            .ok_or_else(|| {
                ResendVerificationError::NotFound("No account with this email".into())
            })?;

        // Check if already verified
        if user.email_verified() {
            return Err(ResendVerificationError::AlreadyVerified);
        }

        // Check for existing code: enforce the brute-force lockout and rate limit.
        if let Some(existing_code) = self
            .email_verification_repo
            .find_by_user(user.id())
            .await
            .map_err(|e| ResendVerificationError::Internal(e.to_string()))?
        {
            // `save` upserts a fresh code with attempts=0, so allowing a
            // resend while the current code is locked would silently clear the
            // lockout. Refuse for a short fixed window (LOCK_DURATION_MINUTES)
            // after the lock, then allow a fresh code — rather than blocking for
            // the full 24h code lifetime.
            let seconds_until_unlock = existing_code.seconds_until_unlock();
            if seconds_until_unlock > 0 {
                return Err(ResendVerificationError::Locked {
                    seconds_until_unlock,
                });
            }
            if !existing_code.can_resend() {
                return Err(ResendVerificationError::CooldownActive {
                    seconds_remaining: existing_code.seconds_until_resend(),
                });
            }
        }

        // Generate a new 6-digit code
        let code = self.code_generator.generate_code(user.email().as_str());

        // Hash the code with Argon2
        let code_hash = self
            .password_hasher
            .hash(&code)
            .await
            .map_err(|e| ResendVerificationError::Internal(e.to_string()))?;

        // Create or update the verification code
        let verification_code = EmailVerificationCode::new(*user.id(), code_hash);
        self.email_verification_repo
            .save(&verification_code)
            .await
            .map_err(|e| ResendVerificationError::Internal(e.to_string()))?;

        // Send the verification email. Resend is code-only: the original
        // magic-link token (preserved by the verification upsert's COALESCE)
        // can't be reconstructed here since only its hash is stored, so no new
        // link is issued — the previously-emailed link remains valid.
        self.email_service
            .send_verification_email(user.email().as_str(), &code, user.first_name(), None)
            .await
            .map_err(|e| ResendVerificationError::Internal(e.to_string()))?;

        tracing::info!(user_id = %user.id(), "Verification code resent");

        // Return the cooldown duration for the frontend countdown timer
        Ok(ResendResult {
            cooldown_seconds: RESEND_COOLDOWN_MINUTES * 60,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::code_generator::MockCodeGenerator;
    use crate::application::ports::outgoing::email_service::MockEmailService;
    use crate::application::ports::outgoing::email_verification_repository::MockEmailVerificationRepository;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::User;
    use crate::domain::value_objects::Email;

    fn create_unverified_user() -> User {
        User::new(
            Email::new("test@example.com").unwrap(),
            Some("password_hash".to_string()),
            "John".to_string(),
            "Doe".to_string(),
        )
    }

    fn create_verified_user() -> User {
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
    async fn test_resend_success_no_existing_code() {
        let user = create_unverified_user();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_user()
            .returning(|_| Ok(None));
        mock_verification_repo.expect_save().returning(|_| Ok(()));

        let mut mock_email_service = MockEmailService::new();
        mock_email_service
            .expect_send_verification_email()
            .withf(|_, _, _, magic_link: &Option<String>| magic_link.is_none())
            .returning(|_, _, _, _| Ok(()));

        let mut mock_code_generator = MockCodeGenerator::new();
        mock_code_generator
            .expect_generate_code()
            .returning(|_| "123456".to_string());

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher
            .expect_hash()
            .returning(|_| Ok("hashed".to_string()));

        let use_case = ResendVerificationUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_verification_repo),
            Arc::new(mock_email_service),
            Arc::new(mock_code_generator),
            Arc::new(mock_hasher),
        );

        let result = use_case.execute("test@example.com").await;
        assert!(result.is_ok());
        let resend_result = result.unwrap();
        assert_eq!(resend_result.cooldown_seconds, RESEND_COOLDOWN_MINUTES * 60);
    }

    #[tokio::test]
    async fn test_resend_already_verified() {
        let user = create_verified_user();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mock_verification_repo = MockEmailVerificationRepository::new();
        let mock_email_service = MockEmailService::new();
        let mock_code_generator = MockCodeGenerator::new();
        let mock_hasher = MockPasswordHasher::new();

        let use_case = ResendVerificationUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_verification_repo),
            Arc::new(mock_email_service),
            Arc::new(mock_code_generator),
            Arc::new(mock_hasher),
        );

        let result = use_case.execute("test@example.com").await;
        assert!(matches!(
            result,
            Err(ResendVerificationError::AlreadyVerified)
        ));
    }

    #[tokio::test]
    async fn test_resend_blocked_while_locked() {
        use crate::domain::entities::email_verification_code::MAX_ATTEMPTS;
        use crate::domain::value_objects::VerificationCodeId;
        use chrono::{Duration, Utc};

        let user = create_unverified_user();
        let user_id = *user.id();

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        // Locked (attempts == MAX), last sent 10 min ago: the 5-min resend
        // cooldown has elapsed, but the 15-min lock window has not — the lockout
        // must still win.
        let locked_code = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            user_id,
            "hash".to_string(),
            Utc::now() + Duration::hours(1),
            MAX_ATTEMPTS,
            Utc::now() - Duration::minutes(10),
            Utc::now() - Duration::minutes(10),
            None,
            None,
        );

        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(locked_code.clone())));
        // save must NOT be called: a locked code must not be reset via resend.
        mock_verification_repo.expect_save().never();

        let use_case = ResendVerificationUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_verification_repo),
            Arc::new(MockEmailService::new()),
            Arc::new(MockCodeGenerator::new()),
            Arc::new(MockPasswordHasher::new()),
        );

        let result = use_case.execute("test@example.com").await;
        assert!(matches!(
            result,
            Err(ResendVerificationError::Locked { .. })
        ));
    }

    #[tokio::test]
    async fn test_resend_user_not_found() {
        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(|_| Ok(None));

        let mock_verification_repo = MockEmailVerificationRepository::new();
        let mock_email_service = MockEmailService::new();
        let mock_code_generator = MockCodeGenerator::new();
        let mock_hasher = MockPasswordHasher::new();

        let use_case = ResendVerificationUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_verification_repo),
            Arc::new(mock_email_service),
            Arc::new(mock_code_generator),
            Arc::new(mock_hasher),
        );

        let result = use_case.execute("test@example.com").await;
        assert!(matches!(result, Err(ResendVerificationError::NotFound(_))));
    }
}
