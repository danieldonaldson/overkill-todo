//! Refresh token entity for JWT token rotation.

use crate::domain::value_objects::{RefreshTokenId, UserId};
use chrono::{DateTime, Utc};

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
    expires_at: DateTime<Utc>,
    revoked_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
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
        expires_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: RefreshTokenId::new(),
            user_id,
            token_hash,
            device_fingerprint,
            expires_at,
            revoked_at: None,
            created_at: Utc::now(),
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
        expires_at: DateTime<Utc>,
        revoked_at: Option<DateTime<Utc>>,
        created_at: DateTime<Utc>,
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
    pub const fn expires_at(&self) -> DateTime<Utc> {
        self.expires_at
    }

    /// Returns when this token was revoked, if at all.
    #[must_use]
    pub const fn revoked_at(&self) -> Option<DateTime<Utc>> {
        self.revoked_at
    }

    /// Returns when this token was created.
    #[must_use]
    pub const fn created_at(&self) -> DateTime<Utc> {
        self.created_at
    }

    /// Checks if this token is valid (not expired and not revoked).
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.revoked_at.is_none() && self.expires_at > Utc::now()
    }

    /// Checks if this token has expired.
    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.expires_at <= Utc::now()
    }

    /// Checks if this token has been revoked.
    #[must_use]
    pub const fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Revokes this token.
    ///
    /// If the token is already revoked, this is a no-op.
    pub fn revoke(&mut self) {
        if self.revoked_at.is_none() {
            self.revoked_at = Some(Utc::now());
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn create_valid_token() -> RefreshToken {
        RefreshToken::new(
            UserId::new(),
            "token_hash".to_string(),
            "device_fingerprint".to_string(),
            Utc::now() + Duration::days(30),
        )
    }

    fn create_expired_token() -> RefreshToken {
        RefreshToken::new(
            UserId::new(),
            "token_hash".to_string(),
            "device_fingerprint".to_string(),
            Utc::now() - Duration::hours(1),
        )
    }

    #[test]
    fn test_new_token_is_valid() {
        let token = create_valid_token();
        assert!(token.is_valid());
        assert!(!token.is_expired());
        assert!(!token.is_revoked());
    }

    #[test]
    fn test_expired_token_is_not_valid() {
        let token = create_expired_token();
        assert!(!token.is_valid());
        assert!(token.is_expired());
        assert!(!token.is_revoked());
    }

    #[test]
    fn test_revoke_token() {
        let mut token = create_valid_token();
        assert!(token.is_valid());

        token.revoke();

        assert!(!token.is_valid());
        assert!(token.is_revoked());
        assert!(token.revoked_at().is_some());
    }

    #[test]
    fn test_revoke_is_idempotent() {
        let mut token = create_valid_token();
        token.revoke();
        let first_revoked_at = token.revoked_at();

        std::thread::sleep(std::time::Duration::from_millis(10));
        token.revoke();

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
        let expires_at = Utc::now() + Duration::days(30);
        let created_at = Utc::now();

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
        assert!(token.is_valid());
    }
}
