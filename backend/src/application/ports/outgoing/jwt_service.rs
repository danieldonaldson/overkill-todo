//! JWT service port for token creation and validation.

use crate::domain::errors::DomainError;
use crate::domain::value_objects::{ResetCodeId, UserId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Claims for a signup/reset token (short-lived, specific purpose).
#[derive(Debug, Serialize, Deserialize)]
pub struct SignupTokenClaims {
    /// Subject (user ID)
    pub sub: String,
    /// Expiration time
    pub exp: i64,
    /// Token type (e.g., "signup", "reset")
    pub typ: String,
}

/// Trait for JWT operations.
///
/// Handles creation and validation of JWTs for auth flows.
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
pub trait JwtService: Send + Sync {
    /// Creates a signup token for completing registration.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The user's ID
    /// * `expires_at` - When the token expires
    ///
    /// # Returns
    ///
    /// A signed JWT string.
    ///
    /// # Errors
    ///
    /// Returns an error if token creation fails.
    fn create_signup_token(
        &self,
        user_id: &UserId,
        expires_at: DateTime<Utc>,
    ) -> Result<String, DomainError>;

    /// Creates a password reset token bound to a specific reset-code row.
    ///
    /// The `reset_code_id` is embedded as the JWT `jti` so the token can be
    /// consumed exactly once: completing the reset deletes that row, which
    /// invalidates the token even before it expires.
    ///
    /// # Arguments
    ///
    /// * `user_id` - The user's ID
    /// * `reset_code_id` - The reset-code row this token authorizes
    /// * `expires_at` - When the token expires
    ///
    /// # Returns
    ///
    /// A signed JWT string.
    ///
    /// # Errors
    ///
    /// Returns an error if token creation fails.
    fn create_reset_token(
        &self,
        user_id: &UserId,
        reset_code_id: &ResetCodeId,
        expires_at: DateTime<Utc>,
    ) -> Result<String, DomainError>;

    /// Validates a signup token and extracts the user ID.
    ///
    /// # Arguments
    ///
    /// * `token` - The JWT to validate
    ///
    /// # Returns
    ///
    /// The user ID if valid.
    ///
    /// # Errors
    ///
    /// Returns an error if the token is invalid or expired.
    fn validate_signup_token(&self, token: &str) -> Result<UserId, DomainError>;

    /// Validates a reset token and extracts the user ID and reset-code id.
    ///
    /// # Arguments
    ///
    /// * `token` - The JWT to validate
    ///
    /// # Returns
    ///
    /// The user ID and the reset-code id (`jti`) the token is bound to.
    ///
    /// # Errors
    ///
    /// Returns an error if the token is invalid, expired, or missing its `jti`.
    fn validate_reset_token(&self, token: &str) -> Result<(UserId, ResetCodeId), DomainError>;

    /// Returns the JWT expiration in minutes.
    fn jwt_expiration_minutes(&self) -> i64;
}
