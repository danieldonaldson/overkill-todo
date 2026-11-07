//! Cryptographic utilities for password hashing, JWT key management, and token generation.
//!
//! Exposes a [`Crypto`] handle that holds precomputed Argon2 parameters, JWT
//! keys, the CSRF secret, and the at-rest secret cipher. It is built once from
//! validated [`CryptoSettings`] in `main` and injected as `Arc<Crypto>` into the
//! adapters that need it (no process-wide singleton).
//!
//! # Environment Variables (all required, no defaults)
//!
//! - `ARGON2_MEMORY_COST` - Memory cost in KiB
//! - `ARGON2_TIME_COST` - Number of iterations
//! - `ARGON2_PARALLELISM` - Degree of parallelism
//! - `JWT_PRIVATE_KEY` - RSA private key in PEM format
//! - `JWT_PUBLIC_KEY` - RSA public key in PEM format
//! - `JWT_EXPIRATION_MINUTES` - JWT token validity in minutes
//! - `REFRESH_TOKEN_EXPIRATION_DAYS` - Refresh token validity in days
//! - `CSRF_SECRET` - Secret for CSRF token signing (base64-encoded)

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use argon2::{Argon2, Params};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use jsonwebtoken::{DecodingKey, EncodingKey};
use rand::Rng;
use sha2::{Digest, Sha256};

use crate::infrastructure::config::CryptoSettings;

/// Wire-format prefix for AES-256-GCM-encrypted secrets. The scheme tag plus
/// version let us migrate to a new algorithm or rotate keys later without
/// guessing at the contents of a column.
const SECRET_CIPHER_PREFIX: &str = "aesgcm.v1.";
/// AES-GCM standard nonce length in bytes.
const SECRET_NONCE_LEN: usize = 12;

/// Errors raised when encrypting or decrypting persisted operational secrets.
#[derive(Debug, thiserror::Error)]
pub enum SecretCipherError {
    /// The stored value was not produced by this cipher (missing/unknown scheme
    /// tag, bad base64, or truncated payload).
    #[error("stored secret is not a recognized ciphertext")]
    Malformed,
    /// AEAD verification failed — wrong key or tampered ciphertext.
    #[error("secret decryption failed")]
    Decrypt,
    /// AEAD encryption failed (should not happen for normal inputs).
    #[error("secret encryption failed")]
    Encrypt,
}

/// Errors raised while building [`Crypto`] from [`CryptoSettings`].
///
/// Returned via `?` from [`Crypto::from_settings`] so misconfiguration surfaces
/// as a clean, actionable message at startup rather than a raw panic.
#[derive(Debug, thiserror::Error)]
pub enum CryptoInitError {
    /// The Argon2 cost parameters are out of range.
    #[error("invalid Argon2 parameters: {0}")]
    Argon2Params(argon2::Error),
    /// `CSRF_SECRET` is not valid base64 (url-safe or standard).
    #[error("CSRF_SECRET must be valid base64 (url-safe or standard)")]
    CsrfSecret,
    /// `SECRETS_ENCRYPTION_KEY` is not valid base64 (url-safe or standard).
    #[error("SECRETS_ENCRYPTION_KEY must be valid base64 (url-safe or standard)")]
    SecretsKeyEncoding,
    /// `SECRETS_ENCRYPTION_KEY` did not decode to exactly 32 bytes (AES-256).
    #[error("SECRETS_ENCRYPTION_KEY must decode to 32 bytes (AES-256); got {0} bytes")]
    SecretsKeyLength(usize),
    /// `JWT_PRIVATE_KEY` is not a parseable RSA private key in PEM form.
    #[error("JWT_PRIVATE_KEY is not a valid RSA private key (PEM): {0}")]
    JwtPrivateKey(jsonwebtoken::errors::Error),
    /// `JWT_PUBLIC_KEY` is not a parseable RSA public key in PEM form.
    #[error("JWT_PUBLIC_KEY is not a valid RSA public key (PEM): {0}")]
    JwtPublicKey(jsonwebtoken::errors::Error),
}

/// Decode a base64 secret, accepting either url-safe (no padding) or standard
/// (padded) alphabets. Tolerating both avoids the boot-breaking `.env.example`
/// drift where `openssl rand -base64 32` emits STANDARD base64.
fn decode_secret_b64(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    URL_SAFE_NO_PAD
        .decode(value)
        .or_else(|_| base64::engine::general_purpose::STANDARD.decode(value))
}

