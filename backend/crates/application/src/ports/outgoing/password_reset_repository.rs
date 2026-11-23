//! Password reset code repository port.

use async_trait::async_trait;
use domain::entities::PasswordResetCode;
use domain::entities::attempt_lockout::{AttemptOutcome, IssueOutcome};
use domain::errors::DomainError;
use domain::value_objects::{ResetCodeId, UserId};

/// Repository trait for password reset code persistence operations.
///
/// This trait defines the contract for storing and managing password reset codes.
/// Codes are stored with their Argon2 hash (never plaintext) for security.
///
/// # Security
///
/// - 6-digit codes are hashed with Argon2 because they are guessable
/// - Only one code per user (UNIQUE constraint on `user_id`)
/// - Requesting a new code replaces the old one (upsert semantics)
/// - Codes expire after 15 minutes (shorter than verification for security)
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait PasswordResetRepository: Send + Sync {
    /// Persists or updates a password reset code.
    ///
    /// If a code already exists for the user, it is replaced (upsert).
    /// The code should already have its value hashed before calling this method.
    ///
    /// INVARIANT (P-11): the existing code is replaced only if the reissue rule
    /// allows it at the new code's `last_sent_at`: below the attempt cap or
    /// past its lock window, and past the resend cooldown. That check is made
    /// in the same statement as the write, because a wrong guess can lock the
    /// code, or a concurrent request issue one, after the caller's own
    /// `reissue` decision. Otherwise nothing is written and this returns
    /// [`IssueOutcome::Withheld`].
    async fn save(&self, code: &PasswordResetCode) -> Result<IssueOutcome, DomainError>;

    /// Finds a password reset code by user ID.
    async fn find_by_user(
        &self,
        user_id: &UserId,
    ) -> Result<Option<PasswordResetCode>, DomainError>;

    /// Atomically consumes a specific reset code for a user.
    ///
    /// Deletes the row matching both `user_id` and `code_id` and reports
    /// whether a row was actually removed. This makes a reset token
    /// single-use: the first successful reset removes the row, so a replayed
    /// token finds nothing to consume.
    ///
    /// Returns `true` if a matching row was deleted, `false` otherwise.
    async fn consume(&self, user_id: &UserId, code_id: &ResetCodeId) -> Result<bool, DomainError>;

    /// Atomically records an attempt against the failure cap of the code
    /// `code_id` the caller read and will compare against.
    ///
    /// Increments `attempts` only while it is strictly below `max_attempts`,
    /// returning [`AttemptOutcome::Claimed`] with the resulting count. Returns
    /// [`AttemptOutcome::Locked`] when that code exists but is already at the
    /// cap, and [`AttemptOutcome::NotFound`] when it doesn't, because it was
    /// deleted, expired or replaced by a newer code since the caller read it
    /// (so callers surface an expired/no-code message, not a misleading
    /// lockout).
    ///
    /// INVARIANT: the claim names the code id. A claim by user alone let a
    /// guess that read a since-replaced code take a slot on the new code and
    /// be compared against the old one, past the old code's cap (found by the
    /// Quint model, OT-010 slice 4: `models/attempt_claim.qnt`, `claim_by_user`).
    ///
    /// INVARIANT: claim the slot with this single statement *before* the
    /// expensive code comparison. The previous load-check-then-increment pattern
    /// let concurrent guesses all read `attempts = 0` and slip past the cap,
    /// brute-forcing the 6-digit code.
    async fn register_attempt(
        &self,
        user_id: &UserId,
        code_id: &ResetCodeId,
        max_attempts: i32,
    ) -> Result<AttemptOutcome, DomainError>;

    /// Releases a previously-claimed attempt slot on code `code_id` after a
    /// *successful* verification. Does nothing if that code has been replaced.
    ///
    /// Decrements `attempts` (never below zero) so a correct code does not
    /// permanently consume one of the capped attempts and a transient failure on
    /// the success path can be retried without tripping the lockout. Pairs with
    /// `register_attempt`, which claims the slot before the expensive compare.
    async fn release_attempt(
        &self,
        user_id: &UserId,
        code_id: &ResetCodeId,
    ) -> Result<(), DomainError>;

    /// Deletes all expired codes from the database.
    ///
    /// This is a cleanup operation that should be run periodically.
    /// Returns the number of codes deleted.
    async fn delete_expired(&self) -> Result<u64, DomainError>;
}
