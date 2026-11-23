//! Refresh token entity for JWT token rotation.

use crate::domain::entities::attempt_lockout::expired_at;
use crate::domain::value_objects::{RefreshTokenId, UserId};
use jiff::Timestamp;

/// Lifecycle of a [`RefreshToken`]; see [`RefreshToken::status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenStatus {
    /// Usable for one rotation.
    Active,
    /// Revoked by rotation, logout or reuse detection. Presenting it again
    /// means it was probably stolen.
    Revoked,
    /// Past its expiry without having been revoked.
    Expired,
}

/// A refresh token for JWT token rotation.
///
/// Refresh tokens are long-lived tokens stored in the database that can be used
/// to obtain new short-lived JWTs. They support:
/// - Token rotation (old token is revoked when a new one is issued)
/// - Device binding (fingerprint must match)
/// - Explicit revocation (e.g., on logout)
///
/// # Security
///
/// The actual token value is never stored. Instead, a SHA-256 hash is stored
/// to prevent token theft if the database is compromised.
#[derive(Debug, Clone)]
pub struct RefreshToken {
    id: RefreshTokenId,
    user_id: UserId,
    token_hash: String,
    device_fingerprint: String,
    expires_at: Timestamp,
    revoked_at: Option<Timestamp>,
    created_at: Timestamp,
}

impl RefreshToken {
    /// Creates a new refresh token.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The ID of the user this token belongs to
    /// * `token_hash` - SHA-256 hash of the actual token value
    /// * `device_fingerprint` - SHA-256 hash of device characteristics
    /// * `expires_at` - When this token expires
    #[must_use]
    pub fn new(
        user_id: UserId,
        token_hash: String,
        device_fingerprint: String,
        expires_at: Timestamp,
    ) -> Self {
        Self {
            id: RefreshTokenId::new(),
            user_id,
            token_hash,
            device_fingerprint,
            expires_at,
            revoked_at: None,
            created_at: Timestamp::now(),
        }
    }

    /// Reconstructs a refresh token from persistence layer data.
    ///
    /// This is used by repositories when loading a token from the database.
    #[must_use]
    pub const fn from_persistence(
        id: RefreshTokenId,
        user_id: UserId,
        token_hash: String,
        device_fingerprint: String,
        expires_at: Timestamp,
        revoked_at: Option<Timestamp>,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            user_id,
            token_hash,
            device_fingerprint,
            expires_at,
            revoked_at,
            created_at,
        }
    }

    /// Returns the token's unique identifier.
    #[must_use]
    pub const fn id(&self) -> &RefreshTokenId {
        &self.id
    }

    /// Returns the user ID this token belongs to.
    #[must_use]
    pub const fn user_id(&self) -> &UserId {
        &self.user_id
    }

    /// Returns the SHA-256 hash of the token value.
    #[must_use]
    pub fn token_hash(&self) -> &str {
        &self.token_hash
    }

    /// Returns the device fingerprint hash.
    #[must_use]
    pub fn device_fingerprint(&self) -> &str {
        &self.device_fingerprint
    }

    /// Returns when this token expires.
    #[must_use]
    pub const fn expires_at(&self) -> Timestamp {
        self.expires_at
    }

    /// Returns when this token was revoked, if at all.
    #[must_use]
    pub const fn revoked_at(&self) -> Option<Timestamp> {
        self.revoked_at
    }

    /// Returns when this token was created.
    #[must_use]
    pub const fn created_at(&self) -> Timestamp {
        self.created_at
    }

    /// Where this token is in its lifecycle.
    ///
    /// A token that is both revoked and expired reports [`Revoked`]: replaying a
    /// revoked token is the theft signal, and it must not be masked by the
    /// token also having aged out.
    ///
    /// [`Revoked`]: RefreshTokenStatus::Revoked
    #[must_use]
    pub fn status(&self, now: Timestamp) -> RefreshTokenStatus {
        status_at(
            self.revoked_at.is_some(),
            self.expires_at.as_microsecond(),
            now.as_microsecond(),
        )
    }

    /// Checks if this token is valid (not expired and not revoked) as of `now`.
    #[must_use]
    pub fn is_valid(&self, now: Timestamp) -> bool {
        self.status(now) == RefreshTokenStatus::Active
    }

    /// Checks if this token has expired as of `now`.
    #[must_use]
    pub fn is_expired(&self, now: Timestamp) -> bool {
        expired_at(self.expires_at.as_microsecond(), now.as_microsecond())
    }

    /// Checks if this token has been revoked.
    #[must_use]
    pub const fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Revokes this token as of `now`.
    ///
    /// If the token is already revoked, this is a no-op: the first revocation
    /// time is kept.
    pub const fn revoke(&mut self, now: Timestamp) {
        self.revoked_at = Some(first_revocation(self.revoked_at, now));
    }

    /// Validates the device fingerprint matches.
    ///
    /// Returns `true` if the provided fingerprint matches the stored one.
    #[must_use]
    pub fn validate_fingerprint(&self, fingerprint: &str) -> bool {
        self.device_fingerprint == fingerprint
    }
}

// The status rule over microseconds since the Unix epoch, so the Kani proofs
// below cover it for every time (design D5, as in `attempt_lockout`).

