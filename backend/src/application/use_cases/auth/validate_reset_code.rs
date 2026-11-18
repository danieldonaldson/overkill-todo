//! Use case for validating a password reset code.

use crate::application::ports::UserRepository;
use crate::application::ports::{JwtService, PasswordHasher, PasswordResetRepository};
use crate::domain::entities::attempt_lockout::AttemptOutcome;
use crate::domain::entities::password_reset_code::MAX_ATTEMPTS;
use crate::domain::errors::DomainError;
use jiff::{SignedDuration, Timestamp};
use std::sync::Arc;

/// Duration in minutes for reset token validity.
const RESET_TOKEN_EXPIRY_MINUTES: i64 = 5;

/// Why a reset code was not accepted.
///
/// The handler records the outcome from these variants; the response comes
/// from the [`DomainError`] each one maps to, which is the same message the
/// endpoint has always returned.
#[derive(Debug)]
pub enum ResetCodeError {
    /// Unknown email, or no pending code. Deliberately one variant: the
    /// endpoint must not reveal whether an account exists.
    NoCode,
    /// The code has expired.
    Expired,
    /// The attempt cap is reached; the code was not checked.
    Locked,
    /// Wrong code. `attempts_remaining` is 0 for the guess that locks it.
    Invalid { attempts_remaining: i32 },
    /// Infrastructure failure.
    Domain(DomainError),
}

impl From<DomainError> for ResetCodeError {
    fn from(error: DomainError) -> Self {
        Self::Domain(error)
    }
}

impl From<ResetCodeError> for DomainError {
    fn from(error: ResetCodeError) -> Self {
        match error {
            ResetCodeError::NoCode => {
                Self::Validation("No reset code found. Please request a new one.".into())
            }
            ResetCodeError::Expired => {
                Self::Validation("Reset code has expired. Please request a new one.".into())
            }
            ResetCodeError::Locked => {
                Self::Validation("Too many failed attempts. Please request a new code.".into())
            }
            ResetCodeError::Invalid { .. } => Self::Validation("Invalid reset code.".into()),
            ResetCodeError::Domain(e) => e,
        }
    }
}

/// Use case for validating a password reset code.
///
/// After the user enters their 6-digit code, this validates it and returns
/// a short-lived JWT token that can be used to set the new password.
pub struct ValidateResetCodeUseCase {
    user_repo: Arc<dyn UserRepository>,
    password_reset_repo: Arc<dyn PasswordResetRepository>,
    password_hasher: Arc<dyn PasswordHasher>,
    jwt_service: Arc<dyn JwtService>,
}

