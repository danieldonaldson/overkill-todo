//! Secure token generation service port.

/// Trait for generating and hashing secure tokens.
///
/// Used for refresh tokens, magic link tokens, etc.
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
pub trait TokenService: Send + Sync {
    /// Generates a cryptographically secure random token.
    ///
    /// Returns a URL-safe base64 encoded token.
    fn generate_token(&self) -> String;

    /// Computes SHA256 hash of a token for storage.
    ///
    /// Tokens are stored as hashes for security.
    fn hash_token(&self, token: &str) -> String;

    /// Returns the refresh token expiration in days.
    fn refresh_token_expiration_days(&self) -> i64;
}
