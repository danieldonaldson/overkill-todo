//! Magic link verification use case.
//!
//! Verifies a user's email using a magic link token (backup to 6-digit code).
//! The magic link is a one-click verification option for web users.

use crate::application::ports::EmailVerificationRepository;
use crate::application::ports::UserRepository;
use crate::domain::entities::User;
use crate::domain::errors::DomainError;
use sha2::{Digest, Sha256};
use std::sync::Arc;

/// Error details for magic link verification failures.
#[derive(Debug)]
pub enum VerifyMagicLinkError {
    /// Token not found or invalid.
    InvalidToken,
    /// Token has expired.
    TokenExpired,
    /// Token is locked (too many code attempts).
    TokenLocked,
    /// Email already verified.
    AlreadyVerified(Box<User>),
    /// Internal error.
    Internal(String),
}

impl From<VerifyMagicLinkError> for DomainError {
    fn from(error: VerifyMagicLinkError) -> Self {
        match error {
            VerifyMagicLinkError::InvalidToken => {
                Self::Validation("Invalid or expired magic link.".into())
            }
            VerifyMagicLinkError::TokenExpired => {
                Self::Validation("This magic link has expired. Please request a new one.".into())
            }
            VerifyMagicLinkError::TokenLocked => {
                Self::Validation("Too many failed attempts. Please request a new code.".into())
            }
            VerifyMagicLinkError::AlreadyVerified(_) => {
                Self::Validation("Email is already verified.".into())
            }
            VerifyMagicLinkError::Internal(msg) => Self::Persistence(msg),
        }
    }
}

/// Use case for verifying a user's email via magic link.
///
/// Magic links are a backup verification method (alternative to 6-digit codes).
/// They are SHA-256 hashed since the token is a random UUID (not guessable).
pub struct VerifyMagicLinkUseCase {
    user_repo: Arc<dyn UserRepository>,
    email_verification_repo: Arc<dyn EmailVerificationRepository>,
}

impl VerifyMagicLinkUseCase {
    /// Creates a new instance with the given dependencies.
    #[must_use]
    pub fn new(
        user_repo: Arc<dyn UserRepository>,
        email_verification_repo: Arc<dyn EmailVerificationRepository>,
    ) -> Self {
        Self {
            user_repo,
            email_verification_repo,
        }
    }

    /// Verifies a user's email using the provided magic link token.
    ///
    /// # Arguments
    ///
    /// * `token` - The magic link token (UUID) from the email link
    ///
    /// # Returns
    ///
    /// The verified user on success, which can be used to issue a signup token.
    ///
    /// # Errors
    ///
    /// Returns `VerifyMagicLinkError` for specific error types.
    #[tracing::instrument(name = "usecase.verify_magic_link", skip(self, token))]
    pub async fn execute(&self, token: &str) -> Result<User, VerifyMagicLinkError> {
        // Hash the token with SHA-256 (magic link tokens are random UUIDs, not guessable)
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let token_hash = hex::encode(hasher.finalize());

        // Find the verification code by magic token hash
        let verification_code = self
            .email_verification_repo
            .find_by_magic_token_hash(&token_hash)
            .await
            .map_err(|e| VerifyMagicLinkError::Internal(e.to_string()))?
            .ok_or(VerifyMagicLinkError::InvalidToken)?;

        // Check if the magic link is expired. Uses the token's own expiry, not
        // the 6-digit code's `expires_at`, so resending a code never extends or
        // revives a previously-issued link.
        if verification_code.is_magic_token_expired() {
            return Err(VerifyMagicLinkError::TokenExpired);
        }

        // Check if locked (too many attempts on the 6-digit code)
        if verification_code.is_locked() {
            return Err(VerifyMagicLinkError::TokenLocked);
        }

        // Get the user
        let user = self
            .user_repo
            .find_by_id(verification_code.user_id())
            .await
            .map_err(|e| VerifyMagicLinkError::Internal(e.to_string()))?
            .ok_or_else(|| VerifyMagicLinkError::Internal("User not found".into()))?;

        // Check if already verified
        if user.email_verified() {
            return Err(VerifyMagicLinkError::AlreadyVerified(Box::new(user)));
        }

        // Mark email as verified
        self.user_repo
            .mark_email_verified(user.id())
            .await
            .map_err(|e| VerifyMagicLinkError::Internal(e.to_string()))?;

        // Delete the verification code
        self.email_verification_repo
            .delete_for_user(user.id())
            .await
            .map_err(|e| VerifyMagicLinkError::Internal(e.to_string()))?;

        // Reload the user to get updated email_verified status
        let verified_user = self
            .user_repo
            .find_by_id(user.id())
            .await
            .map_err(|e| VerifyMagicLinkError::Internal(e.to_string()))?
            .ok_or_else(|| {
                VerifyMagicLinkError::Internal("User not found after verification".into())
            })?;

        tracing::info!(user_id = %user.id(), "Email verified via magic link");

        Ok(verified_user)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::outgoing::email_verification_repository::MockEmailVerificationRepository;
    use crate::application::ports::outgoing::user_repository::MockUserRepository;
    use crate::domain::entities::{EmailVerificationCode, UserBuilder};
    use crate::domain::value_objects::{UserId, VerificationCodeId};
    use chrono::{Duration, Utc};

    fn create_test_user(verified: bool) -> User {
        UserBuilder::new()
            .password_hash(Some("password_hash"))
            .name("John", "Doe")
            .email_verified(verified)
            .build()
    }

    fn hash_token(token: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        hex::encode(hasher.finalize())
    }

    #[tokio::test]
    async fn test_verify_magic_link_success() {
        let user = create_test_user(false);
        let user_id = *user.id();
        let verified_user = create_test_user(true);
        let token = "test-magic-token-uuid";
        let token_hash = hash_token(token);

        let verification_code = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            user_id,
            "code_hash".to_string(), // Not used for magic link verification
            Utc::now() + Duration::hours(24),
            0,
            Utc::now(),
            Utc::now(),
            Some(token_hash),
            Some(Utc::now() + Duration::hours(24)),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(user.clone())));
        mock_user_repo
            .expect_mark_email_verified()
            .times(1)
            .returning(|_| Ok(()));
        mock_user_repo
            .expect_find_by_id()
            .times(1)
            .returning(move |_| Ok(Some(verified_user.clone())));

        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_magic_token_hash()
            .returning(move |_| Ok(Some(verification_code.clone())));
        mock_verification_repo
            .expect_delete_for_user()
            .returning(|_| Ok(()));