impl ValidateResetCodeUseCase {
    /// Creates a new instance with the given dependencies.
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        password_reset_repo: Arc<dyn PasswordResetRepository>,
        password_hasher: Arc<dyn PasswordHasher>,
        jwt_service: Arc<dyn JwtService>,
    ) -> Self {
        Self {
            user_repo,
            password_reset_repo,
            password_hasher,
            jwt_service,
        }
    }

    /// Validates the reset code and returns a short-lived reset token.
    ///
    /// # Arguments
    ///
    /// * `email` - The email address
    /// * `code` - The 6-digit code entered by the user
    ///
    /// # Returns
    ///
    /// A JWT token valid for 5 minutes that can be used to reset the password.
    ///
    /// # Errors
    ///
    /// - [`ResetCodeError::NoCode`] for an unknown email or a known email with
    ///   no pending code (reported identically, to avoid an
    ///   account-enumeration oracle)
    /// - [`ResetCodeError::Expired`], [`ResetCodeError::Locked`] or
    ///   [`ResetCodeError::Invalid`] for a code that can't be accepted
    /// - [`ResetCodeError::Domain`] for infrastructure failures
    #[tracing::instrument(name = "usecase.validate_reset_code", skip_all)]
    pub async fn execute(&self, email: &str, code: &str) -> Result<String, ResetCodeError> {
        // Find the user by email. An unknown email returns the SAME generic
        // "no reset code" error as a known email without a valid code: the
        // request endpoint (`request_password_reset`) deliberately doesn't reveal
        // whether an account exists, and this endpoint must not undo that by
        // handing back a distinguishable NotFound for unknown emails.
        let Some(user) = self.user_repo.find_by_email(email).await? else {
            return Err(ResetCodeError::NoCode);
        };

        // Find the reset code for this user
        let reset_code = self
            .password_reset_repo
            .find_by_user(user.id())
            .await?
            .ok_or(ResetCodeError::NoCode)?;

        // Check if expired
        if reset_code.is_expired(Timestamp::now()) {
            return Err(ResetCodeError::Expired);
        }

        // Atomically claim an attempt slot *before* the expensive code
        // comparison: this single statement bounds the number of guesses
        // under concurrency (mirrors verify_email). `Locked` means the cap is
        // reached; `NotFound` means the row was deleted/expired between the load
        // above and here, which is a no-code case — not a lockout.
        let attempts_after = match self
            .password_reset_repo
            .register_attempt(user.id(), reset_code.id(), MAX_ATTEMPTS)
            .await?
        {
            AttemptOutcome::Claimed(attempts) => attempts,
            AttemptOutcome::Locked => return Err(ResetCodeError::Locked),
            AttemptOutcome::NotFound => return Err(ResetCodeError::NoCode),
        };

        // Verify the code using the password hasher
        let is_valid = self
            .password_hasher
            .verify(code, reset_code.code_hash())
            .await?;

        if !is_valid {
            // `Claimed` is only returned after incrementing while below the cap,
            // so this is in `0..MAX_ATTEMPTS`.
            return Err(ResetCodeError::Invalid {
                attempts_remaining: MAX_ATTEMPTS - attempts_after,
            });
        }

        // The code was live when read, but the Argon2 check takes time: refuse
        // it if it expired meanwhile, so no code is accepted from its expiry
        // instant on (P-19). The claim is by code id and a resend makes a new
        // one, so the expiry loaded above is still this code's.
        if reset_code.is_expired(Timestamp::now()) {
            return Err(ResetCodeError::Expired);
        }

        // Correct code: release the slot we claimed so a correct guess never
        // counts against the cap (e.g. a user re-validating after a downstream
        // step fails is not penalised). See verify_email for why claim-then-
        // release is preferred over increment-on-failure or a transaction across
        // the Argon2 verify; the residual one-attempt crash window is bounded and
        // self-healing via resend.
        self.password_reset_repo
            .release_attempt(user.id(), reset_code.id())
            .await?;

        // Code is valid - generate a short-lived JWT token (5 minutes) bound to
        // this reset-code row so it can only be consumed once.
        let expires_at = Timestamp::now() + SignedDuration::from_mins(RESET_TOKEN_EXPIRY_MINUTES);
        let token = self
            .jwt_service
            .create_reset_token(user.id(), reset_code.id(), expires_at)?;

        tracing::info!(user_id = %user.id(), "Reset code validated, token issued");

        Ok(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::jwt_service::MockJwtService;
    use crate::application::ports::outgoing::password_hasher::MockPasswordHasher;
    use crate::application::ports::outgoing::password_reset_repository::MockPasswordResetRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::PasswordResetCode;
    use crate::domain::entities::User;
    use crate::domain::value_objects::{Email, ResetCodeId};
    use jiff::{SignedDuration, Timestamp};

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
    async fn test_validate_reset_code_success() {
        let user = create_test_user();
        let user_id = *user.id();

        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Timestamp::now() + SignedDuration::from_mins(15),
            0,
            Timestamp::now(),
            Timestamp::now(),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(reset_code.clone())));
        // The slot is claimed before the compare, then released on success so a
        // correct code does not consume an attempt.
        mock_reset_repo
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Claimed(1)));
        mock_reset_repo
            .expect_release_attempt()
            .times(1)
            .returning(|_, _| Ok(()));

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher.expect_verify().returning(|_, _| Ok(true));

        let mut mock_jwt = MockJwtService::new();
        mock_jwt
            .expect_create_reset_token()
            .returning(|_, _, _| Ok("reset_token".to_string()));

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("test@example.com", "123456").await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "reset_token");
    }

    /// A code that expires while its Argon2 check runs is refused as expired,
    /// even though it was live when read and the guess was right (P-19).
    #[tokio::test]
    async fn code_expiring_during_the_check_is_refused() {
        let user = create_test_user();
        let user_id = *user.id();
        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Timestamp::now() + SignedDuration::from_millis(50),
            0,
            Timestamp::now(),
            Timestamp::now(),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));
        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(reset_code.clone())));
        mock_reset_repo
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Claimed(1)));
        mock_reset_repo.expect_release_attempt().never();
        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher.expect_verify().returning(|_, _| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            Ok(true)
        });
        let mut mock_jwt = MockJwtService::new();
        mock_jwt.expect_create_reset_token().never();

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_jwt),
        );
        let err = use_case
            .execute("test@example.com", "123456")
            .await
            .unwrap_err();
        assert!(matches!(err, ResetCodeError::Expired), "{err:?}");
    }

    #[tokio::test]
    async fn test_validate_reset_code_invalid() {
        let user = create_test_user();
        let user_id = *user.id();

        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Timestamp::now() + SignedDuration::from_mins(15),
            0,
            Timestamp::now(),
            Timestamp::now(),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(reset_code.clone())));
        // A wrong guess claims the slot (before the compare) and is NOT released.
        mock_reset_repo
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Claimed(1)));
        mock_reset_repo.expect_release_attempt().never();

        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher.expect_verify().returning(|_, _| Ok(false));

        let mock_jwt = MockJwtService::new();

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("test@example.com", "wrong!").await;
        match result {
            Err(ResetCodeError::Invalid { attempts_remaining }) => {
                assert_eq!(attempts_remaining, MAX_ATTEMPTS - 1);
            }
            other => panic!("Expected Invalid, got {other:?}"),
        }
    }

    /// A code whose `attempts` already reached the cap is rejected as locked
    /// (`register_attempt` returns `Locked`) and the expensive verify is never
    /// run.
    #[tokio::test]
    async fn test_validate_reset_code_locked() {
        let user = create_test_user();
        let user_id = *user.id();

        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Timestamp::now() + SignedDuration::from_mins(15),
            MAX_ATTEMPTS,
            Timestamp::now(),
            Timestamp::now(),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(reset_code.clone())));
        // The atomic claim returns `Locked` once the cap is reached.
        mock_reset_repo
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Locked));
        mock_reset_repo.expect_release_attempt().never();

        // verify must not be reached
        let mock_hasher = MockPasswordHasher::new();
        let mock_jwt = MockJwtService::new();

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("test@example.com", "123456").await;
        assert!(
            matches!(result, Err(ResetCodeError::Locked)),
            "Expected Locked, got {result:?}"
        );
    }

    /// Finding #2: when the code row is deleted between the load and the atomic
    /// claim (a concurrent successful reset, or expiry cleanup),
    /// `register_attempt` returns `NotFound` and the user sees a no-code message
    /// — not a misleading "too many attempts" lockout.
    #[tokio::test]
    async fn test_validate_reset_code_row_deleted_is_not_locked() {
        let user = create_test_user();
        let user_id = *user.id();

        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Timestamp::now() + SignedDuration::from_mins(15),
            0,
            Timestamp::now(),
            Timestamp::now(),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(reset_code.clone())));
        // The row vanished before the claim landed.
        mock_reset_repo
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::NotFound));
        mock_reset_repo.expect_release_attempt().never();

        // verify must not be reached
        let mock_hasher = MockPasswordHasher::new();
        let mock_jwt = MockJwtService::new();

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("test@example.com", "123456").await;
        assert!(
            matches!(result, Err(ResetCodeError::NoCode)),
            "Expected NoCode (not a lockout), got {result:?}"
        );
    }

    /// An unknown email must be indistinguishable from a known email with no
    /// pending code: same `Validation` variant, same generic message — no
    /// account-enumeration oracle.
    #[tokio::test]
    async fn test_validate_reset_code_unknown_email_is_generic() {
        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(|_| Ok(None));

        // Nothing downstream should run for an unknown email.
        let mock_reset_repo = MockPasswordResetRepository::new();
        let mock_hasher = MockPasswordHasher::new();
        let mock_jwt = MockJwtService::new();

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("nobody@example.com", "123456").await;
        assert!(
            matches!(result, Err(ResetCodeError::NoCode)),
            "Expected the generic NoCode, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_validate_reset_code_expired() {
        let user = create_test_user();
        let user_id = *user.id();

        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Timestamp::now() - SignedDuration::from_mins(1), // Expired
            0,
            Timestamp::now() - SignedDuration::from_mins(16),
            Timestamp::now() - SignedDuration::from_mins(16),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(reset_code.clone())));

        let mock_hasher = MockPasswordHasher::new();
        let mock_jwt = MockJwtService::new();

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(mock_jwt),
        );

        let result = use_case.execute("test@example.com", "123456").await;
        assert!(
            matches!(result, Err(ResetCodeError::Expired)),
            "Expected Expired, got {result:?}"
        );
    }

    /// The guess that uses the last attempt reports none remaining, which is
    /// how the handler tells a lockout from an ordinary wrong guess.
    #[tokio::test]
    async fn the_locking_guess_reports_no_attempts_remaining() {
        let user = create_test_user();
        let user_id = *user.id();
        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Timestamp::now() + SignedDuration::from_mins(15),
            MAX_ATTEMPTS - 1,
            Timestamp::now(),
            Timestamp::now(),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(user.clone())));
        let mut mock_reset_repo = MockPasswordResetRepository::new();
        mock_reset_repo
            .expect_find_by_user()
            .returning(move |_| Ok(Some(reset_code.clone())));
        mock_reset_repo
            .expect_register_attempt()
            .returning(|_, _, _| Ok(AttemptOutcome::Claimed(MAX_ATTEMPTS)));
        let mut mock_hasher = MockPasswordHasher::new();
        mock_hasher.expect_verify().returning(|_, _| Ok(false));

        let use_case = ValidateResetCodeUseCase::new(
            Arc::new(mock_user_repo),
            Arc::new(mock_reset_repo),
            Arc::new(mock_hasher),
            Arc::new(MockJwtService::new()),
        );
        let result = use_case.execute("test@example.com", "000000").await;
        assert!(
            matches!(
                result,
                Err(ResetCodeError::Invalid {
                    attempts_remaining: 0
                })
            ),
            "{result:?}"
        );
    }

    /// Each variant maps to exactly the message the endpoint returned before
    /// the error was typed, so responses don't change.
    #[test]
    fn errors_map_to_the_original_messages() {
        let cases = [
            (
                ResetCodeError::NoCode,
                "No reset code found. Please request a new one.",
            ),
            (
                ResetCodeError::Expired,
                "Reset code has expired. Please request a new one.",
            ),
            (
                ResetCodeError::Locked,
                "Too many failed attempts. Please request a new code.",
            ),
            (
                ResetCodeError::Invalid {
                    attempts_remaining: 3,
                },
                "Invalid reset code.",
            ),
            (
                ResetCodeError::Invalid {
                    attempts_remaining: 0,
                },
                "Invalid reset code.",
            ),
        ];
        for (error, message) in cases {
            match DomainError::from(error) {
                DomainError::Validation(msg) => assert_eq!(msg, message),
                other => panic!("expected Validation, got {other:?}"),
            }
        }
        assert!(matches!(
            DomainError::from(ResetCodeError::Domain(DomainError::Persistence("x".into()))),
            DomainError::Persistence(_)
        ));
    }
}
