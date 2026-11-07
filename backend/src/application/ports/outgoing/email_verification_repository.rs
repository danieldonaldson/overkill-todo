//! Email verification code repository port.

use crate::domain::entities::EmailVerificationCode;
use crate::domain::entities::attempt_lockout::AttemptOutcome;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Repository trait for email verification code persistence operations.
///
/// This trait defines the contract for storing and managing email verification codes.
/// Codes are stored with their Argon2 hash (never plaintext) for security.
///
/// # Security
///
/// - 6-digit codes are hashed with Argon2 because they are guessable
/// - Only one code per user (UNIQUE constraint on `user_id`)
/// - Requesting a new code replaces the old one (upsert semantics)
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
#[async_trait]
pub trait EmailVerificationRepository: Send + Sync {
    /// Persists or updates an email verification code.
    ///
    /// If a code already exists for the user, it is replaced (upsert).
    /// The code should already have its value hashed before calling this method.
    async fn save(&self, code: &EmailVerificationCode) -> Result<(), DomainError>;

    /// Finds an email verification code by user ID.
    async fn find_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Option<EmailVerificationCode>, DomainError>;

    /// Deletes the verification code for a user.
    ///
    /// Called after successful verification.
    async fn delete_for_user(&self, user_id: &UserId) -> Result<(), DomainError>;

    /// Atomically records an attempt against the failure cap.
    ///
    /// Increments `attempts` only while it is strictly below `max_attempts`,
    /// returning [`AttemptOutcome::Claimed`] with the resulting count. Returns
    /// [`AttemptOutcome::Locked`] when the row exists but is already at the cap,
    /// and [`AttemptOutcome::NotFound`] when no code row exists (so callers can
    /// surface an expired/no-code message instead of a misleading lockout when a
    /// concurrent delete races the claim).
    ///
    /// INVARIANT: claim the slot with this single statement *before* the
    /// expensive code comparison so concurrent guesses can't all read a stale
    /// `attempts = 0` and slip past the cap.
    async fn register_attempt(
        &self,
        user_id: &UserId,
        max_attempts: i32,
    ) -> Result<AttemptOutcome, DomainError>;

    /// Releases a previously-claimed attempt slot after a *successful*
    /// verification.
    ///
    /// Decrements `attempts` (never below zero) so a correct code does not
    /// permanently consume one of the capped attempts and a transient failure on
    /// the success path can be retried without tripping the lockout. Pairs with
    /// `register_attempt`, which claims the slot before the expensive compare.
    async fn release_attempt(&self, user_id: &UserId) -> Result<(), DomainError>;

    /// Overwrites `last_sent_at` for a user's code.
    ///
    /// Used to back-date the stamp after a delivery failure: `last_sent_at` is
    /// written when the code is *issued*, but it gates the resend cooldown, so
    /// an email that never went out would otherwise lock the user out of the one
    /// recovery path for [`RESEND_COOLDOWN_MINUTES`]. The caller supplies the
    /// timestamp, keeping the cooldown policy in the domain.
    ///
    /// [`RESEND_COOLDOWN_MINUTES`]: crate::domain::entities::attempt_lockout::RESEND_COOLDOWN_MINUTES
    async fn set_last_sent_at(
        &self,
        user_id: &UserId,
        last_sent_at: DateTime<Utc>,
    ) -> Result<(), DomainError>;

    /// Updates the code for a resend operation.
    ///
    /// Resets attempts, updates `code_hash`, `last_sent_at`, and `expires_at`.
    async fn update_for_resend(
        &self,
        user_id: &UserId,
        code_hash: String,
        expires_at: DateTime<Utc>,
    ) -> Result<(), DomainError>;

    /// Deletes all expired codes from the database.
    ///
    /// This is a cleanup operation that should be run periodically.
    /// Returns the number of codes deleted.
    async fn delete_expired(&self) -> Result<u64, DomainError>;

    /// Saves the magic token hash for an existing verification code.
    ///
    /// The magic token is a random UUID that can be used as a one-click
    /// verification link (backup to 6-digit code).
    async fn save_magic_token(
        &self,
        user_id: &UserId,
        magic_token_hash: String,
    ) -> Result<(), DomainError>;

    /// Finds a verification code by its magic token hash.
    ///
    /// Used to verify magic link tokens.
    async fn find_by_magic_token_hash(
        &self,
        magic_token_hash: &str,
    ) -> Result<Option<EmailVerificationCode>, DomainError>;
}
