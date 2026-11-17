//! Password reset code entity.

use crate::domain::entities::attempt_lockout::{self, AttemptLockout, Reissue};
use crate::domain::value_objects::{ResetCodeId, UserId};
use jiff::{SignedDuration, Timestamp};

// The brute-force lockout policy (attempt cap, lock window, resend cooldown) is
// shared with the email-verification flow; re-export it here so existing call
// sites keep their import path while the arithmetic lives in one place.
pub use crate::domain::entities::attempt_lockout::{
    LOCK_DURATION_MINUTES, MAX_ATTEMPTS, RESEND_COOLDOWN_MINUTES,
};

/// Duration in minutes before the reset code expires.
pub const EXPIRY_MINUTES: i64 = 15;

/// A password reset code for user password recovery.
///
/// When a user forgets their password, they receive a 6-digit code via email
/// that they must enter to reset their password.
///
/// # Security
///
/// - Codes are hashed with Argon2 (not SHA-256) because 6-digit codes are guessable
/// - Maximum 5 attempts before the code is locked
/// - Code expires after 15 minutes (shorter than verification for security)
#[derive(Debug, Clone)]
pub struct PasswordResetCode {
    id: ResetCodeId,
    user_id: UserId,
    code_hash: String,
    expires_at: Timestamp,
    attempts: i32,
    last_sent_at: Timestamp,
    created_at: Timestamp,
    /// Codes for this user replaced while locked at the attempt cap; each
    /// doubles the next lock window. Persisted, never set by the domain.
    lockouts: i32,
}

impl PasswordResetCode {
    /// Creates a new password reset code.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The ID of the user this code belongs to
    /// * `code_hash` - Argon2 hash of the 6-digit code
    #[must_use]
    pub fn new(user_id: UserId, code_hash: String) -> Self {
        let now = Timestamp::now();
        Self {
            id: ResetCodeId::new(),
            user_id,
            code_hash,
            expires_at: now + SignedDuration::from_mins(EXPIRY_MINUTES),
            attempts: 0,
            last_sent_at: now,
            created_at: now,
            lockouts: 0,
        }
    }

