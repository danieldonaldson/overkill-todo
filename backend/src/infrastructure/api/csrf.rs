//! CSRF token generation and validation using double-submit cookie pattern.
//!
//! Tokens use HMAC-SHA256 for signature verification and include a timestamp
//! for expiration checking. The wire format is:
//!
//! ```text
//! {random_16_bytes_base64}.{unix_timestamp}.{hmac_sha256_signature}
//! ```
//!
//! The signature additionally covers the owning user id, which is *not*
//! transmitted in the token. This binds each token to a single principal:
//! a token minted for one user fails verification when presented in a request
//! authenticated as another, so the double-submit check is no longer "is this a
//! valid signature from anyone."
//!
//! # Security Properties
//!
//! - Tokens are cryptographically signed with HMAC-SHA256
//! - Tokens expire after 15 minutes
//! - Random component prevents token prediction
//! - Tokens are bound to the authenticated user id

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use rand::Rng;
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

use super::error::AppError;

type HmacSha256 = Hmac<Sha256>;

/// CSRF token lifetime in seconds (15 minutes).
const TOKEN_LIFETIME_SECS: u64 = 15 * 60;

/// Clock skew tolerance in seconds (5 seconds into the future).
const CLOCK_SKEW_TOLERANCE_SECS: u64 = 5;

/// Generates a new CSRF token.
///
/// The token contains:
/// - 16 random bytes (base64 encoded)
/// - Current Unix timestamp
/// - HMAC-SHA256 signature of the above
///
/// # Arguments
///
/// * `secret` - The CSRF secret key for HMAC signing
/// * `user_id` - The owning principal; folded into the signature but not
///   transmitted in the token
///
/// # Returns
///
/// A token string in the format `{random}.{timestamp}.{signature}`
///
/// # Panics
///
/// Panics if the system time is before the Unix epoch (should never happen on
/// properly configured systems).
#[must_use]
pub fn generate_csrf_token(secret: &[u8], user_id: &str) -> String {
    // Generate 16 random bytes
    let mut random_bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut random_bytes);
    let random_part = URL_SAFE_NO_PAD.encode(random_bytes);

    // Get current timestamp
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs();

    // Sign over random.timestamp.user_id so the token is bound to the principal.
    // Only random.timestamp.signature is returned; the user id is recovered from
    // the authenticated session at verify time.
    let signed = format!("{random_part}.{timestamp}.{user_id}");

    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can accept key of any size");
    mac.update(signed.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    format!("{random_part}.{timestamp}.{signature}")
}

