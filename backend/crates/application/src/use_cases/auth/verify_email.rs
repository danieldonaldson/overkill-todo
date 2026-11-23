//! Use case for verifying a user's email address.

use crate::ports::UserRepository;
use crate::ports::{EmailVerificationRepository, PasswordHasher};
use domain::entities::User;
use domain::entities::attempt_lockout::AttemptOutcome;
use domain::entities::email_verification_code::MAX_ATTEMPTS;
use domain::errors::DomainError;
use std::sync::Arc;

/// Error details for verification failures.
#[derive(Debug)]
pub enum VerifyEmailError {
    /// User not found.
    NotFound(String),
    /// Code has expired.
    CodeExpired,
    /// Too many failed attempts.
    CodeLocked,
    /// Invalid code with attempts remaining.
    CodeInvalid { attempts_remaining: i32 },
    /// Email already verified.
    AlreadyVerified(Box<User>),
    /// Internal error.
    Internal(String),
}

impl From<VerifyEmailError> for DomainError {
    fn from(error: VerifyEmailError) -> Self {
        match error {
            VerifyEmailError::NotFound(msg) => Self::NotFound(msg),
            VerifyEmailError::CodeExpired => {
                Self::Validation("Verification code has expired. Please request a new one.".into())
            }
            VerifyEmailError::CodeLocked => {
                Self::Validation("Too many failed attempts. Please request a new code.".into())
            }
            VerifyEmailError::CodeInvalid { attempts_remaining } => Self::Validation(format!(
                "Invalid verification code. {attempts_remaining} attempts remaining."
            )),
            VerifyEmailError::AlreadyVerified(_) => {
                Self::Validation("Email is already verified.".into())
            }
            VerifyEmailError::Internal(msg) => Self::Persistence(msg),
        }
    }
}

/// Use case for verifying a user's email address.
///
/// Validates the 6-digit code entered by the user and marks their email as verified.
pub struct VerifyEmailUseCase {
    user_repo: Arc<dyn UserRepository>,
    email_verification_repo: Arc<dyn EmailVerificationRepository>,
    password_hasher: Arc<dyn PasswordHasher>,
}