        let use_case =
            VerifyMagicLinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_verification_repo));

        let result = use_case.execute(token).await;
        assert!(result.is_ok());
        assert!(result.unwrap().email_verified());
    }

    #[tokio::test]
    async fn test_verify_magic_link_invalid_token() {
        let mock_user_repo = MockUserRepository::new();

        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_magic_token_hash()
            .returning(|_| Ok(None)); // Token not found

        let use_case =
            VerifyMagicLinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_verification_repo));

        let result = use_case.execute("invalid-token").await;
        assert!(matches!(result, Err(VerifyMagicLinkError::InvalidToken)));
    }

    #[tokio::test]
    async fn test_verify_magic_link_expired() {
        let user_id = UserId::new();
        let token = "expired-token";
        let token_hash = hash_token(token);

        let verification_code = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            user_id,
            "code_hash".to_string(),
            Utc::now() - Duration::hours(1), // Expired
            0,
            Utc::now() - Duration::hours(25),
            Utc::now() - Duration::hours(25),
            Some(token_hash),
            Some(Utc::now() - Duration::hours(1)), // magic link expired
        );

        let mock_user_repo = MockUserRepository::new();

        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_magic_token_hash()
            .returning(move |_| Ok(Some(verification_code.clone())));

        let use_case =
            VerifyMagicLinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_verification_repo));

        let result = use_case.execute(token).await;
        assert!(matches!(result, Err(VerifyMagicLinkError::TokenExpired)));
    }

    #[tokio::test]
    async fn test_verify_magic_link_token_locked() {
        // A valid, unexpired magic-link token whose underlying 6-digit code is at
        // the attempt cap must surface `TokenLocked`. The lock check sits between
        // the expiry check and the user lookup, so only the code lookup is hit.
        use crate::domain::entities::attempt_lockout::MAX_ATTEMPTS;

        let user_id = UserId::new();
        let token = "locked-token";
        let token_hash = hash_token(token);

        let verification_code = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            user_id,
            "code_hash".to_string(),
            Utc::now() + Duration::hours(24), // code not expired
            MAX_ATTEMPTS,                     // at the attempt cap → locked
            Utc::now(),                       // last_sent_at within lock window
            Utc::now(),
            Some(token_hash),
            Some(Utc::now() + Duration::hours(24)), // magic link not expired
        );

        let mock_user_repo = MockUserRepository::new();

        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_magic_token_hash()
            .returning(move |_| Ok(Some(verification_code.clone())));

        let use_case =
            VerifyMagicLinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_verification_repo));

        let result = use_case.execute(token).await;
        assert!(matches!(result, Err(VerifyMagicLinkError::TokenLocked)));
    }

    #[tokio::test]
    async fn test_verify_magic_link_already_verified() {
        let user = create_test_user(true); // Already verified
        let user_id = *user.id();
        let token = "test-token";
        let token_hash = hash_token(token);

        let verification_code = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            user_id,
            "code_hash".to_string(),
            Utc::now() + Duration::hours(24),
            0,
            Utc::now(),
            Utc::now(),
            Some(token_hash),
            Some(Utc::now() + Duration::hours(24)),
        );

        let mut mock_user_repo = MockUserRepository::new();
        mock_user_repo
            .expect_find_by_id()
            .returning(move |_| Ok(Some(user.clone())));

        let mut mock_verification_repo = MockEmailVerificationRepository::new();
        mock_verification_repo
            .expect_find_by_magic_token_hash()
            .returning(move |_| Ok(Some(verification_code.clone())));

        let use_case =
            VerifyMagicLinkUseCase::new(Arc::new(mock_user_repo), Arc::new(mock_verification_repo));

        let result = use_case.execute(token).await;
        assert!(matches!(
            result,
            Err(VerifyMagicLinkError::AlreadyVerified(_))
        ));
    }
}