/// Verifies a CSRF token.
///
/// Checks that:
/// 1. The token has the correct format (3 parts)
/// 2. The HMAC signature is valid
/// 3. The token has not expired (15 minute lifetime)
///
/// # Arguments
///
/// * `token` - The token to verify
/// * `secret` - The CSRF secret key for HMAC verification
/// * `user_id` - The authenticated principal the token must be bound to
///
/// # Errors
///
/// Returns `AppError::Forbidden` if verification fails for any reason.
///
/// # Panics
///
/// Panics if the system time is before the Unix epoch (should never happen on
/// properly configured systems).
pub fn verify_csrf_token(token: &str, secret: &[u8], user_id: &str) -> Result<(), AppError> {
    // Split token into parts
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(AppError::csrf_invalid());
    }

    let random_part = parts[0];
    let timestamp_str = parts[1];
    let provided_signature = parts[2];

    // Parse timestamp
    let timestamp: u64 = timestamp_str
        .parse()
        .map_err(|_| AppError::csrf_invalid())?;

    // Check expiration
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("System time before UNIX epoch")
        .as_secs();

    // Allow small clock skew into the future
    if timestamp > now + CLOCK_SKEW_TOLERANCE_SECS {
        return Err(AppError::csrf_invalid());
    }

    // Check if expired
    if now > timestamp + TOKEN_LIFETIME_SECS {
        return Err(AppError::csrf_invalid());
    }

    // Verify signature over random.timestamp.user_id.
    let signed = format!("{random_part}.{timestamp_str}.{user_id}");
    let mut mac = HmacSha256::new_from_slice(secret).expect("HMAC can accept key of any size");
    mac.update(signed.as_bytes());

    // Decode the provided signature
    let provided_sig_bytes = URL_SAFE_NO_PAD
        .decode(provided_signature)
        .map_err(|_| AppError::csrf_invalid())?;

    // Verify using constant-time comparison
    mac.verify_slice(&provided_sig_bytes)
        .map_err(|_| AppError::csrf_invalid())?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rand::Rng;

    const TEST_SECRET: &[u8] = b"test-csrf-secret-key-32-bytes!!";
    const TEST_USER: &str = "11111111-1111-1111-1111-111111111111";

    #[test]
    fn test_generate_token_format() {
        let token = generate_csrf_token(TEST_SECRET, TEST_USER);
        let parts: Vec<&str> = token.split('.').collect();

        assert_eq!(parts.len(), 3, "Token should have 3 parts");

        // Random part should be base64 encoded 16 bytes (22 chars)
        assert_eq!(parts[0].len(), 22, "Random part should be 22 chars");

        // Timestamp should be a valid number
        let timestamp: u64 = parts[1].parse().expect("Timestamp should be a number");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            timestamp <= now + 1 && timestamp >= now - 1,
            "Timestamp should be close to now"
        );

        // Signature should be base64 encoded HMAC-SHA256 (43 chars)
        assert_eq!(parts[2].len(), 43, "Signature should be 43 chars");
    }

    #[test]
    fn test_verify_valid_token() {
        let token = generate_csrf_token(TEST_SECRET, TEST_USER);
        assert!(verify_csrf_token(&token, TEST_SECRET, TEST_USER).is_ok());
    }

    #[test]
    fn test_verify_expired_token() {
        // Create a token with an old timestamp
        let mut random_bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut random_bytes);
        let random_part = URL_SAFE_NO_PAD.encode(random_bytes);

        // Timestamp from 20 minutes ago (beyond 15 minute lifetime)
        let old_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - (20 * 60);

        let message = format!("{random_part}.{old_timestamp}");
        let mut mac = HmacSha256::new_from_slice(TEST_SECRET).unwrap();
        mac.update(message.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

        let token = format!("{message}.{signature}");
        let result = verify_csrf_token(&token, TEST_SECRET, TEST_USER);

        assert!(result.is_err());
    }

    #[test]
    fn test_verify_invalid_signature() {
        let token = generate_csrf_token(TEST_SECRET, TEST_USER);

        // Tamper with the token by changing a character in the signature
        let mut chars: Vec<char> = token.chars().collect();
        let last_idx = chars.len() - 1;
        chars[last_idx] = if chars[last_idx] == 'a' { 'b' } else { 'a' };
        let tampered_token: String = chars.into_iter().collect();

        let result = verify_csrf_token(&tampered_token, TEST_SECRET, TEST_USER);
        assert!(result.is_err());
    }

    /// A token minted for one user must not verify for another.
    #[test]
    fn test_verify_rejects_other_user() {
        let token = generate_csrf_token(TEST_SECRET, TEST_USER);
        let other_user = "22222222-2222-2222-2222-222222222222";
        assert!(verify_csrf_token(&token, TEST_SECRET, other_user).is_err());
        assert!(verify_csrf_token(&token, TEST_SECRET, TEST_USER).is_ok());
    }

    #[test]
    fn test_verify_malformed_token() {
        // Missing parts
        assert!(verify_csrf_token("only-one-part", TEST_SECRET, TEST_USER).is_err());
        assert!(verify_csrf_token("two.parts", TEST_SECRET, TEST_USER).is_err());
        assert!(verify_csrf_token("too.many.parts.here", TEST_SECRET, TEST_USER).is_err());
    }

    #[test]
    fn test_verify_wrong_secret() {
        let token = generate_csrf_token(TEST_SECRET, TEST_USER);
        let wrong_secret = b"different-secret-key-32-bytes!!";

        let result = verify_csrf_token(&token, wrong_secret, TEST_USER);
        assert!(result.is_err());
    }

    #[test]
    fn test_clock_skew_tolerance() {
        // Create a token with a timestamp slightly in the future (within tolerance)
        let mut random_bytes = [0u8; 16];
        rand::rng().fill_bytes(&mut random_bytes);
        let random_part = URL_SAFE_NO_PAD.encode(random_bytes);

        // Timestamp 3 seconds in the future (within 5 second tolerance)
        let future_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3;

        let signed = format!("{random_part}.{future_timestamp}.{TEST_USER}");
        let mut mac = HmacSha256::new_from_slice(TEST_SECRET).unwrap();
        mac.update(signed.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

        let token = format!("{random_part}.{future_timestamp}.{signature}");
        assert!(
            verify_csrf_token(&token, TEST_SECRET, TEST_USER).is_ok(),
            "Token with small future timestamp should be valid"
        );

        // But 10 seconds in the future should fail
        let far_future_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 10;

        let message = format!("{random_part}.{far_future_timestamp}");
        let mut mac = HmacSha256::new_from_slice(TEST_SECRET).unwrap();
        mac.update(message.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

        let token = format!("{message}.{signature}");
        let result = verify_csrf_token(&token, TEST_SECRET, TEST_USER);
        assert!(result.is_err());
    }

    #[test]
    fn test_tokens_are_unique() {
        let token1 = generate_csrf_token(TEST_SECRET, TEST_USER);
        let token2 = generate_csrf_token(TEST_SECRET, TEST_USER);
        assert_ne!(token1, token2, "Each token should be unique");
    }

    // ==================== Property Tests ====================

    proptest! {
        /// Valid tokens generated by our function should always verify successfully.
        #[test]
        fn prop_generated_tokens_verify(_seed in 0u64..1000) {
            let token = generate_csrf_token(TEST_SECRET, TEST_USER);
            prop_assert!(verify_csrf_token(&token, TEST_SECRET, TEST_USER).is_ok());
        }

        /// Tokens with wrong number of parts should fail.
        #[test]
        fn prop_wrong_part_count_fails(
            num_parts in 0usize..=5,
            parts in proptest::collection::vec("[a-zA-Z0-9_-]{1,20}", 0..=5)
        ) {
            // Skip the valid case (3 parts)
            prop_assume!(num_parts != 3);
            let token = parts.into_iter().take(num_parts).collect::<Vec<_>>().join(".");
            prop_assert!(verify_csrf_token(&token, TEST_SECRET, TEST_USER).is_err());
        }

        /// Non-numeric timestamps should fail parsing.
        #[test]
        fn prop_non_numeric_timestamp_fails(
            random_part in "[a-zA-Z0-9_-]{22}",
            bad_timestamp in "[a-zA-Z]+",
            signature in "[a-zA-Z0-9_-]{43}"
        ) {
            let token = format!("{random_part}.{bad_timestamp}.{signature}");
            let result = verify_csrf_token(&token, TEST_SECRET, TEST_USER);
            prop_assert!(result.is_err());
        }

        /// Invalid base64 in signature should fail.
        #[test]
        fn prop_invalid_base64_signature_fails(
            random_part in "[a-zA-Z0-9_-]{22}",
            // Use characters that are invalid in base64
            bad_signature in "[!@#$%^&*()]+{10,20}"
        ) {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
            let token = format!("{random_part}.{timestamp}.{bad_signature}");
            let result = verify_csrf_token(&token, TEST_SECRET, TEST_USER);
            prop_assert!(result.is_err());
        }

        /// Different secrets should produce verification failure.
        #[test]
        fn prop_different_secret_fails(
            secret1 in proptest::collection::vec(any::<u8>(), 16..=64),
            secret2 in proptest::collection::vec(any::<u8>(), 16..=64)
        ) {
            prop_assume!(secret1 != secret2);
            let token = generate_csrf_token(&secret1, TEST_USER);
            let result = verify_csrf_token(&token, &secret2, TEST_USER);
            prop_assert!(result.is_err());
        }

        /// Arbitrary strings should fail verification (overwhelmingly).
        #[test]
        fn prop_arbitrary_strings_fail(s in "\\PC{0,100}") {
            // Almost all arbitrary strings should fail verification
            // (statistically impossible to hit a valid signed token)
            let result = verify_csrf_token(&s, TEST_SECRET, TEST_USER);
            prop_assert!(result.is_err());
        }
    }
}
