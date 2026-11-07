//! Use case for validating a password reset code.

use crate::application::ports::UserRepository;
use crate::application::ports::{JwtService, PasswordHasher, PasswordResetRepository};
use crate::domain::entities::attempt_lockout::AttemptOutcome;
use crate::domain::entities::password_reset_code::MAX_ATTEMPTS;
use crate::domain::errors::DomainError;
use chrono::{Duration, Utc};
use std::sync::Arc;

/// Duration in minutes for reset token validity.
const RESET_TOKEN_EXPIRY_MINUTES: i64 = 5;

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
    /// - `Validation` if the code is invalid, expired, locked, or the email has no
    ///   pending reset code (an unknown email is reported identically to a known
    ///   email with no code, to avoid an account-enumeration oracle)
    #[tracing::instrument(
        name = "usecase.validate_reset_code",
        skip(self, code),
        fields(email = %email)
    )]
    pub async fn execute(&self, email: &str, code: &str) -> Result<String, DomainError> {
        // Find the user by email. An unknown email returns the SAME generic
        // "no reset code" error as a known email without a valid code: the
        // request endpoint (`request_password_reset`) deliberately doesn't reveal
        // whether an account exists, and this endpoint must not undo that by
        // handing back a distinguishable NotFound for unknown emails.
        let Some(user) = self.user_repo.find_by_email(email).await? else {
            return Err(DomainError::Validation(
                "No reset code found. Please request a new one.".into(),
            ));
        };

        // Find the reset code for this user
        let reset_code = self
            .password_reset_repo
            .find_by_user(user.id())
            .await?
            .ok_or_else(|| {
                DomainError::Validation("No reset code found. Please request a new one.".into())
            })?;

        // Check if expired
        if reset_code.is_expired() {
            return Err(DomainError::Validation(
                "Reset code has expired. Please request a new one.".into(),
            ));
        }

        // Atomically claim an attempt slot *before* the expensive code
        // comparison: this single statement bounds the number of guesses
        // under concurrency (mirrors verify_email). `Locked` means the cap is
        // reached; `NotFound` means the row was deleted/expired between the load
        // above and here, which is a no-code case — not a lockout.
        match self
            .password_reset_repo
            .register_attempt(user.id(), MAX_ATTEMPTS)
            .await?
        {
            AttemptOutcome::Claimed(_) => {}
            AttemptOutcome::Locked => {
                return Err(DomainError::Validation(
                    "Too many failed attempts. Please request a new code.".into(),
                ));
            }
            AttemptOutcome::NotFound => {
                return Err(DomainError::Validation(
                    "No reset code found. Please request a new one.".into(),
                ));
            }
        }

        // Verify the code using the password hasher
        let is_valid = self
            .password_hasher
            .verify(code, reset_code.code_hash())
            .await?;

        if !is_valid {
            return Err(DomainError::Validation("Invalid reset code.".into()));
        }

        // Correct code: release the slot we claimed so a correct guess never
        // counts against the cap (e.g. a user re-validating after a downstream
        // step fails is not penalised). See verify_email for why claim-then-
        // release is preferred over increment-on-failure or a transaction across
        // the Argon2 verify; the residual one-attempt crash window is bounded and
        // self-healing via resend.
        self.password_reset_repo.release_attempt(user.id()).await?;

        // Code is valid - generate a short-lived JWT token (5 minutes) bound to
        // this reset-code row so it can only be consumed once.
        let expires_at = Utc::now() + Duration::minutes(RESET_TOKEN_EXPIRY_MINUTES);
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
    use chrono::{Duration, Utc};

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
            Utc::now() + Duration::minutes(15),
            0,
            Utc::now(),
            Utc::now(),
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
            .returning(|_, _| Ok(AttemptOutcome::Claimed(1)));
        mock_reset_repo
            .expect_release_attempt()
            .times(1)
            .returning(|_| Ok(()));

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

    #[tokio::test]
    async fn test_validate_reset_code_invalid() {
        let user = create_test_user();
        let user_id = *user.id();

        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Utc::now() + Duration::minutes(15),
            0,
            Utc::now(),
            Utc::now(),
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
            .returning(|_, _| Ok(AttemptOutcome::Claimed(1)));
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
        assert!(result.is_err());
        match result {
            Err(DomainError::Validation(msg)) => assert!(msg.contains("Invalid")),
            _ => panic!("Expected Validation error"),
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
            Utc::now() + Duration::minutes(15),
            MAX_ATTEMPTS,
            Utc::now(),
            Utc::now(),
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
            .returning(|_, _| Ok(AttemptOutcome::Locked));
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
        match result {
            Err(DomainError::Validation(msg)) => assert!(msg.contains("Too many")),
            _ => panic!("Expected Validation (locked) error"),
        }
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
            Utc::now() + Duration::minutes(15),
            0,
            Utc::now(),
            Utc::now(),
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
            .returning(|_, _| Ok(AttemptOutcome::NotFound));
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
        match result {
            Err(DomainError::Validation(msg)) => {
                assert!(msg.contains("No reset code"), "got: {msg}");
                assert!(
                    !msg.contains("Too many"),
                    "must not report a lockout: {msg}"
                );
            }
            other => panic!("Expected Validation (no-code) error, got {other:?}"),
        }
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
        match result {
            Err(DomainError::Validation(msg)) => {
                assert!(msg.contains("No reset code"), "got: {msg}");
            }
            other => panic!("Expected generic Validation error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_validate_reset_code_expired() {
        let user = create_test_user();
        let user_id = *user.id();

        let reset_code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            user_id,
            "hashed_code".to_string(),
            Utc::now() - Duration::minutes(1), // Expired
            0,
            Utc::now() - Duration::minutes(16),
            Utc::now() - Duration::minutes(16),
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
        assert!(result.is_err());
        match result {
            Err(DomainError::Validation(msg)) => assert!(msg.contains("expired")),
            _ => panic!("Expected Validation error"),
        }
    }
}