/// Precomputed cryptographic primitives for password hashing and JWT operations.
///
/// These are expensive to create, so they're built once at startup and reused.
pub struct Crypto {
    argon2: Argon2<'static>,
    encoding_key: EncodingKey,
    decoding_key: DecodingKey,
    jwt_expiration_minutes: i64,
    refresh_token_expiration_days: i64,
    csrf_secret: Vec<u8>,
    secret_cipher: Aes256Gcm,
}

impl Crypto {
    /// Build from validated application settings.
    ///
    /// # Errors
    ///
    /// Returns a [`CryptoInitError`] if the Argon2 parameters, base64 secrets,
    /// key length, or RSA PEM keys in `settings` are malformed. All failure
    /// paths surface as a clean message instead of a panic.
    pub fn from_settings(settings: &CryptoSettings) -> Result<Self, CryptoInitError> {
        let params = Params::new(
            settings.argon2_memory_cost,
            settings.argon2_time_cost,
            settings.argon2_parallelism,
            None,
        )
        .map_err(CryptoInitError::Argon2Params)?;
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

        let csrf_secret = decode_secret_b64(&settings.csrf_secret_b64)
            .map_err(|_| CryptoInitError::CsrfSecret)?;

        let secret_key_bytes = decode_secret_b64(&settings.secrets_encryption_key_b64)
            .map_err(|_| CryptoInitError::SecretsKeyEncoding)?;
        if secret_key_bytes.len() != 32 {
            return Err(CryptoInitError::SecretsKeyLength(secret_key_bytes.len()));
        }
        let secret_cipher = Aes256Gcm::new(
            &Key::<Aes256Gcm>::try_from(&secret_key_bytes[..])
                .map_err(|_| CryptoInitError::SecretsKeyLength(secret_key_bytes.len()))?,
        );

        Ok(Self {
            argon2,
            encoding_key: EncodingKey::from_rsa_pem(settings.jwt_private_key.as_bytes())
                .map_err(CryptoInitError::JwtPrivateKey)?,
            decoding_key: DecodingKey::from_rsa_pem(settings.jwt_public_key.as_bytes())
                .map_err(CryptoInitError::JwtPublicKey)?,
            jwt_expiration_minutes: settings.jwt_expiration_minutes,
            refresh_token_expiration_days: settings.refresh_token_expiration_days,
            csrf_secret,
            secret_cipher,
        })
    }

