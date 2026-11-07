//! Email verification code entity.

use crate::domain::entities::attempt_lockout::AttemptLockout;
use crate::domain::value_objects::{UserId, VerificationCodeId};
use chrono::{DateTime, Duration, Utc};

// The brute-force lockout policy (attempt cap, lock window, resend cooldown) is
// shared with the password-reset flow; re-export it here so existing call sites
// keep their import path while the arithmetic lives in one place.
pub use crate::domain::entities::attempt_lockout::{
    LOCK_DURATION_MINUTES, MAX_ATTEMPTS, RESEND_COOLDOWN_MINUTES,
};

/// Duration in hours before the verification code expires.
pub const EXPIRY_HOURS: i64 = 24;

/// An email verification code for user email verification.
///
/// After registration, users receive a 6-digit code via email that they
/// must enter to verify their email address.
///
/// # Security
///
/// - Codes are hashed with Argon2 (not SHA-256) because 6-digit codes are guessable
/// - Maximum 5 attempts before the code is locked
/// - New code can only be sent every 5 minutes (rate limiting)
/// - Code expires after 24 hours
#[derive(Debug, Clone)]
pub struct EmailVerificationCode {
    id: VerificationCodeId,
    user_id: UserId,
    code_hash: String,
    expires_at: DateTime<Utc>,
    attempts: i32,
    last_sent_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    /// Optional magic link token hash (SHA-256, since token is random UUID)
    magic_token_hash: Option<String>,
    /// Expiry of the magic link token, tracked independently of `expires_at` so
    /// resending the 6-digit code (which refreshes `expires_at`) does not extend
    /// or revive a previously-issued magic link.
    magic_token_expires_at: Option<DateTime<Utc>>,
}

impl EmailVerificationCode {
    /// Creates a new email verification code.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The ID of the user this code belongs to
    /// * `code_hash` - Argon2 hash of the 6-digit code
    #[must_use]
    pub fn new(user_id: UserId, code_hash: String) -> Self {
        let now = Utc::now();
        Self {
            id: VerificationCodeId::new(),
            user_id,
            code_hash,
            expires_at: now + Duration::hours(EXPIRY_HOURS),
            attempts: 0,
            last_sent_at: now,
            created_at: now,
            magic_token_hash: None,
            magic_token_expires_at: None,
        }
    }

