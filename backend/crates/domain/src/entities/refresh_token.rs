//! Refresh token entity for JWT token rotation.

use crate::entities::attempt_lockout::expired_at;
use crate::value_objects::{RefreshTokenId, UserId};
use jiff::Timestamp;

/// Why a [`RefreshToken`] was revoked.
///
/// Distinguishes theft signals (replaying a rotated token, a reuse-revoked
/// token, or a legacy row whose reason was not recorded) from benign
/// revocations (`Logout`, `PasswordChange`) so that an in-flight refresh racing
/// sign-out or a stale device after a password reset does not trigger
/// full-account reuse revocation and a false-positive security alert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevocationReason {
    /// Replaced by a successor during normal token rotation.
    Rotated,
    /// Explicitly signed out by the user.
    Logout,
    /// Revoked as part of reuse-detection cleanup after another token was replayed.
    ReuseDetected,
    /// Revoked when the user's password was reset or changed.
    PasswordChange,
    /// Revoked by an older release that did not record a reason (fail closed).
    Unknown,
}

impl RevocationReason {
    /// Database representation for persisted reasons (`None` for [`Self::Unknown`]).
    #[must_use]
    pub const fn as_db_str(self) -> Option<&'static str> {
        match self {
            Self::Rotated => Some("rotated"),
            Self::Logout => Some("logout"),
            Self::ReuseDetected => Some("reuse_detected"),
            Self::PasswordChange => Some("password_change"),
            Self::Unknown => None,
        }
    }

    /// Parses a persisted reason, mapping `None` or unrecognised values to
    /// [`Self::Unknown`] (fail closed).
    #[must_use]
    pub fn from_db_str(value: Option<&str>) -> Self {
        match value {
            Some("rotated") => Self::Rotated,
            Some("logout") => Self::Logout,
            Some("reuse_detected") => Self::ReuseDetected,
            Some("password_change") => Self::PasswordChange,
            _ => Self::Unknown,
        }
    }

    /// Whether presenting a token revoked for this reason signals possible
    /// token theft and should invalidate every active session for the user.
    #[must_use]
    pub const fn triggers_reuse_detection(self) -> bool {
        matches!(self, Self::Rotated | Self::ReuseDetected | Self::Unknown)
    }
}

/// Lifecycle of a [`RefreshToken`]; see [`RefreshToken::status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshTokenStatus {
    /// Usable for one rotation.
    Active,
    /// Revoked for the carried [`RevocationReason`]. Presenting it again is
    /// always refused; whether it also triggers full-account reuse revocation
    /// depends on [`RevocationReason::triggers_reuse_detection`].
    Revoked(RevocationReason),
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
    revoked_reason: Option<RevocationReason>,
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
            revoked_reason: None,
            created_at: Timestamp::now(),
        }
    }

    /// Reconstructs a refresh token from persistence layer data.
    ///
    /// This is used by repositories when loading a token from the database.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "rehydrates one persisted row; each argument is a column"
    )]
    pub const fn from_persistence(
        id: RefreshTokenId,
        user_id: UserId,
        token_hash: String,
        device_fingerprint: String,
        expires_at: Timestamp,
        revoked_at: Option<Timestamp>,
        revoked_reason: Option<RevocationReason>,
        created_at: Timestamp,
    ) -> Self {
        Self {
            id,
            user_id,
            token_hash,
            device_fingerprint,
            expires_at,
            revoked_at,
            revoked_reason,
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

    /// Returns why this token was revoked, if recorded.
    #[must_use]
    pub const fn revoked_reason(&self) -> Option<RevocationReason> {
        self.revoked_reason
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
            self.revoked_reason,
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

    /// Revokes this token as of `now` for `reason`.
    ///
    /// If the token is already revoked, this is a no-op: the first revocation
    /// time and reason are kept.
    pub const fn revoke(&mut self, now: Timestamp, reason: RevocationReason) {
        if self.revoked_at.is_none() {
            self.revoked_at = Some(now);
            self.revoked_reason = Some(reason);
        }
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
/// revoked (threat model P-15). A missing reason on a revoked row fails closed
/// to [`RevocationReason::Unknown`].
const fn status_at(
    revoked: bool,
    reason: Option<RevocationReason>,
    expires_us: i64,
    now_us: i64,
) -> RefreshTokenStatus {
    if revoked {
        let reason = match reason {
            Some(r) => r,
            None => RevocationReason::Unknown,
        };
        RefreshTokenStatus::Revoked(reason)
    } else if expired_at(expires_us, now_us) {
        RefreshTokenStatus::Expired
    } else {
        RefreshTokenStatus::Active
    }
}

/// The revocation time after revoking at `now`: an earlier revocation stands.
#[cfg(kani)]
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
    use crate::entities::attempt_lockout::verification::any_time;

    fn any_reason() -> Option<RevocationReason> {
        match kani::any::<u8>() {
            0 => None,
            1 => Some(RevocationReason::Rotated),
            2 => Some(RevocationReason::Logout),
            3 => Some(RevocationReason::ReuseDetected),
            4 => Some(RevocationReason::PasswordChange),
            _ => Some(RevocationReason::Unknown),
        }
    }

    /// For every expiry, current time, and reason: a revoked token reports
    /// `Revoked` (defaulting a missing reason to `Unknown`), before or after
    /// its expiry, and an unrevoked one is `Active` exactly before its expiry
    /// and `Expired` from it on.
    #[kani::proof]
    fn a_revoked_token_is_never_accepted() {
        let (revoked, reason, expires, now) = (kani::any(), any_reason(), any_time(), any_time());
        let status = status_at(revoked, reason, expires, now);
        if revoked {
            let expected = reason.unwrap_or(RevocationReason::Unknown);
            assert_eq!(status, RefreshTokenStatus::Revoked(expected));
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

        token.revoke(Timestamp::now(), RevocationReason::Rotated);

        assert_eq!(
            token.status(Timestamp::now()),
            RefreshTokenStatus::Revoked(RevocationReason::Rotated)
        );
    }

    #[test]
    fn test_revocation_reason_reuse_detection() {
        assert!(RevocationReason::Rotated.triggers_reuse_detection());
        assert!(RevocationReason::ReuseDetected.triggers_reuse_detection());
        assert!(RevocationReason::Unknown.triggers_reuse_detection());
        assert!(!RevocationReason::Logout.triggers_reuse_detection());
        assert!(!RevocationReason::PasswordChange.triggers_reuse_detection());
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

        token.revoke(Timestamp::now(), RevocationReason::Logout);

        assert!(!token.is_valid(Timestamp::now()));
        assert!(token.is_revoked());
        assert!(token.revoked_at().is_some());
        assert_eq!(token.revoked_reason(), Some(RevocationReason::Logout));
    }

    #[test]
    fn test_revoke_is_idempotent() {
        let mut token = create_valid_token();
        token.revoke(Timestamp::now(), RevocationReason::Logout);
        let first_revoked_at = token.revoked_at();

        std::thread::sleep(std::time::Duration::from_millis(10));
        token.revoke(Timestamp::now(), RevocationReason::Rotated);

        assert_eq!(token.revoked_at(), first_revoked_at);
        assert_eq!(token.revoked_reason(), Some(RevocationReason::Logout));
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