    /// Construct a Crypto instance with fixed test-safe parameters.
    ///
    /// Only intended for use in unit tests — do not call in production code.
    ///
    /// # Panics
    ///
    /// Panics if the hardcoded test keys or parameters are malformed (they are not).
    #[cfg(any(test, feature = "mocks"))]
    #[must_use]
    pub fn for_testing() -> Self {
        const TEST_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCrrWYCApN+866p\nxXEZHqkl9DTa/nQKcx1LPwryXBqAEfZen+gf038DjbmmzqggEbYTx62aUTiSAEs0\nNoouFy/+B9UUepaOMkQN/JqtxwdQjGSFb9++WCM6+5X7iXoyzjRg0VFqyA6LGGs7\nx1uxStvifhOcmFxLBXPD5shVYlT7KoERRv1FGsX4vbfuAjAku4MDuq7FDmc2a86P\nDGEcmrqGiEKlHrJgEPdYa8/OHxE6/Qy/UgQUjN0sQS7OjwbP5ef+93s8JE03JbLo\nljuerCLR5JVJmgZ4IQOOh5bdCR+/K3ZbyETOG+5Sop3iHMLwGCBls9BKK0eBYaPd\n9pGqzifzAgMBAAECggEADWV4gkulKtDlHkZT7vvfgyCtRzF36KDW9lozBnp9buT3\n7qZXvLQKQ0K56/RQVkBLjvuT5mjUG0B+ndJrsfEJQ69kxzZRrULq4JSHlWvkfDqY\nVXlHYlkKVr3vFDPEH1bL64op6kU2LVB4fa+uzgTjZ6ci7KNPAGOqhuzpURBdRUQI\n5UGDNcZPl5jWBRb9Jtjjkaxtvm2qSXC7jeV6rvcjkQhWHJFL2xrhT1IRKj8bUrLD\n21aLTM+mLW2iCuDSfOC3pWfZoQEhNkdVqsKFnKVOPIGRDysPqeRfloE9uMT7YK95\nT/aRksVvPE+bEDbKvdTsuOvvTKY2HRu2St+KbKtXkQKBgQDqpIArr6eftuInb2a8\ndYIuIWfI6UV9ernc9uXglo9JNGgXqcMI7wzaP35EkqAFROiXg29jjkUgQw8BDqwt\n437ic+Wf5+EYq8hQ8L0ig+zstt3FQ3v2i/teBZqOUsoCElXeYxR4DNbQCEbYAEs4\nzGRoqrBCQ2wwEbp0wxNswTKGvwKBgQC7TbhHQeA1es2RMvfZHIzO/8IoSyUV1F9/\nVDgX+pcUZ0FZfb2OMIDwb1w/1A+84NnC63iSI9E/6Rr6XSCNJk9eJBMBoZ2AD/4s\ngDLI2iAUrQHtuNIhshPAbCaW6AVw46fwtCDALFhXK3rw12nezrgYWfehODqZH0R/\nke9kxuD/zQKBgCY3WLDE0TZQIRFvMXpsYSGGS1MLu0eun2r/w30KyJeRs4p3Gc7w\nsFUzlisw/mMdUAytWY0LdPLGOUFg1GY88UOMwM+EgsLz55JXQ1bFyebb+c7RKPWE\n4xHzM7MOhNxFhDrORF0xKiOD1ZYEpju//nKVqlOAi7Rgjs/JnfZOmR4dAoGAXcX0\na6B8iunXf0lgMs+LY5tCbn/oqjwwL4RPrRmlh+6GFjLQsx5vf3CRY9dcpbiA7YiM\nTcYm3fNrsUrsoB49ACnZZiiuvM3E+CyawFoMU62mWIj+kyYfo2fCDi1NhA5GJ0lX\nWD8bX1ECdQ+0Dnu6SV7JjmqjU8QGvEOo8fBr90UCgYEAxvr4swdT46qEm8kWHU3I\ng7yR2rbNYP9TUqg2YKTX3B1Ab9uX1S9c9gg2RdPp5Mxs4XSqVfqJr+yfZP3lSyUa\nT6l2cr36CBT9QNCpHNsfnv4ar+RDqxxojd1+5RFJU4Kg9cWbwB/oIB0g5T1q0HqJ\npqUx9x2LMNqJelwDx/57zt4=\n-----END PRIVATE KEY-----";
        const TEST_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAq61mAgKTfvOuqcVxGR6p\nJfQ02v50CnMdSz8K8lwagBH2Xp/oH9N/A425ps6oIBG2E8etmlE4kgBLNDaKLhcv\n/gfVFHqWjjJEDfyarccHUIxkhW/fvlgjOvuV+4l6Ms40YNFRasgOixhrO8dbsUrb\n4n4TnJhcSwVzw+bIVWJU+yqBEUb9RRrF+L237gIwJLuDA7quxQ5nNmvOjwxhHJq6\nhohCpR6yYBD3WGvPzh8ROv0Mv1IEFIzdLEEuzo8Gz+Xn/vd7PCRNNyWy6JY7nqwi\n0eSVSZoGeCEDjoeW3Qkfvyt2W8hEzhvuUqKd4hzC8BggZbPQSitHgWGj3faRqs4n\n8wIDAQAB\n-----END PUBLIC KEY-----";

        let params = Params::new(4096, 1, 1, None).expect("test Argon2 params are valid");
        let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);
        let csrf_secret = URL_SAFE_NO_PAD
            .decode("MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA")
            .expect("test CSRF secret is valid base64");

        // Fixed all-zero-ish 32-byte key — test use only, never production.
        let secret_key = [7u8; 32];
        let secret_cipher = Aes256Gcm::new(
            &Key::<Aes256Gcm>::try_from(&secret_key[..]).expect("test secret key is 32 bytes"),
        );

