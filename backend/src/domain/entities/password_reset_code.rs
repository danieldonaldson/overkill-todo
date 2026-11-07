//! Password reset code entity.

use crate::domain::entities::attempt_lockout::AttemptLockout;
use crate::domain::value_objects::{ResetCodeId, UserId};
use chrono::{DateTime, Duration, Utc};

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
    expires_at: DateTime<Utc>,
    attempts: i32,
    last_sent_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
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
        let now = Utc::now();
        Self {
            id: ResetCodeId::new(),
            user_id,
            code_hash,
            expires_at: now + Duration::minutes(EXPIRY_MINUTES),
            attempts: 0,
            last_sent_at: now,
            created_at: now,
        }
    }

    /// Reconstructs a reset code from persistence layer data.
    #[must_use]
    pub const fn from_persistence(
        id: ResetCodeId,
        user_id: UserId,
        code_hash: String,
        expires_at: DateTime<Utc>,
        attempts: i32,
        last_sent_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id,
            user_id,
            code_hash,
            expires_at,
            attempts,
            last_sent_at,
            created_at,
        }
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
    pub const fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Returns the number of verification attempts.
    #[must_use]
    pub const fn attempts(&self) -> i32 {
        self.attempts
    }

    /// Returns when this code was created.
    #[must_use]
    pub const fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Returns when a code was last sent.
    #[must_use]
    pub const fn last_sent_at(&self) -> DateTime<Utc> {
        self.last_sent_at
    }

    /// Checks if this code has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }

    /// Brute-force lockout/cooldown view over this code's `attempts` and
    /// `last_sent_at`. The policy itself lives in [`AttemptLockout`] so it stays
    /// in sync with the email-verification flow.
    #[must_use]
    const fn lockout(&self) -> AttemptLockout {
        AttemptLockout::new(self.attempts, self.last_sent_at)
    }

    /// Checks if this code is locked (too many failed attempts).
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.lockout().is_locked()
    }

    /// Seconds until a locked code may be replaced via a new request, measured
    /// from `last_sent_at`. Returns 0 once the lock window has elapsed (or if the
    /// code is not locked), at which point a new request issues a fresh code.
    #[must_use]
    pub fn seconds_until_unlock(&self) -> i64 {
        self.lockout().seconds_until_unlock()
    }

    /// Checks if a new code can be sent (resend cooldown expired).
    #[must_use]
    pub fn can_resend(&self) -> bool {
        self.lockout().can_resend()
    }

    /// Returns how many seconds until a new code may be sent.
    /// Returns 0 if a resend is already allowed.
    #[must_use]
    pub fn seconds_until_resend(&self) -> i64 {
        self.lockout().seconds_until_resend()
    }

    /// Increments the attempt counter after a failed verification.
    pub const fn increment_attempts(&mut self) {
        self.attempts += 1;
    }

    /// Resets the code for a new request.
    ///
    /// Updates the code hash, resets attempts, and updates expiry.
    pub fn reset(&mut self, new_code_hash: String) {
        let now = Utc::now();
        self.code_hash = new_code_hash;
        self.attempts = 0;
        self.last_sent_at = now;
        self.expires_at = now + Duration::minutes(EXPIRY_MINUTES);
    }

    /// Checks if this code is valid (not expired and not locked).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        !self.is_expired() && !self.is_locked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_valid_code() -> PasswordResetCode {
        PasswordResetCode::new(UserId::new(), "code_hash".to_string())
    }

    #[test]
    fn test_new_code_is_valid() {
        let code = create_valid_code();
        assert!(code.is_valid());
        assert!(!code.is_expired());
        assert!(!code.is_locked());
    }

    #[test]
    fn test_increment_attempts() {
        let mut code = create_valid_code();
        assert_eq!(code.attempts(), 0);

        code.increment_attempts();
        assert_eq!(code.attempts(), 1);
        assert!(!code.is_locked());

        // Lock after MAX_ATTEMPTS
        for _ in 1..MAX_ATTEMPTS {
            code.increment_attempts();
        }
        assert_eq!(code.attempts(), MAX_ATTEMPTS);
        assert!(code.is_locked());
        assert!(!code.is_valid());
    }

    #[test]
    fn test_reset() {
        let mut code = create_valid_code();
        code.increment_attempts();
        code.increment_attempts();
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
            Utc::now() - Duration::minutes(1), // Expired 1 minute ago
            0,
            Utc::now() - Duration::minutes(16),
            Utc::now() - Duration::minutes(16),
        );

        assert!(code.is_expired());
        assert!(!code.is_valid());
    }

    #[test]
    fn test_from_persistence() {
        let id = ResetCodeId::new();
        let user_id = UserId::new();
        let code_hash = "hash".to_string();
        let expires_at = Utc::now() + Duration::minutes(15);
        let created_at = Utc::now();

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
        assert!(code.is_valid());
    }

    #[test]
    fn test_expiry_is_15_minutes() {
        let code = create_valid_code();
        let expected_expiry = code.created_at() + Duration::minutes(EXPIRY_MINUTES);
        // Allow 1 second tolerance for test execution time
        let diff = (code.expires_at() - expected_expiry).num_seconds().abs();
        assert!(diff <= 1);
    }

    #[test]
    fn test_seconds_until_unlock() {
        let make = |attempts, sent_minutes_ago| {
            PasswordResetCode::from_persistence(
                ResetCodeId::new(),
                UserId::new(),
                "hash".to_string(),
                Utc::now() + Duration::minutes(EXPIRY_MINUTES),
                attempts,
                Utc::now() - Duration::minutes(sent_minutes_ago),
                Utc::now() - Duration::minutes(sent_minutes_ago),
            )
        };

        // Not locked → 0 regardless of last_sent_at.
        assert_eq!(make(0, 0).seconds_until_unlock(), 0);

        // Locked, sent just now → within the lock window.
        let secs = make(MAX_ATTEMPTS, 0).seconds_until_unlock();
        assert!(secs > 0 && secs <= LOCK_DURATION_MINUTES * 60);

        // Locked, but the lock window has elapsed → 0 (a new request is allowed).
        assert_eq!(
            make(MAX_ATTEMPTS, LOCK_DURATION_MINUTES + 1).seconds_until_unlock(),
            0
        );
    }

    #[test]
    fn test_can_resend_cooldown() {
        let recent = PasswordResetCode::new(UserId::new(), "hash".to_string());
        assert!(!recent.can_resend());
        assert!(recent.seconds_until_resend() > 0);

        let old = PasswordResetCode::from_persistence(
            ResetCodeId::new(),
            UserId::new(),
            "hash".to_string(),
            Utc::now() + Duration::minutes(EXPIRY_MINUTES),
            0,
            Utc::now() - Duration::minutes(RESEND_COOLDOWN_MINUTES + 1),
            Utc::now() - Duration::minutes(RESEND_COOLDOWN_MINUTES + 1),
        );
        assert!(old.can_resend());
        assert_eq!(old.seconds_until_resend(), 0);
    }
}