/// Revoked wins over expired, so a replayed revoked token is always seen as
/// possible theft (threat model P-15).
const fn status_at(revoked: bool, expires_us: i64, now_us: i64) -> RefreshTokenStatus {
    if revoked {
        RefreshTokenStatus::Revoked
    } else if expired_at(expires_us, now_us) {
        RefreshTokenStatus::Expired
    } else {
        RefreshTokenStatus::Active
    }
}

/// The revocation time after revoking at `now`: an earlier revocation stands.
const fn first_revocation<T: Copy>(revoked_at: Option<T>, now: T) -> T {
    match revoked_at {
        Some(at) => at,
        None => now,
    }
}

/// Kani proofs (OT-010, threat model P-15). Run with `just verify`.
#[cfg(kani)]
mod verification {
    use super::*;
    use crate::domain::entities::attempt_lockout::verification::any_time;

    /// For every expiry and current time: a revoked token reports `Revoked`,
    /// before or after its expiry, and an unrevoked one is `Active` exactly
    /// before its expiry and `Expired` from it on.
    #[kani::proof]
    fn a_revoked_token_is_never_accepted() {
        let (revoked, expires, now): (bool, i64, i64) = (kani::any(), any_time(), any_time());
        let status = status_at(revoked, expires, now);
        if revoked {
            assert_eq!(status, RefreshTokenStatus::Revoked);
        } else if now < expires {
            assert_eq!(status, RefreshTokenStatus::Active);
        } else {
            assert_eq!(status, RefreshTokenStatus::Expired);
        }
    }

    /// For every earlier revocation (or none) and every revoking time:
    /// revoking keeps an existing revocation time and otherwise records now.
    #[kani::proof]
    fn revoking_twice_keeps_the_first_time() {
        let earlier: Option<i64> = if kani::any() { Some(any_time()) } else { None };
        let now = any_time();
        let after = first_revocation(earlier, now);
        assert_eq!(after, earlier.unwrap_or(now));
        assert_eq!(first_revocation(Some(after), any_time()), after);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::SignedDuration;

    fn create_valid_token() -> RefreshToken {
        RefreshToken::new(
            UserId::new(),
            "token_hash".to_string(),
            "device_fingerprint".to_string(),
            Timestamp::now() + SignedDuration::from_hours(24 * 30),
        )
    }

    fn create_expired_token() -> RefreshToken {
        RefreshToken::new(
            UserId::new(),
            "token_hash".to_string(),
            "device_fingerprint".to_string(),
            Timestamp::now() - SignedDuration::from_hours(1),
        )
    }

    #[test]
    fn test_status_revoked_outranks_expired() {
        let mut token = create_expired_token();
        assert_eq!(token.status(Timestamp::now()), RefreshTokenStatus::Expired);

        token.revoke(Timestamp::now());

        assert_eq!(token.status(Timestamp::now()), RefreshTokenStatus::Revoked);
    }

    #[test]
    fn test_new_token_is_valid() {
        let token = create_valid_token();
        assert!(token.is_valid(Timestamp::now()));
        assert!(!token.is_expired(Timestamp::now()));
        assert!(!token.is_revoked());
    }

    #[test]
    fn test_expired_token_is_not_valid() {
        let token = create_expired_token();
        assert!(!token.is_valid(Timestamp::now()));
        assert!(token.is_expired(Timestamp::now()));
        assert!(!token.is_revoked());
    }

    #[test]
    fn test_revoke_token() {
        let mut token = create_valid_token();
        assert!(token.is_valid(Timestamp::now()));

        token.revoke(Timestamp::now());

        assert!(!token.is_valid(Timestamp::now()));
        assert!(token.is_revoked());
        assert!(token.revoked_at().is_some());
    }

    #[test]
    fn test_revoke_is_idempotent() {
        let mut token = create_valid_token();
        token.revoke(Timestamp::now());
        let first_revoked_at = token.revoked_at();

        std::thread::sleep(std::time::Duration::from_millis(10));
        token.revoke(Timestamp::now());

        assert_eq!(token.revoked_at(), first_revoked_at);
    }

    #[test]
    fn test_validate_fingerprint_match() {
        let token = create_valid_token();
        assert!(token.validate_fingerprint("device_fingerprint"));
    }

    #[test]
    fn test_validate_fingerprint_mismatch() {
        let token = create_valid_token();
        assert!(!token.validate_fingerprint("different_fingerprint"));
    }

    #[test]
    fn test_from_persistence() {
        let id = RefreshTokenId::new();
        let user_id = UserId::new();
        let token_hash = "hash".to_string();
        let fingerprint = "fingerprint".to_string();
        let expires_at = Timestamp::now() + SignedDuration::from_hours(24 * 30);
        let created_at = Timestamp::now();

        let token = RefreshToken::from_persistence(
            id,
            user_id,
            token_hash.clone(),
            fingerprint.clone(),
            expires_at,
            None,
            created_at,
        );

        assert_eq!(token.id(), &id);
        assert_eq!(token.user_id(), &user_id);
        assert_eq!(token.token_hash(), &token_hash);
        assert_eq!(token.device_fingerprint(), &fingerprint);
        assert!(token.is_valid(Timestamp::now()));
    }
}
