//! Use case for resending an email verification code.

use crate::ports::UserRepository;
use crate::ports::{CodeGenerator, EmailService, EmailVerificationRepository, PasswordHasher};
use domain::entities::attempt_lockout::IssueOutcome;
use domain::entities::email_verification_code::RESEND_COOLDOWN_MINUTES;
use domain::entities::{EmailVerificationCode, Reissue};
use domain::value_objects::{Email, UserId};
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
    #[tracing::instrument(name = "usecase.resend_verification", skip_all)]
    pub async fn execute(&self, email: &str) -> Result<ResendResult, ResendVerificationError> {
        // Find the user by normalized email
        let email = Email::new(email)
            .map_err(|_| ResendVerificationError::NotFound("No account with this email".into()))?;
        let user = self
            .user_repo
            .find_by_email(&email)
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
            match existing_code.reissue(jiff::Timestamp::now()) {
                Reissue::Allowed => {}
                Reissue::Locked { seconds_remaining } => {
                    return Err(ResendVerificationError::Locked {
                        seconds_until_unlock: seconds_remaining,
                    });
                }
                Reissue::Cooldown { seconds_remaining } => {
                    return Err(ResendVerificationError::CooldownActive { seconds_remaining });
                }
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

        // Create or update the verification code. A wrong guess can lock the
        // current code after the decision above, or a concurrent request
        // issue one; the store then refuses, and the caller gets the answer it
        // would have got had it seen that first.
        let verification_code = EmailVerificationCode::new(*user.id(), code_hash);
        let stored = self
            .email_verification_repo
            .save(&verification_code)
            .await
            .map_err(|e| ResendVerificationError::Internal(e.to_string()))?;
        if stored == IssueOutcome::Withheld {
            tracing::info!(user_id = %user.id(), "Verification code changed while resending; no new code");
            return Err(self.withheld(user.id()).await);
        }

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

impl ResendVerificationUseCase {
    /// Why the store withheld a new code: the stored code's own reissue
    /// decision now. Best-effort: if it can't be read, or no longer refuses,
    /// this reports the full cooldown rather than failing a request whose
    /// answer the handler makes uniform anyway.
    async fn withheld(&self, user_id: &UserId) -> ResendVerificationError {
        let cooldown = ResendVerificationError::CooldownActive {
            seconds_remaining: RESEND_COOLDOWN_MINUTES * 60,
        };
        match self.email_verification_repo.find_by_user(user_id).await {
            Ok(Some(code)) => match code.reissue(jiff::Timestamp::now()) {
                Reissue::Locked { seconds_remaining } => ResendVerificationError::Locked {
                    seconds_until_unlock: seconds_remaining,
                },
                Reissue::Cooldown { seconds_remaining } => {
                    ResendVerificationError::CooldownActive { seconds_remaining }
                }
                Reissue::Allowed => cooldown,
            },
            Ok(None) => cooldown,
            Err(e) => {
                tracing::warn!(user_id = %user_id, error = %e, "Could not read the code a resend was refused for");
                cooldown
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::code_generator::MockCodeGenerator;
    use crate::ports::outgoing::email_service::MockEmailService;
    use crate::ports::outgoing::email_verification_repository::MockEmailVerificationRepository;
    use crate::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::ports::outgoing::user_repository::MockUserRepository;
    use domain::entities::User;
    use domain::value_objects::Email;

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
        mock_verification_repo
            .expect_save()
            .returning(|_| Ok(IssueOutcome::Issued));

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
        use domain::entities::email_verification_code::MAX_ATTEMPTS;
        use domain::value_objects::VerificationCodeId;
        use jiff::{SignedDuration, Timestamp};

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
            Timestamp::now() + SignedDuration::from_hours(1),
            MAX_ATTEMPTS,
            Timestamp::now() - SignedDuration::from_mins(10),
            Timestamp::now() - SignedDuration::from_mins(10),
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

    /// P-11: a wrong guess locks the code after the reissue check, so the
    /// store refuses the new one. The resend reports `Locked`, with the wait
    /// read from the stored code, and sends nothing.
    #[tokio::test]
    async fn resend_refused_by_the_store_reports_locked() {
        use domain::entities::email_verification_code::MAX_ATTEMPTS;
        use domain::value_objects::VerificationCodeId;
        use jiff::{SignedDuration, Timestamp};

        let user = create_unverified_user();
        let user_id = *user.id();
        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        // First read: 4 attempts, sent 6 minutes ago, so a resend is allowed.
        // Second read (after the refused write): the guess has locked it.
        let code = move |attempts| {
            EmailVerificationCode::from_persistence(
                VerificationCodeId::new(),
                user_id,
                "hash".to_string(),
                Timestamp::now() + SignedDuration::from_hours(1),
                attempts,
                Timestamp::now() - SignedDuration::from_mins(6),
                Timestamp::now() - SignedDuration::from_mins(6),
                None,
                None,
            )
        };
        let mut reads = 0;
        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_user()
            .times(2)
            .returning(move |_| {
                reads += 1;
                Ok(Some(code(if reads == 1 {
                    MAX_ATTEMPTS - 1
                } else {
                    MAX_ATTEMPTS
                })))
            });
        mock_verification_repo
            .expect_save()
            .times(1)
            .returning(|_| Ok(IssueOutcome::Withheld));

        let mut mock_email_service = MockEmailService::new();
        mock_email_service.expect_send_verification_email().times(0);
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

        // 15-minute window from a send 6 minutes ago: about 9 minutes left
        // (rounded up to whole seconds).
        match use_case.execute("test@example.com").await {
            Err(ResendVerificationError::Locked {
                seconds_until_unlock,
            }) => assert!(
                (8 * 60..=9 * 60 + 1).contains(&seconds_until_unlock),
                "got {seconds_until_unlock}"
            ),
            other => panic!("expected Locked, got {other:?}"),
        }
    }

    /// A refused resend whose follow-up read fails still answers like a
    /// cooldown, never with an internal error (the handler's answer must not
    /// depend on the account's state).
    #[tokio::test]
    async fn resend_refused_by_the_store_never_fails_on_the_follow_up_read() {
        let user = create_unverified_user();
        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut reads = 0;
        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_user()
            .times(2)
            .returning(move |_| {
                reads += 1;
                if reads == 1 {
                    Ok(None)
                } else {
                    Err(domain::errors::DomainError::Persistence("down".into()))
                }
            });
        mock_verification_repo
            .expect_save()
            .times(1)
            .returning(|_| Ok(IssueOutcome::Withheld));

        let mut mock_email_service = MockEmailService::new();
        mock_email_service.expect_send_verification_email().times(0);
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

        assert!(matches!(
            use_case.execute("test@example.com").await,
            Err(ResendVerificationError::CooldownActive { seconds_remaining })
                if seconds_remaining == RESEND_COOLDOWN_MINUTES * 60
        ));
    }
}
