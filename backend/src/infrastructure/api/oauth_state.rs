//! OAuth state JWT helpers for CSRF protection.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
use rand::Rng;

use super::error::AppError;
use crate::application::dto::OAuthStateClaims;

fn generate_nonce() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Create an OAuth state JWT (5 min expiry).
///
/// Returns the signed state token together with its `nonce`. The caller stores
/// the nonce in an `HttpOnly` cookie at `oauth_start` and compares it at the
/// callback to bind the flow to the originating browser.
///
/// # Errors
///
/// Returns `AppError::Internal` if JWT encoding fails.
pub fn create_oauth_state(
    encoding_key: &EncodingKey,
    provider: &str,
    mode: &str,
    user_id: Option<&str>,
) -> Result<(String, String), AppError> {
    let exp = Utc::now() + Duration::minutes(5);
    let nonce = generate_nonce();

    let claims = OAuthStateClaims {
        nonce: nonce.clone(),
        mode: mode.to_string(),
        user_id: user_id.map(String::from),
        provider: provider.to_string(),
        exp: exp.timestamp(),
    };

    let token = encode(&Header::new(Algorithm::RS256), &claims, encoding_key)
        .map_err(|e| AppError::Internal(format!("Failed to create OAuth state: {e}")))?;

    Ok((token, nonce))
}

/// Verify and decode an OAuth state JWT.
///
/// # Errors
///
/// Returns `AppError::Validation` if the JWT is invalid or expired.
pub fn verify_oauth_state(
    decoding_key: &DecodingKey,
    state: &str,
) -> Result<OAuthStateClaims, AppError> {
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_required_spec_claims(&["exp"]);

    decode::<OAuthStateClaims>(state, decoding_key, &validation)
        .map(|data| data.claims)
        .map_err(|e| AppError::Validation(format!("Invalid OAuth state: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_nonce_is_unique() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_ne!(nonce1, nonce2);
        assert_eq!(nonce1.len(), 22);
    }

    #[test]
    fn oauth_state_does_not_carry_device_fingerprint() {
        let crypto = crate::shared::crypto::Crypto::for_testing();

        let (state, nonce) =
            create_oauth_state(crypto.encoding_key(), "google", "login", None).unwrap();
        let claims = verify_oauth_state(crypto.decoding_key(), &state).unwrap();

        assert_eq!(claims.provider, "google");
        assert_eq!(claims.mode, "login");
        assert!(claims.user_id.is_none());
        // The returned nonce matches the one embedded in the signed state.
        assert_eq!(claims.nonce, nonce);
    }
}