impl VerifyEmailUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        email_verification_repo: Arc<dyn EmailVerificationRepository>,
        password_hasher: Arc<dyn PasswordHasher>,
    ) -> Self {
        Self {
            user_repo,
            email_verification_repo,
            password_hasher,
        }
    }

    /// Verifies a user's email using the provided code.
    ///
    /// # Arguments
    ///
    /// * `email` - The email address to verify
    /// * `code` - The 6-digit code entered by the user
    ///
    /// # Returns
    ///
    /// The verified user on success, which can be used to auto-login.
    ///
    /// # Errors
    ///
    /// Returns `VerifyEmailError` for specific error types that the handler
    /// can use to provide detailed feedback to the frontend.
    #[tracing::instrument(name = "usecase.verify_email", skip_all)]
    pub async fn execute(&self, email: &str, code: &str) -> Result<User, VerifyEmailError> {
        // Find the user by email
        let user = self
            .user_repo
            .find_by_email(email)
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?
            .ok_or_else(|| VerifyEmailError::NotFound("No account with this email".into()))?;

        // Check if already verified - return the user (not an error for the handler)
        if user.email_verified() {
            return Err(VerifyEmailError::AlreadyVerified(Box::new(user)));
        }

        // Find the verification code for this user
        let verification_code = self
            .email_verification_repo
            .find_by_user(user.id())
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?
            .ok_or(VerifyEmailError::CodeExpired)?;

        // Check if expired
        if verification_code.is_expired(jiff::Timestamp::now()) {
            return Err(VerifyEmailError::CodeExpired);
        }

        // Atomically claim an attempt slot *before* the expensive code
        // comparison: this single statement bounds the number of guesses
        // under concurrency. `Locked` means the cap is reached; `NotFound` means
        // the row was deleted/expired between the load above and here, which is
        // an expired code — not a lockout.
        let attempts_after = match self
            .email_verification_repo
            .register_attempt(user.id(), verification_code.id(), MAX_ATTEMPTS)
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?
        {
            AttemptOutcome::Claimed(attempts) => attempts,
            AttemptOutcome::Locked => {
                // Investigation event (OT-007 D7). `flow` matches the metric label.
                tracing::info!(
                    event = "auth.code.refused_locked",
                    flow = "email_code",
                    user_id = %user.id(),
                    "Code check refused: code is locked"
                );
                return Err(VerifyEmailError::CodeLocked);
            }
            AttemptOutcome::NotFound => return Err(VerifyEmailError::CodeExpired),
        };

        // Verify the code using the password hasher
        let is_valid = self
            .password_hasher
            .verify(code, verification_code.code_hash())
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?;

        if !is_valid {
            // `register_attempt` only returns `Claimed` after incrementing while
            // below the cap, so `attempts_after` is in `1..=MAX_ATTEMPTS` and this
            // subtraction is always >= 0.
            let attempts_remaining = MAX_ATTEMPTS - attempts_after;
            if attempts_remaining == 0 {
                tracing::warn!(
                    event = "auth.code.locked",
                    flow = "email_code",
                    user_id = %user.id(),
                    "Code locked after too many wrong guesses"
                );
            }

            return Err(VerifyEmailError::CodeInvalid { attempts_remaining });
        }

        // The code was live when read, but the Argon2 check takes time: refuse
        // it if it expired meanwhile, so no code is accepted from its expiry
        // instant on (P-19). The claim is by code id and a resend makes a new
        // one, so the expiry loaded above is still this code's.
        if verification_code.is_expired(jiff::Timestamp::now()) {
            return Err(VerifyEmailError::CodeExpired);
        }

        // Correct code: release the attempt slot we claimed above so a correct
        // guess never counts against the cap, and a transient failure on the
        // steps below can be retried without self-locking the user.
        //
        // DESIGN: we deliberately claim-then-release rather than (a) increment
        // only on a wrong guess — which loses atomicity, letting concurrent
        // guesses read a stale `attempts = 0` and slip past the cap — or (b)
        // wrap claim+verify+release in a transaction, which would hold a DB
        // connection open across the CPU-bound Argon2 verify. A crash strictly
        // between the claim and this release leaks at most one attempt, which is
        // self-healing: a resend resets `attempts` after the cooldown, well
        // inside the lock window.
        self.email_verification_repo
            .release_attempt(user.id(), verification_code.id())
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?;

        // Code is valid - mark email as verified
        self.user_repo
            .mark_email_verified(user.id())
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?;

        // Delete the verification code
        self.email_verification_repo
            .delete_for_user(user.id())
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?;

        // Reload the user to get updated email_verified status
        let verified_user = self
            .user_repo
            .find_by_id(user.id())
            .await
            .map_err(|e| VerifyEmailError::Internal(e.to_string()))?
            .ok_or_else(|| {
                VerifyEmailError::Internal("User not found after verification".into())
            })?;

        tracing::info!(user_id = %user.id(), "Email verified successfully");

        Ok(verified_user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ports::outgoing::email_verification_repository::MockEmailVerificationRepository;
    use crate::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::ports::outgoing::user_repository::MockUserRepository;
    use domain::entities::{EmailVerificationCode, UserBuilder};
    use domain::value_objects::{UserId, VerificationCodeId};
    use jiff::{SignedDuration, Timestamp};

    const EMAIL: &str = "verify@example.com";

    fn user(id: UserId, email_verified: bool) -> User {
        UserBuilder::new()
            .id(id)
            .email(EMAIL)
            .password_hash(None)
            .name("", "")
            .email_verified(email_verified)
            .build()
    }

    fn valid_code(user_id: UserId) -> EmailVerificationCode {
        EmailVerificationCode::new(user_id, "code-hash".to_string())
    }

    fn expired_code(user_id: UserId) -> EmailVerificationCode {
        EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            user_id,
            "code-hash".to_string(),
            Timestamp::now() - SignedDuration::from_hours(1),
            0,
            Timestamp::now() - SignedDuration::from_hours(25),
            Timestamp::now() - SignedDuration::from_hours(25),
            None,
            None,
        )
    }

    fn hasher_verifying(result: bool) -> MockPasswordHasher {
        let mut h = MockPasswordHasher::new();
        h.expect_verify().returning(move |_, _| Ok(result));
        h
    }

    #[tokio::test]
    async fn rejects_unknown_email() {
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email().returning(|_| Ok(None));

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(MockEmailVerificationRepository::new()),
            Arc::new(MockPasswordHasher::new()),
        );

        let err = uc.execute(EMAIL, "123456").await.unwrap_err();
        assert!(matches!(err, VerifyEmailError::NotFound(_)));
    }

    #[tokio::test]
    async fn returns_already_verified() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, true))));

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(MockEmailVerificationRepository::new()),
            Arc::new(MockPasswordHasher::new()),
        );

        let err = uc.execute(EMAIL, "123456").await.unwrap_err();
        assert!(matches!(err, VerifyEmailError::AlreadyVerified(_)));
    }

    #[tokio::test]
    async fn missing_code_is_expired() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));

        let mut codes = MockEmailVerificationRepository::new();
        codes.expect_find_by_user().returning(|_| Ok(None));

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(codes),
            Arc::new(MockPasswordHasher::new()),
        );

        let err = uc.execute(EMAIL, "123456").await.unwrap_err();
        assert!(matches!(err, VerifyEmailError::CodeExpired));
    }

    #[tokio::test]
    async fn expired_code_is_rejected() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));

        let mut codes = MockEmailVerificationRepository::new();
        codes
            .expect_find_by_user()
            .returning(move |_| Ok(Some(expired_code(user_id))));
        // We must not even claim an attempt slot on an already-expired code.
        codes.expect_register_attempt().never();

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(codes),
            Arc::new(MockPasswordHasher::new()),
        );

        let err = uc.execute(EMAIL, "123456").await.unwrap_err();
        assert!(matches!(err, VerifyEmailError::CodeExpired));
    }

    #[tokio::test]
    async fn locked_when_attempts_exhausted() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));

        let mut codes = MockEmailVerificationRepository::new();
        codes
            .expect_find_by_user()
            .returning(move |_| Ok(Some(valid_code(user_id))));
        codes
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Locked));

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(codes),
            Arc::new(MockPasswordHasher::new()),
        );

        let err = uc.execute(EMAIL, "123456").await.unwrap_err();
        assert!(matches!(err, VerifyEmailError::CodeLocked));
    }

    /// A concurrent delete can remove the code row between load and claim;
    /// `register_attempt` returns `NotFound`, which we surface as expired rather
    /// than a misleading lockout.
    #[tokio::test]
    async fn register_attempt_not_found_is_expired() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));

        let mut codes = MockEmailVerificationRepository::new();
        codes
            .expect_find_by_user()
            .returning(move |_| Ok(Some(valid_code(user_id))));
        codes
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::NotFound));

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(codes),
            Arc::new(MockPasswordHasher::new()),
        );

        let err = uc.execute(EMAIL, "123456").await.unwrap_err();
        assert!(matches!(err, VerifyEmailError::CodeExpired));
    }

    #[tokio::test]
    async fn wrong_code_reports_remaining_attempts() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));
        // A wrong guess must not verify the email or release the slot.
        repo.expect_mark_email_verified().never();

        let mut codes = MockEmailVerificationRepository::new();
        codes
            .expect_find_by_user()
            .returning(move |_| Ok(Some(valid_code(user_id))));
        codes
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Claimed(2)));
        codes.expect_release_attempt().never();

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(codes),
            Arc::new(hasher_verifying(false)),
        );

        let err = uc.execute(EMAIL, "wrong0").await.unwrap_err();
        match err {
            VerifyEmailError::CodeInvalid { attempts_remaining } => {
                assert_eq!(attempts_remaining, MAX_ATTEMPTS - 2);
            }
            other => panic!("expected CodeInvalid, got {other:?}"),
        }
    }

    /// A code that expires while its Argon2 check runs is refused as expired,
    /// even though it was live when read and the guess was right (P-19).
    #[tokio::test]
    async fn code_expiring_during_the_check_is_refused() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));
        repo.expect_mark_email_verified().never();

        let expires_at = Timestamp::now() + SignedDuration::from_millis(50);
        let mut codes = MockEmailVerificationRepository::new();
        codes.expect_find_by_user().returning(move |_| {
            Ok(Some(EmailVerificationCode::from_persistence(
                VerificationCodeId::new(),
                user_id,
                "code-hash".to_string(),
                expires_at,
                0,
                Timestamp::now(),
                Timestamp::now(),
                None,
                None,
            )))
        });
        codes
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Claimed(1)));
        codes.expect_release_attempt().never();
        codes.expect_delete_for_user().never();

        let mut hasher = MockPasswordHasher::new();
        hasher.expect_verify().returning(|_, _| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(true)
        });

        let uc = VerifyEmailUseCase::new(Arc::new(repo), Arc::new(codes), Arc::new(hasher));
        let err = uc.execute(EMAIL, "123456").await.unwrap_err();
        assert!(matches!(err, VerifyEmailError::CodeExpired), "{err:?}");
    }

    /// 5 wrong guesses then 3 more: exactly one `auth.code.locked` (on the
    /// fifth guess) and three `auth.code.refused_locked`, each naming the flow
    /// and the account and nothing else identifying (OT-007 D7).
    #[tokio::test]
    async fn lockout_and_refused_checks_are_logged() {
        use super::super::log_capture::LogCapture;
        use std::sync::atomic::{AtomicI32, Ordering};

        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));

        let attempts = Arc::new(AtomicI32::new(0));
        let mut codes = MockEmailVerificationRepository::new();
        codes
            .expect_find_by_user()
            .returning(move |_| Ok(Some(valid_code(user_id))));
        codes.expect_register_attempt().returning(move |_, _, max| {
            let n = attempts.load(Ordering::SeqCst);
            if n >= max {
                return Ok(AttemptOutcome::Locked);
            }
            attempts.store(n + 1, Ordering::SeqCst);
            Ok(AttemptOutcome::Claimed(n + 1))
        });

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(codes),
            Arc::new(hasher_verifying(false)),
        );

        let logs = LogCapture::install();
        for _ in 0..8 {
            let _ = uc.execute(EMAIL, "000000").await;
        }

        let locked = logs.events("auth.code.locked");
        let refused = logs.events("auth.code.refused_locked");
        assert_eq!(locked.len(), 1, "{locked:?}");
        assert_eq!(refused.len(), 3, "{refused:?}");
        assert_eq!(locked[0]["level"], "WARN");
        for event in locked.iter().chain(&refused) {
            assert_eq!(event["flow"], "email_code");
            assert_eq!(event["user_id"], user_id.to_string());
        }
        for event in &refused {
            assert_eq!(event["level"], "INFO");
        }
        let text = serde_json::to_string(&logs.all()).unwrap();
        assert!(!text.contains(EMAIL) && !text.contains("000000"), "{text}");
    }

    #[tokio::test]
    async fn correct_code_verifies_and_releases_slot() {
        let user_id = UserId::new();
        let mut repo = MockUserRepository::new();
        repo.expect_find_by_email()
            .returning(move |_| Ok(Some(user(user_id, false))));
        repo.expect_mark_email_verified()
            .times(1)
            .returning(|_| Ok(()));
        // Reload after verification returns the now-verified user.
        repo.expect_find_by_id()
            .returning(move |_| Ok(Some(user(user_id, true))));

        let mut codes = MockEmailVerificationRepository::new();
        codes
            .expect_find_by_user()
            .returning(move |_| Ok(Some(valid_code(user_id))));
        codes
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Claimed(1)));
        // Correct code releases the claimed slot and deletes the code.
        codes
            .expect_release_attempt()
            .times(1)
            .returning(|_, _| Ok(()));
        codes
            .expect_delete_for_user()
            .times(1)
            .returning(|_| Ok(()));

        let uc = VerifyEmailUseCase::new(
            Arc::new(repo),
            Arc::new(codes),
            Arc::new(hasher_verifying(true)),
        );

        let verified = uc.execute(EMAIL, "123456").await.unwrap();
        assert!(verified.email_verified());
    }
}