    /// Reconstructs a reset code from persistence layer data.
    #[must_use]
    pub const fn from_persistence(
        id: ResetCodeId,
        user_id: UserId,
        code_hash: String,
        expires_at: Timestamp,
        attempts: i32,
        last_sent_at: Timestamp,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            user_id,
            code_hash,
            expires_at,
            attempts,
            last_sent_at,
            created_at,
            lockouts: 0,
        }
    }

    /// Sets the persisted lockout count. Chained onto [`Self::from_persistence`]
    /// by the repository; kept separate so the many test call sites of
    /// `from_persistence` don't each have to spell out a zero.
    #[must_use]
    pub const fn with_lockouts(mut self, lockouts: i32) -> Self {
        self.lockouts = lockouts;
        self
    }

    /// Returns how many earlier codes were replaced while locked.
    #[must_use]
    pub const fn lockouts(&self) -> i32 {
        self.lockouts
    }

    /// Returns the code's unique identifier.
    #[must_use]
    pub const fn id(&self) -> &ResetCodeId {
        &self.id
    }

    /// Returns the user ID this code belongs to.
    #[must_use]
    pub const fn user_id(&self) -> &UserId {
        &self.user_id
    }

    /// Returns the Argon2 hash of the code.
    #[must_use]
    pub fn code_hash(&self) -> &str {
        &self.code_hash
    }

    /// Returns when this code expires.
    #[must_use]
    pub const fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// Returns the number of verification attempts.
    #[must_use]
    pub const fn attempts(&self) -> i32 {
        self.attempts
    }

    /// Returns when this code was created.
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// Returns when a code was last sent.
    #[must_use]
    pub const fn last_sent_at(&self) -> Timestamp {
        self.last_sent_at
    }

    /// Checks if this code has expired.
    #[must_use]
    pub fn is_expired(&self, now: Timestamp) -> bool {
        attempt_lockout::expired(self.expires_at, now)
    }

    /// Brute-force lockout/cooldown view over this code's `attempts` and
    /// `last_sent_at`. The policy itself lives in [`AttemptLockout`] so it stays
    /// in sync with the email-verification flow.
    #[must_use]
    const fn lockout(&self) -> AttemptLockout {
        AttemptLockout::new(self.attempts, self.last_sent_at, self.lockouts)
    }

    /// Checks if this code is locked (too many failed attempts).
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.lockout().is_locked()
    }

    /// Whether a replacement code may be issued as of `now` (see
    /// [`AttemptLockout::reissue`]). Every path that issues a code for this
    /// user must check it first, or it would clear an active lockout.
    #[must_use]
    pub fn reissue(&self, now: Timestamp) -> Reissue {
        self.lockout().reissue(now)
    }

    /// Resets the code for a new request.
    ///
    /// Updates the code hash, resets attempts, and updates expiry.
    pub fn reset(&mut self, new_code_hash: String) {
        let now = Timestamp::now();
        self.code_hash = new_code_hash;
        self.attempts = 0;
        self.last_sent_at = now;
        self.expires_at = now + SignedDuration::from_mins(EXPIRY_MINUTES);
    }

    /// Checks if this code is valid (not expired and not locked).
    #[must_use]
    pub fn is_valid(&self, now: Timestamp) -> bool {
        !self.is_expired(now) && !self.is_locked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_code() -> PasswordResetCode {
        PasswordResetCode::new(UserId::new(), "code_hash".to_string())
    }

    /// A live code that already has `attempts` failed guesses.
    fn with_attempts(attempts: i32) -> PasswordResetCode {
        PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            UserId::new(),
            "code_hash".to_string(),
            Timestamp::now() + SignedDuration::from_mins(EXPIRY_MINUTES),
            attempts,
            Timestamp::now(),
            Timestamp::now(),
        )
    }

    #[test]
    fn test_new_code_is_valid() {
        let code = create_valid_code();
        assert!(code.is_valid(Timestamp::now()));
        assert!(!code.is_expired(Timestamp::now()));
        assert!(!code.is_locked());
    }

    #[test]
    fn test_reset() {
        let mut code = with_attempts(2);
        assert_eq!(code.attempts(), 2);

        let original_expires = code.expires_at();
        std::thread::sleep(std::time::Duration::from_millis(10));

        code.reset("new_hash".to_string());

        assert_eq!(code.code_hash(), "new_hash");
        assert_eq!(code.attempts(), 0);
        assert!(code.expires_at() > original_expires);
    }

    #[test]
    fn test_expired_code() {
        let code = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            UserId::new(),
            "hash".to_string(),
            Timestamp::now() - SignedDuration::from_mins(1), // Expired 1 minute ago
            0,
            Timestamp::now() - SignedDuration::from_mins(16),
            Timestamp::now() - SignedDuration::from_mins(16),
        );

        assert!(code.is_expired(Timestamp::now()));
        assert!(!code.is_valid(Timestamp::now()));
    }

    #[test]
    fn test_from_persistence() {
        let id = ResetCodeId::new();
        let user_id = UserId::new();
        let code_hash = "hash".to_string();
        let expires_at = Timestamp::now() + SignedDuration::from_mins(15);
        let created_at = Timestamp::now();

        let code = PasswordResetCode::from_persistence(
            id,
            user_id,
            code_hash.clone(),
            expires_at,
            2,
            created_at,
            created_at,
        );

        assert_eq!(code.id(), &id);
        assert_eq!(code.user_id(), &user_id);
        assert_eq!(code.code_hash(), &code_hash);
        assert_eq!(code.attempts(), 2);
        assert!(code.is_valid(Timestamp::now()));
    }

    #[test]
    fn test_expiry_is_15_minutes() {
        let code = create_valid_code();
        let expected_expiry = code.created_at() + SignedDuration::from_mins(EXPIRY_MINUTES);
        // Allow 1 second tolerance for test execution time
        let diff = (code.expires_at() - expected_expiry).as_secs().abs();
        assert!(diff <= 1);
    }

    #[test]
    fn test_reissue_while_locked() {
        let make = |attempts, sent_minutes_ago| {
            PasswordResetCode::from_persistence(
                ResetCodeId::new(),
                UserId::new(),
                "hash".to_string(),
                Timestamp::now() + SignedDuration::from_mins(EXPIRY_MINUTES),
                attempts,
                Timestamp::now() - SignedDuration::from_mins(sent_minutes_ago),
                Timestamp::now() - SignedDuration::from_mins(sent_minutes_ago),
            )
        };

        // Locked, sent just now → within the lock window.
        assert!(matches!(
            make(MAX_ATTEMPTS, 0).reissue(Timestamp::now()),
            Reissue::Locked { seconds_remaining }
                if seconds_remaining > 0 && seconds_remaining <= LOCK_DURATION_MINUTES * 60
        ));

        // Locked, but the lock window has elapsed → a new request is allowed.
        assert_eq!(
            make(MAX_ATTEMPTS, LOCK_DURATION_MINUTES + 1).reissue(Timestamp::now()),
            Reissue::Allowed
        );
    }

    #[test]
    fn test_reissue_cooldown() {
        let recent = PasswordResetCode::new(UserId::new(), "hash".to_string());
        assert!(matches!(
            recent.reissue(Timestamp::now()),
            Reissue::Cooldown { seconds_remaining } if seconds_remaining > 0
        ));

        let old = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            UserId::new(),
            "hash".to_string(),
            Timestamp::now() + SignedDuration::from_mins(EXPIRY_MINUTES),
            0,
            Timestamp::now() - SignedDuration::from_mins(RESEND_COOLDOWN_MINUTES + 1),
            Timestamp::now() - SignedDuration::from_mins(RESEND_COOLDOWN_MINUTES + 1),
        );
        assert_eq!(old.reissue(Timestamp::now()), Reissue::Allowed);
    }
}