        Self {
            argon2,
            encoding_key: EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes())
                .expect("test RSA private key is valid"),
            decoding_key: DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes())
                .expect("test RSA public key is valid"),
            jwt_expiration_minutes: 15,
            refresh_token_expiration_days: 30,
            csrf_secret,
            secret_cipher,
        }
    }

    /// Encrypts an operational secret for storage at rest using AES-256-GCM.
    ///
    /// Returns a self-describing string: a scheme/version tag followed by the
    /// base64url-encoded random nonce concatenated with the ciphertext+tag. Each
    /// call uses a fresh random nonce, so encrypting the same plaintext twice
    /// yields different output.
    ///
    /// `aad` is bound into the AEAD authentication tag as associated data — pass
    /// the owning context (e.g. the user id) so the ciphertext authenticates not
    /// just "encrypted by this key" but "belongs to this owner". A row whose
    /// ciphertext is transplanted into a different owner's row then fails to
    /// decrypt under that owner's `aad`. Pass `&[]` for an unbound secret.
    ///
    /// # Errors
    ///
    /// Returns [`SecretCipherError::Encrypt`] if the AEAD operation fails (does
    /// not happen for normal-sized inputs).
    pub fn encrypt_secret(&self, plaintext: &str, aad: &[u8]) -> Result<String, SecretCipherError> {
        let mut nonce_bytes = [0u8; SECRET_NONCE_LEN];
        rand::rng().fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);

        let ciphertext = self
            .secret_cipher
            .encrypt(
                &nonce,
                Payload {
                    msg: plaintext.as_bytes(),
                    aad,
                },
            )
            .map_err(|_| SecretCipherError::Encrypt)?;

        let mut payload = Vec::with_capacity(SECRET_NONCE_LEN + ciphertext.len());
        payload.extend_from_slice(&nonce_bytes);
        payload.extend_from_slice(&ciphertext);

        Ok(format!(
            "{SECRET_CIPHER_PREFIX}{}",
            URL_SAFE_NO_PAD.encode(&payload)
        ))
    }

    /// Decrypts a secret previously produced by [`Crypto::encrypt_secret`].
    ///
    /// `aad` must match the associated data supplied at encryption time
    /// (typically the owning context, e.g. the user id), or AEAD verification
    /// fails — this is what makes a cross-owner ciphertext transplant unusable.
    ///
    /// # Errors
    ///
    /// Returns [`SecretCipherError::Malformed`] if `stored` is not a recognized
    /// ciphertext, or [`SecretCipherError::Decrypt`] if authentication fails
    /// (wrong key, wrong `aad`, or tampering).
    pub fn decrypt_secret(&self, stored: &str, aad: &[u8]) -> Result<String, SecretCipherError> {
        let encoded = stored
            .strip_prefix(SECRET_CIPHER_PREFIX)
            .ok_or(SecretCipherError::Malformed)?;
        let payload = URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| SecretCipherError::Malformed)?;
        if payload.len() <= SECRET_NONCE_LEN {
            return Err(SecretCipherError::Malformed);
        }
        let (nonce_bytes, ciphertext) = payload.split_at(SECRET_NONCE_LEN);
        let nonce = Nonce::try_from(nonce_bytes).map_err(|_| SecretCipherError::Decrypt)?;

        let plaintext = self
            .secret_cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| SecretCipherError::Decrypt)?;
        String::from_utf8(plaintext).map_err(|_| SecretCipherError::Decrypt)
    }

    /// Returns the configured Argon2 hasher.
    #[must_use]
    pub const fn argon2(&self) -> &Argon2<'static> {
        &self.argon2
    }

    /// Returns the JWT encoding (private) key.
    #[must_use]
    pub const fn encoding_key(&self) -> &EncodingKey {
        &self.encoding_key
    }

    /// Returns the JWT decoding (public) key.
    #[must_use]
    pub const fn decoding_key(&self) -> &DecodingKey {
        &self.decoding_key
    }

    /// Returns the configured JWT expiration time in minutes.
    #[must_use]
    pub const fn jwt_expiration_minutes(&self) -> i64 {
        self.jwt_expiration_minutes
    }

    /// Returns the configured refresh token expiration time in days.
    #[must_use]
    pub const fn refresh_token_expiration_days(&self) -> i64 {
        self.refresh_token_expiration_days
    }

    /// Returns the CSRF secret for token signing.
    #[must_use]
    pub fn csrf_secret(&self) -> &[u8] {
        &self.csrf_secret
    }
}

