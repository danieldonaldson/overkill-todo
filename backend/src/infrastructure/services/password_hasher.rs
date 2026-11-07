//! Password hashing service implementation using Argon2.

use std::sync::Arc;

use argon2::PasswordHasher as Argon2PasswordHasher;
use argon2::password_hash::{PasswordHash, PasswordVerifier, SaltString};
use async_trait::async_trait;
use rand_core::OsRng;

use crate::application::ports::PasswordHasher;
use crate::domain::errors::DomainError;
use crate::shared::crypto::Crypto;

/// Argon2-based password hasher backed by an injected [`Crypto`] handle.
///
/// Uses the Argon2id variant with parameters configured from environment variables.
pub struct Argon2Hasher {
    crypto: Arc<Crypto>,
}

impl Argon2Hasher {
    /// Creates a new Argon2 hasher using the given crypto handle.
    #[must_use]
    pub const fn new(crypto: Arc<Crypto>) -> Self {
        Self { crypto }
    }
}

impl Argon2Hasher {
    /// Synchronous Argon2 hash — CPU-bound; run on a blocking thread.
    fn hash_blocking(crypto: &Crypto, password: &str) -> Result<String, DomainError> {
        let salt = SaltString::generate(&mut OsRng);
        let hash = crypto
            .argon2()
            .hash_password(password.as_bytes(), &salt)
            .map_err(|e| DomainError::Persistence(format!("Failed to hash password: {e}")))?;
        Ok(hash.to_string())
    }

    /// Synchronous Argon2 verify — CPU-bound; run on a blocking thread.
    fn verify_blocking(crypto: &Crypto, password: &str, hash: &str) -> Result<bool, DomainError> {
        let parsed_hash = PasswordHash::new(hash)
            .map_err(|e| DomainError::Persistence(format!("Invalid password hash format: {e}")))?;

        Ok(crypto
            .argon2()
            .verify_password(password.as_bytes(), &parsed_hash)
            .is_ok())
    }
}

#[async_trait]
impl PasswordHasher for Argon2Hasher {
    async fn hash(&self, password: &str) -> Result<String, DomainError> {
        // Argon2 is intentionally expensive (tens of ms+); offload it so it never
        // blocks an async worker for the full configured cost.
        let crypto = Arc::clone(&self.crypto);
        let password = password.to_owned();
        tokio::task::spawn_blocking(move || Self::hash_blocking(&crypto, &password))
            .await
            .map_err(|e| DomainError::Persistence(format!("Password hashing task failed: {e}")))?
    }

    async fn verify(&self, password: &str, hash: &str) -> Result<bool, DomainError> {
        let crypto = Arc::clone(&self.crypto);
        let password = password.to_owned();
        let hash = hash.to_owned();
        tokio::task::spawn_blocking(move || Self::verify_blocking(&crypto, &password, &hash))
            .await
            .map_err(|e| {
                DomainError::Persistence(format!("Password verification task failed: {e}"))
            })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_hasher() -> Argon2Hasher {
        Argon2Hasher::new(Arc::new(Crypto::for_testing()))
    }

    #[tokio::test]
    async fn test_hash_produces_valid_argon2_hash() {
        let hasher = test_hasher();
        let hash = hasher.hash("test_password").await.unwrap();

        // Argon2id hashes start with $argon2id$
        assert!(hash.starts_with("$argon2id$"));
    }

    #[tokio::test]
    async fn test_verify_correct_password() {
        let hasher = test_hasher();
        let hash = hasher.hash("correct_password").await.unwrap();

        assert!(hasher.verify("correct_password", &hash).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_incorrect_password() {
        let hasher = test_hasher();
        let hash = hasher.hash("correct_password").await.unwrap();

        assert!(!hasher.verify("wrong_password", &hash).await.unwrap());
    }

    #[tokio::test]
    async fn test_hash_is_unique() {
        let hasher = test_hasher();
        let hash1 = hasher.hash("same_password").await.unwrap();
        let hash2 = hasher.hash("same_password").await.unwrap();

        // Hashes should be different due to random salt
        assert_ne!(hash1, hash2);

        // But both should verify correctly
        assert!(hasher.verify("same_password", &hash1).await.unwrap());
        assert!(hasher.verify("same_password", &hash2).await.unwrap());
    }

    #[tokio::test]
    async fn test_verify_invalid_hash_format() {
        let hasher = test_hasher();
        let result = hasher.verify("password", "not_a_valid_hash").await;

        assert!(result.is_err());
    }

    /// SECURITY: tampering with the digest section of an otherwise well-formed
    /// PHC hash must make verification return `Ok(false)` (a clean mismatch),
    /// NOT `Err` — a parse error here could be misinterpreted as "format issue"
    /// rather than "wrong password".
    #[tokio::test]
    async fn test_verify_rejects_tampered_digest() {
        let hasher = test_hasher();
        let hash = hasher.hash("correct_password").await.unwrap();

        // PHC string: $argon2id$v=..$m=..,t=..,p=..$<salt_b64>$<digest_b64>.
        // Flip the FIRST character of the digest segment. The digest is
        // base64 without padding, so its last character carries only a few
        // significant bits (the rest must be zero for canonical encoding) —
        // tampering there can yield a non-canonical, unparseable string. The
        // first character is fully significant, so any swap stays valid base64
        // and `PasswordHash::new` still parses it.
        let (prefix, digest) = hash
            .rsplit_once('$')
            .expect("argon2 PHC string has a digest segment");
        let first = digest.chars().next().expect("digest is non-empty");
        // Swap to a different, still-valid base64 character.
        let replacement = if first == 'A' { 'B' } else { 'A' };
        let mut tampered_digest = String::from(replacement);
        tampered_digest.push_str(&digest[first.len_utf8()..]);
        let tampered = format!("{prefix}${tampered_digest}");

        assert_ne!(tampered, hash, "tamper must actually change the hash");
        // Confirm it still parses as a PHC string (so this exercises the
        // verify-mismatch path, not the parse-error path).
        assert!(
            PasswordHash::new(&tampered).is_ok(),
            "tampered hash must remain a parseable PHC string"
        );

        let result = hasher
            .verify("correct_password", &tampered)
            .await
            .expect("verify against a parseable-but-wrong digest must not Err");
        assert!(!result, "tampered digest must not verify the password");
    }

    /// Hashing and verifying an empty password round-trips correctly, and a
    /// non-empty password does not verify against the empty-password hash.
    #[tokio::test]
    async fn test_hash_and_verify_empty_password() {
        let hasher = test_hasher();
        let hash = hasher.hash("").await.unwrap();

        assert!(hash.starts_with("$argon2id$"));
        assert!(
            hasher.verify("", &hash).await.unwrap(),
            "empty password must verify against its own hash"
        );
        assert!(
            !hasher.verify("not_empty", &hash).await.unwrap(),
            "a non-empty password must not verify against the empty-password hash"
        );
    }
}