    /// Creates a new email verification code with a magic link token.
    #[must_use]
    pub fn new_with_magic_token(
        user_id: UserId,
        code_hash: String,
        magic_token_hash: String,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: VerificationCodeId::new(),
            user_id,
            code_hash,
            expires_at: now + Duration::hours(EXPIRY_HOURS),
            attempts: 0,
            last_sent_at: now,
            created_at: now,
            magic_token_hash: Some(magic_token_hash),
            magic_token_expires_at: Some(now + Duration::hours(EXPIRY_HOURS)),
        }
    }

    /// Reconstructs a verification code from persistence layer data.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub const fn from_persistence(
        id: VerificationCodeId,
        user_id: UserId,
        code_hash: String,
        expires_at: DateTime<Utc>,
        attempts: i32,
        last_sent_at: DateTime<Utc>,
        created_at: DateTime<Utc>,
        magic_token_hash: Option<String>,
        magic_token_expires_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            id,
            user_id,
            code_hash,
            expires_at,
            attempts,
            last_sent_at,
            created_at,
            magic_token_hash,
            magic_token_expires_at,
        }
    }

    /// Returns the code's unique identifier.
    #[must_use]
    pub const fn id(&self) -> &VerificationCodeId {
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

    /// Returns when a code was last sent.
    #[must_use]
    pub const fn last_sent_at(&self) -> DateTime<Utc> {
        self.last_sent_at
    }

    /// Returns when this code was created.
    #[must_use]
    pub const fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Returns the magic link token hash, if set.
    #[must_use]
    pub fn magic_token_hash(&self) -> Option<&str> {
        self.magic_token_hash.as_deref()
    }

    /// Returns when the magic link token expires, if set.
    #[must_use]
    pub const fn magic_token_expires_at(&self) -> Option<DateTime<Utc>> {
        self.magic_token_expires_at
    }

    /// Checks whether the magic link token is expired.
    ///
    /// Uses the token's own expiry (independent of the 6-digit code's
    /// `expires_at`) so a code resend never extends the link. A token with no
    /// recorded expiry is treated as expired/invalid.
    #[must_use]
    pub fn is_magic_token_expired(&self) -> bool {
        self.magic_token_expires_at
            .is_none_or(|expires_at| expires_at <= Utc::now())
    }

    /// Checks if this code has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }

    /// Brute-force lockout/cooldown view over this code's `attempts` and
    /// `last_sent_at`. The policy itself lives in [`AttemptLockout`] so it stays
    /// in sync with the password-reset flow.
    #[must_use]
    const fn lockout(&self) -> AttemptLockout {
        AttemptLockout::new(self.attempts, self.last_sent_at)
    }

    /// Checks if this code is locked (too many failed attempts).
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        self.lockout().is_locked()
    }

    /// Seconds until a locked code may be replaced via resend, measured from
    /// `last_sent_at`. Returns 0 once the lock window has elapsed (or if the
    /// code is not locked), at which point a resend issues a fresh code.
    #[must_use]
    pub fn seconds_until_unlock(&self) -> i64 {
        self.lockout().seconds_until_unlock()
    }

    /// Checks if a new code can be sent (cooldown expired).
    #[must_use]
    pub fn can_resend(&self) -> bool {
        self.lockout().can_resend()
    }

    /// Returns how many seconds until resend is allowed.
    /// Returns 0 if resend is already allowed.
    #[must_use]
    pub fn seconds_until_resend(&self) -> i64 {
        self.lockout().seconds_until_resend()
    }

    /// Increments the attempt counter after a failed verification.
    pub const fn increment_attempts(&mut self) {
        self.attempts += 1;
    }

    /// Resets the code for a resend operation.
    ///
    /// Updates the code hash, resets attempts, and updates `last_sent_at`.
    pub fn reset_for_resend(&mut self, new_code_hash: String) {
        let now = Utc::now();
        self.code_hash = new_code_hash;
        self.attempts = 0;
        self.last_sent_at = now;
        self.expires_at = now + Duration::hours(EXPIRY_HOURS);
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

    fn create_valid_code() -> EmailVerificationCode {
        EmailVerificationCode::new(UserId::new(), "code_hash".to_string())
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
    fn test_cannot_resend_immediately() {
        let code = create_valid_code();
        assert!(!code.can_resend());
        assert!(code.seconds_until_resend() > 0);
    }

    #[test]
    fn test_reset_for_resend() {
        let mut code = create_valid_code();
        code.increment_attempts();
        code.increment_attempts();
        assert_eq!(code.attempts(), 2);

        let original_expires = code.expires_at();
        std::thread::sleep(std::time::Duration::from_millis(10));

        code.reset_for_resend("new_hash".to_string());

        assert_eq!(code.code_hash(), "new_hash");
        assert_eq!(code.attempts(), 0);
        assert!(code.expires_at() > original_expires);
    }

    #[test]
    fn test_expired_code() {
        let code = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            UserId::new(),
            "hash".to_string(),
            Utc::now() - Duration::hours(1), // Expired 1 hour ago
            0,
            Utc::now() - Duration::hours(25),
            Utc::now() - Duration::hours(25),
            None,
            None,
        );

        assert!(code.is_expired());
        assert!(!code.is_valid());
    }

    #[test]
    fn test_from_persistence() {
        let id = VerificationCodeId::new();
        let user_id = UserId::new();
        let code_hash = "hash".to_string();
        let expires_at = Utc::now() + Duration::hours(24);
        let last_sent_at = Utc::now();
        let created_at = Utc::now();

        let code = EmailVerificationCode::from_persistence(
            id,
            user_id,
            code_hash.clone(),
            expires_at,
            2,
            last_sent_at,
            created_at,
            None,
            None,
        );

        assert_eq!(code.id(), &id);
        assert_eq!(code.user_id(), &user_id);
        assert_eq!(code.code_hash(), &code_hash);
        assert_eq!(code.attempts(), 2);
        assert!(code.is_valid());
    }

    #[test]
    fn test_with_magic_token() {
        let code = EmailVerificationCode::new_with_magic_token(
            UserId::new(),
            "code_hash".to_string(),
            "magic_hash".to_string(),
        );
        assert_eq!(code.magic_token_hash(), Some("magic_hash"));
    }

    #[test]
    fn test_seconds_until_unlock() {
        // Not locked → 0 regardless of last_sent_at.
        let unlocked = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            UserId::new(),
            "hash".to_string(),
            Utc::now() + Duration::hours(24),
            0,
            Utc::now(),
            Utc::now(),
            None,
            None,
        );
        assert_eq!(unlocked.seconds_until_unlock(), 0);

        // Locked, sent just now → within the lock window.
        let just_locked = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            UserId::new(),
            "hash".to_string(),
            Utc::now() + Duration::hours(24),
            MAX_ATTEMPTS,
            Utc::now(),
            Utc::now(),
            None,
            None,
        );
        let secs = just_locked.seconds_until_unlock();
        assert!(secs > 0 && secs <= LOCK_DURATION_MINUTES * 60);

        // Locked, but the lock window has elapsed → 0 (resend allowed).
        let lock_elapsed = EmailVerificationCode::from_persistence(
            VerificationCodeId::new(),
            UserId::new(),
            "hash".to_string(),
            Utc::now() + Duration::hours(24),
            MAX_ATTEMPTS,
            Utc::now() - Duration::minutes(LOCK_DURATION_MINUTES + 1),
            Utc::now() - Duration::minutes(LOCK_DURATION_MINUTES + 1),
            None,
            None,
        );
        assert_eq!(lock_elapsed.seconds_until_unlock(), 0);
    }

    #[test]
    fn test_is_magic_token_expired() {
        let make = |magic_expiry| {
            EmailVerificationCode::from_persistence(
                VerificationCodeId::new(),
                UserId::new(),
                "hash".to_string(),
                Utc::now() + Duration::hours(24),
                0,
                Utc::now(),
                Utc::now(),
                Some("magic_hash".to_string()),
                magic_expiry,
            )
        };

        // Future expiry → not expired.
        assert!(!make(Some(Utc::now() + Duration::hours(1))).is_magic_token_expired());
        // Past expiry → expired.
        assert!(make(Some(Utc::now() - Duration::hours(1))).is_magic_token_expired());
        // No recorded expiry → treated as expired/invalid.
        assert!(make(None).is_magic_token_expired());
    }
}
