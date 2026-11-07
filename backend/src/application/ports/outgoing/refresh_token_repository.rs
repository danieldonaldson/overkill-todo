//! Refresh token repository port.

use crate::domain::entities::RefreshToken;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{RefreshTokenId, UserId};
use async_trait::async_trait;

/// Repository trait for refresh token persistence operations.
///
/// This trait defines the contract for storing and managing refresh tokens.
/// Tokens are stored with their hash (never plaintext) for security.
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
#[async_trait]
pub trait RefreshTokenRepository: Send + Sync {
    /// Persists a new refresh token to the database.
    ///
    /// The token should already have its value hashed before calling this method.
    async fn save(&self, token: &RefreshToken) -> Result<(), DomainError>;

    /// Finds a refresh token by its hash.
    ///
    /// This is the primary lookup method used during token refresh.
    async fn find_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>, DomainError>;

    /// Revokes a specific token by its ID.
    ///
    /// Sets the `revoked_at` timestamp to the current time.
    async fn revoke(&self, id: &RefreshTokenId) -> Result<(), DomainError>;

    /// Revokes all tokens for a specific user.
    ///
    /// Useful for "logout all devices" functionality or when
    /// a security concern requires invalidating all sessions.
    async fn revoke_all_for_user(&self, user_id: &UserId) -> Result<u64, DomainError>;

    /// Revokes all of a user's tokens *except* the one whose hash is
    /// `keep_token_hash`.
    ///
    /// Powers "log out other devices": the caller's current session (identified
    /// by its refresh-token hash) stays valid while every other device is
    /// evicted. Returns the number of tokens revoked.
    async fn revoke_all_for_user_except(
        &self,
        user_id: &UserId,
        keep_token_hash: &str,
    ) -> Result<u64, DomainError>;

    /// Atomically revokes `old_id` and saves `new_token` in a single transaction.
    ///
    /// Prevents the window where a crash between separate revoke + save calls
    /// would log a user out permanently. The revoke is single-use: if `old_id`
    /// was already revoked (e.g. a concurrent refresh won the race), this returns
    /// [`DomainError::Conflict`] and persists no successor token.
    async fn rotate(
        &self,
        old_id: &RefreshTokenId,
        new_token: &RefreshToken,
    ) -> Result<(), DomainError>;

    /// Deletes all expired tokens from the database.
    ///
    /// This is a cleanup operation that should be run periodically.
    /// Returns the number of tokens deleted.
    async fn delete_expired(&self) -> Result<u64, DomainError>;
}