/// Generates a cryptographically secure random token.
///
/// Returns a 32-byte random value encoded as URL-safe base64 (no padding).
/// This produces a 43-character string suitable for use as a refresh token.
///
/// # Examples
///
/// ```
/// use app::shared::crypto::generate_secure_token;
///
/// let token = generate_secure_token();
/// assert_eq!(token.len(), 43); // 32 bytes -> 43 base64 chars
/// ```
#[must_use]
pub fn generate_secure_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Computes the SHA-256 hash of the input and returns it as a hex string.
///
/// This is used to hash refresh tokens before storing them in the database.
/// The resulting string is always 64 characters long.
///
/// # Examples
///
/// ```
/// use app::shared::crypto::sha256_hex;
///
/// let hash = sha256_hex("my_secret_token");
/// assert_eq!(hash.len(), 64);
/// ```
#[must_use]
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn test_generate_secure_token_length() {
        let token = generate_secure_token();
        assert_eq!(token.len(), 43); // 32 bytes -> 43 base64url chars
    }

    #[test]
    fn test_generate_secure_token_uniqueness() {
        let token1 = generate_secure_token();
        let token2 = generate_secure_token();
        assert_ne!(token1, token2);
    }

    #[test]
    fn secret_cipher_round_trips() {
        let crypto = Crypto::for_testing();
        let plaintext = "sk-super-secret-api-key";
        let aad = b"user-123";
        let encrypted = crypto.encrypt_secret(plaintext, aad).expect("encrypts");
        assert!(encrypted.starts_with(SECRET_CIPHER_PREFIX));
        assert_ne!(encrypted, plaintext);
        assert_eq!(
            crypto.decrypt_secret(&encrypted, aad).expect("decrypts"),
            plaintext
        );
    }

    /// Ciphertext is bound to its associated data: decrypting under a different
    /// `aad` (e.g. another user's id) fails authentication. This is the
    /// cross-owner transplant defense.
    #[test]
    fn secret_cipher_rejects_mismatched_aad() {
        let crypto = Crypto::for_testing();
        let encrypted = crypto
            .encrypt_secret("sk-owner-a", b"user-aaaa")
            .expect("encrypts");
        assert!(matches!(
            crypto.decrypt_secret(&encrypted, b"user-bbbb"),
            Err(SecretCipherError::Decrypt)
        ));
        // The rightful owner still decrypts.
        assert_eq!(
            crypto
                .decrypt_secret(&encrypted, b"user-aaaa")
                .expect("decrypts"),
            "sk-owner-a"
        );
    }

    #[test]
    fn secret_cipher_uses_fresh_nonce_each_call() {
        let crypto = Crypto::for_testing();
        let a = crypto.encrypt_secret("same", b"").expect("encrypts");
        let b = crypto.encrypt_secret("same", b"").expect("encrypts");
        assert_ne!(a, b, "each encryption must use a fresh random nonce");
    }

    #[test]
    fn secret_cipher_rejects_plaintext_and_tampering() {
        let crypto = Crypto::for_testing();
        assert!(matches!(
            crypto.decrypt_secret("not-encrypted", b""),
            Err(SecretCipherError::Malformed)
        ));

        let mut encrypted = crypto.encrypt_secret("secret", b"").expect("encrypts");
        // Flip the final base64 character to corrupt the ciphertext/tag.
        let last = encrypted.pop().expect("non-empty");
        encrypted.push(if last == 'A' { 'B' } else { 'A' });
        assert!(crypto.decrypt_secret(&encrypted, b"").is_err());
    }

    /// Golden vector — a ciphertext captured under `Crypto::for_testing`'s
    /// fixed key (with empty AAD) must keep decrypting to the same plaintext.
    /// Guards the at-rest wire format (`aesgcm.v1.` + nonce||ciphertext) against
    /// silent breakage.
    #[test]
    fn secret_cipher_golden_vector_decrypts() {
        const GOLDEN: &str =
            "aesgcm.v1.gtA4RxgnrgyMkyu0a40D0QJT1ocfndN3ckE2Yic-g3zAEtDSUxxhsUVW08nw9q8";
        let crypto = Crypto::for_testing();
        assert_eq!(
            crypto
                .decrypt_secret(GOLDEN, b"")
                .expect("golden vector decrypts"),
            "golden-secret-value"
        );
    }

    /// Any scheme other than the current `aesgcm.v1.` prefix must be
    /// rejected as `Malformed` rather than silently mis-decrypted — so a future
    /// key/format rotation is a deliberate, fail-loud change.
    #[test]
    fn decrypt_rejects_unknown_scheme() {
        let crypto = Crypto::for_testing();
        for stored in [
            "plaintext-no-prefix",
            "aesgcm.v2.AAAA", // future scheme
            "aesgcm.AAAA",    // missing version
            "AESGCM.V1.AAAA", // wrong case
        ] {
            assert!(
                matches!(
                    crypto.decrypt_secret(stored, b""),
                    Err(SecretCipherError::Malformed)
                ),
                "scheme must be rejected as Malformed: {stored}"
            );
        }
    }

    #[test]
    fn test_sha256_hex_length() {
        let hash = sha256_hex("test");
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_sha256_hex_deterministic() {
        let hash1 = sha256_hex("same_input");
        let hash2 = sha256_hex("same_input");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_sha256_hex_different_inputs() {
        let hash1 = sha256_hex("input1");
        let hash2 = sha256_hex("input2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_sha256_hex_known_value() {
        // SHA-256 of "hello" is well-known
        let hash = sha256_hex("hello");
        assert_eq!(
            hash,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    // ==================== Property Tests ====================

    proptest! {
        /// generate_secure_token always produces 43-character base64url strings.
        #[test]
        fn prop_secure_token_format(_seed in 0u64..1000) {
            let token = generate_secure_token();
            prop_assert_eq!(token.len(), 43);
            // Verify it's valid base64url (no padding)
            let decoded = URL_SAFE_NO_PAD.decode(&token);
            prop_assert!(decoded.is_ok());
            prop_assert_eq!(decoded.unwrap().len(), 32);
        }

        /// sha256_hex always produces 64-character hex strings.
        #[test]
        fn prop_sha256_hex_length(input in "\\PC{0,1000}") {
            let hash = sha256_hex(&input);
            prop_assert_eq!(hash.len(), 64);
        }

        /// sha256_hex output contains only hex characters.
        #[test]
        fn prop_sha256_hex_format(input in "\\PC{0,100}") {
            let hash = sha256_hex(&input);
            prop_assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        }

        /// sha256_hex is deterministic - same input produces same output.
        #[test]
        fn prop_sha256_hex_deterministic(input in "\\PC{0,100}") {
            let hash1 = sha256_hex(&input);
            let hash2 = sha256_hex(&input);
            prop_assert_eq!(hash1, hash2);
        }

        /// Different inputs (almost always) produce different hashes.
        #[test]
        fn prop_sha256_hex_collision_resistant(
            input1 in "\\PC{1,50}",
            input2 in "\\PC{1,50}"
        ) {
            prop_assume!(input1 != input2);
            let hash1 = sha256_hex(&input1);
            let hash2 = sha256_hex(&input2);
            // SHA-256 collision probability is astronomically low
            prop_assert_ne!(hash1, hash2);
        }

        /// Empty string has a valid hash.
        #[test]
        fn prop_sha256_hex_empty_string_valid(_seed in 0u64..1) {
            let hash = sha256_hex("");
            prop_assert_eq!(hash.len(), 64);
            prop_assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
        }
    }

    // ---- decode_secret_b64 alphabet tolerance -------------------------------

    #[test]
    fn decode_secret_b64_accepts_url_safe_and_standard() {
        // "hello" -> url-safe no-pad and standard padded both decode to the same bytes.
        let url_safe = URL_SAFE_NO_PAD.encode(b"hello");
        let standard = base64::engine::general_purpose::STANDARD.encode(b"hello");
        assert_eq!(decode_secret_b64(&url_safe).unwrap(), b"hello");
        // Exercises the `.or_else(STANDARD)` fallback branch.
        assert_eq!(decode_secret_b64(&standard).unwrap(), b"hello");
    }

    #[test]
    fn decode_secret_b64_rejects_garbage() {
        assert!(decode_secret_b64("!!!not base64!!!").is_err());
    }

    // ---- Crypto::from_settings error paths ----------------------------------
    //
    // Reuse `for_testing`'s known-good RSA PEM keys and base64 secrets so each
    // test isolates exactly one malformed field.

    const VALID_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCrrWYCApN+866p\nxXEZHqkl9DTa/nQKcx1LPwryXBqAEfZen+gf038DjbmmzqggEbYTx62aUTiSAEs0\nNoouFy/+B9UUepaOMkQN/JqtxwdQjGSFb9++WCM6+5X7iXoyzjRg0VFqyA6LGGs7\nx1uxStvifhOcmFxLBXPD5shVYlT7KoERRv1FGsX4vbfuAjAku4MDuq7FDmc2a86P\nDGEcmrqGiEKlHrJgEPdYa8/OHxE6/Qy/UgQUjN0sQS7OjwbP5ef+93s8JE03JbLo\nljuerCLR5JVJmgZ4IQOOh5bdCR+/K3ZbyETOG+5Sop3iHMLwGCBls9BKK0eBYaPd\n9pGqzifzAgMBAAECggEADWV4gkulKtDlHkZT7vvfgyCtRzF36KDW9lozBnp9buT3\n7qZXvLQKQ0K56/RQVkBLjvuT5mjUG0B+ndJrsfEJQ69kxzZRrULq4JSHlWvkfDqY\nVXlHYlkKVr3vFDPEH1bL64op6kU2LVB4fa+uzgTjZ6ci7KNPAGOqhuzpURBdRUQI\n5UGDNcZPl5jWBRb9Jtjjkaxtvm2qSXC7jeV6rvcjkQhWHJFL2xrhT1IRKj8bUrLD\n21aLTM+mLW2iCuDSfOC3pWfZoQEhNkdVqsKFnKVOPIGRDysPqeRfloE9uMT7YK95\nT/aRksVvPE+bEDbKvdTsuOvvTKY2HRu2St+KbKtXkQKBgQDqpIArr6eftuInb2a8\ndYIuIWfI6UV9ernc9uXglo9JNGgXqcMI7wzaP35EkqAFROiXg29jjkUgQw8BDqwt\n437ic+Wf5+EYq8hQ8L0ig+zstt3FQ3v2i/teBZqOUsoCElXeYxR4DNbQCEbYAEs4\nzGRoqrBCQ2wwEbp0wxNswTKGvwKBgQC7TbhHQeA1es2RMvfZHIzO/8IoSyUV1F9/\nVDgX+pcUZ0FZfb2OMIDwb1w/1A+84NnC63iSI9E/6Rr6XSCNJk9eJBMBoZ2AD/4s\ngDLI2iAUrQHtuNIhshPAbCaW6AVw46fwtCDALFhXK3rw12nezrgYWfehODqZH0R/\nke9kxuD/zQKBgCY3WLDE0TZQIRFvMXpsYSGGS1MLu0eun2r/w30KyJeRs4p3Gc7w\nsFUzlisw/mMdUAytWY0LdPLGOUFg1GY88UOMwM+EgsLz55JXQ1bFyebb+c7RKPWE\n4xHzM7MOhNxFhDrORF0xKiOD1ZYEpju//nKVqlOAi7Rgjs/JnfZOmR4dAoGAXcX0\na6B8iunXf0lgMs+LY5tCbn/oqjwwL4RPrRmlh+6GFjLQsx5vf3CRY9dcpbiA7YiM\nTcYm3fNrsUrsoB49ACnZZiiuvM3E+CyawFoMU62mWIj+kyYfo2fCDi1NhA5GJ0lX\nWD8bX1ECdQ+0Dnu6SV7JjmqjU8QGvEOo8fBr90UCgYEAxvr4swdT46qEm8kWHU3I\ng7yR2rbNYP9TUqg2YKTX3B1Ab9uX1S9c9gg2RdPp5Mxs4XSqVfqJr+yfZP3lSyUa\nT6l2cr36CBT9QNCpHNsfnv4ar+RDqxxojd1+5RFJU4Kg9cWbwB/oIB0g5T1q0HqJ\npqUx9x2LMNqJelwDx/57zt4=\n-----END PRIVATE KEY-----";
    const VALID_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAq61mAgKTfvOuqcVxGR6p\nJfQ02v50CnMdSz8K8lwagBH2Xp/oH9N/A425ps6oIBG2E8etmlE4kgBLNDaKLhcv\n/gfVFHqWjjJEDfyarccHUIxkhW/fvlgjOvuV+4l6Ms40YNFRasgOixhrO8dbsUrb\n4n4TnJhcSwVzw+bIVWJU+yqBEUb9RRrF+L237gIwJLuDA7quxQ5nNmvOjwxhHJq6\nhohCpR6yYBD3WGvPzh8ROv0Mv1IEFIzdLEEuzo8Gz+Xn/vd7PCRNNyWy6JY7nqwi\n0eSVSZoGeCEDjoeW3Qkfvyt2W8hEzhvuUqKd4hzC8BggZbPQSitHgWGj3faRqs4n\n8wIDAQAB\n-----END PUBLIC KEY-----";
    const VALID_CSRF_B64: &str = "MWq42LqA6RbYEsgrc0w2Ik9YydReTlRwh_D5Ynus2QA";

    fn valid_settings() -> CryptoSettings {
        CryptoSettings {
            argon2_memory_cost: 4096,
            argon2_time_cost: 1,
            argon2_parallelism: 1,
            jwt_private_key: VALID_PRIVATE_KEY.to_string(),
            jwt_public_key: VALID_PUBLIC_KEY.to_string(),
            jwt_expiration_minutes: 15,
            refresh_token_expiration_days: 30,
            csrf_secret_b64: VALID_CSRF_B64.to_string(),
            // 32 zero bytes, url-safe base64.
            secrets_encryption_key_b64: URL_SAFE_NO_PAD.encode([0u8; 32]),
        }
    }

    #[test]
    fn from_settings_succeeds_with_valid_settings() {
        let crypto = Crypto::from_settings(&valid_settings()).expect("valid settings build");
        assert_eq!(crypto.jwt_expiration_minutes(), 15);
        assert_eq!(crypto.refresh_token_expiration_days(), 30);
        assert_ne!(crypto.csrf_secret().len(), 0);
        // The constructed cipher round-trips.
        let enc = crypto.encrypt_secret("x", b"").unwrap();
        assert_eq!(crypto.decrypt_secret(&enc, b"").unwrap(), "x");
    }

    #[test]
    fn from_settings_rejects_invalid_argon2_params() {
        let mut s = valid_settings();
        // parallelism of 0 is below Argon2's minimum.
        s.argon2_parallelism = 0;
        assert!(matches!(
            Crypto::from_settings(&s),
            Err(CryptoInitError::Argon2Params(_))
        ));
    }

    #[test]
    fn from_settings_rejects_bad_csrf_base64() {
        let mut s = valid_settings();
        s.csrf_secret_b64 = "!!!not base64!!!".to_string();
        assert!(matches!(
            Crypto::from_settings(&s),
            Err(CryptoInitError::CsrfSecret)
        ));
    }

    #[test]
    fn from_settings_rejects_bad_secrets_key_encoding() {
        let mut s = valid_settings();
        s.secrets_encryption_key_b64 = "!!!not base64!!!".to_string();
        assert!(matches!(
            Crypto::from_settings(&s),
            Err(CryptoInitError::SecretsKeyEncoding)
        ));
    }

    #[test]
    fn from_settings_rejects_wrong_secrets_key_length() {
        let mut s = valid_settings();
        // 16 bytes instead of 32.
        s.secrets_encryption_key_b64 = URL_SAFE_NO_PAD.encode([0u8; 16]);
        match Crypto::from_settings(&s) {
            Err(CryptoInitError::SecretsKeyLength(n)) => assert_eq!(n, 16),
            Err(other) => panic!("expected SecretsKeyLength(16), got {other:?}"),
            Ok(_) => panic!("expected SecretsKeyLength(16), got Ok"),
        }
    }

    #[test]
    fn from_settings_rejects_bad_private_key() {
        let mut s = valid_settings();
        s.jwt_private_key = "not a pem key".to_string();
        assert!(matches!(
            Crypto::from_settings(&s),
            Err(CryptoInitError::JwtPrivateKey(_))
        ));
    }

    #[test]
    fn from_settings_rejects_bad_public_key() {
        let mut s = valid_settings();
        s.jwt_public_key = "not a pem key".to_string();
        assert!(matches!(
            Crypto::from_settings(&s),
            Err(CryptoInitError::JwtPublicKey(_))
        ));
    }

    #[test]
    fn decrypt_rejects_truncated_payload_as_malformed() {
        let crypto = Crypto::for_testing();
        // Valid prefix + valid base64 that decodes to fewer than NONCE_LEN bytes,
        // hitting the `payload.len() <= SECRET_NONCE_LEN` guard.
        let short = format!("{SECRET_CIPHER_PREFIX}{}", URL_SAFE_NO_PAD.encode([1u8; 4]));
        assert!(matches!(
            crypto.decrypt_secret(&short, b""),
            Err(SecretCipherError::Malformed)
        ));
    }

    #[test]
    fn decrypt_rejects_bad_base64_after_prefix_as_malformed() {
        let crypto = Crypto::for_testing();
        let bad = format!("{SECRET_CIPHER_PREFIX}!!!not base64!!!");
        assert!(matches!(
            crypto.decrypt_secret(&bad, b""),
            Err(SecretCipherError::Malformed)
        ));
    }

    #[test]
    fn crypto_accessors_expose_keys() {
        let crypto = Crypto::for_testing();
        // Exercise the const accessors so they are covered.
        let _ = crypto.argon2();
        let _ = crypto.encoding_key();
        let _ = crypto.decoding_key();
    }
}
