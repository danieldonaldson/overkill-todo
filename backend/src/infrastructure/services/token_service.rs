//! Token generation service implementation.

use std::sync::Arc;

use crate::application::ports::TokenService;
use crate::shared::crypto::Crypto;
use crate::shared::{generate_secure_token, sha256_hex};

/// Token service backed by an injected [`Crypto`] handle.
pub struct CryptoTokenService {
    crypto: Arc<Crypto>,
}

impl CryptoTokenService {
    /// Creates a new token service using the given crypto handle.
    #[must_use]
    pub const fn new(crypto: Arc<Crypto>) -> Self {
        Self { crypto }
    }
}

impl TokenService for CryptoTokenService {
    fn generate_token(&self) -> String {
        generate_secure_token()
    }

    fn hash_token(&self, token: &str) -> String {
        sha256_hex(token)
    }

    fn refresh_token_expiration_days(&self) -> i64 {
        self.crypto.refresh_token_expiration_days()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service() -> CryptoTokenService {
        CryptoTokenService::new(Arc::new(Crypto::for_testing()))
    }

    #[test]
    fn test_generate_token_length() {
        let service = test_service();
        let token = service.generate_token();
        assert_eq!(token.len(), 43); // 32 bytes base64url
    }

    #[test]
    fn test_generate_token_uniqueness() {
        let service = test_service();
        let t1 = service.generate_token();
        let t2 = service.generate_token();
        assert_ne!(t1, t2);
    }

    #[test]
    fn test_hash_token_deterministic() {
        let service = test_service();
        let h1 = service.hash_token("test");
        let h2 = service.hash_token("test");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_token_length() {
        let service = test_service();
        let hash = service.hash_token("test");
        assert_eq!(hash.len(), 64); // SHA256 hex
    }
}
