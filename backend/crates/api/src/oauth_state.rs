//! OAuth state JWT helpers for CSRF protection.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use std::sync::LazyLock;

use jiff::{SignedDuration, Timestamp};
use jsonwebtoken::Validation;
use rand::Rng;

use super::error::AppError;
use application::dto::OAuthStateClaims;
use platform::crypto::TokenKeyring;

/// The OAuth-state validation, built once.
static STATE_VALIDATION: LazyLock<Validation> = LazyLock::new(|| {
    let mut validation = TokenKeyring::validation();
    validation.set_required_spec_claims(&["exp"]);
    validation
});

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
    keys: &TokenKeyring,
    provider: &str,
    mode: &str,
    user_id: Option<&str>,
) -> Result<(String, String), AppError> {
    let exp = Timestamp::now() + SignedDuration::from_mins(5);
    let nonce = generate_nonce();

    let claims = OAuthStateClaims {
        nonce: nonce.clone(),
        mode: mode.to_string(),
        user_id: user_id.map(String::from),
        provider: provider.to_string(),
        exp: exp.as_second(),
    };

    let token = keys
        .sign(&claims)
        .map_err(|e| AppError::Internal(format!("Failed to create OAuth state: {e}")))?;

    Ok((token, nonce))
}

/// Verify and decode an OAuth state JWT.
///
/// # Errors
///
/// Returns `AppError::Validation` if the JWT is invalid or expired.
pub fn verify_oauth_state(keys: &TokenKeyring, state: &str) -> Result<OAuthStateClaims, AppError> {
    keys.verify::<OAuthStateClaims>(state, &STATE_VALIDATION)
        .map_err(|e| {
            // The decoder's reason stays in the logs; clients get a fixed message.
            tracing::debug!(reason = ?e.kind(), "OAuth state rejected");
            AppError::Validation("Invalid OAuth state".into())
        })
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
        let crypto = platform::crypto::Crypto::for_testing();

        let (state, nonce) = create_oauth_state(crypto.tokens(), "google", "login", None).unwrap();
        let claims = verify_oauth_state(crypto.tokens(), &state).unwrap();

        assert_eq!(claims.provider, "google");
        assert_eq!(claims.mode, "login");
        assert!(claims.user_id.is_none());
        // The returned nonce matches the one embedded in the signed state.
        assert_eq!(claims.nonce, nonce);
    }

    fn assert_invalid_state(result: Result<OAuthStateClaims, AppError>) {
        match result {
            Err(AppError::Validation(msg)) => assert_eq!(msg, "Invalid OAuth state"),
            other => panic!("expected Invalid OAuth state, got {other:?}"),
        }
    }

    #[test]
    fn oauth_state_is_hs512_and_names_the_active_key() {
        let keys = TokenKeyring::for_testing();
        let (state, _) = create_oauth_state(&keys, "google", "login", None).unwrap();
        let header = jsonwebtoken::decode_header(&state).unwrap();
        assert_eq!(header.alg, jsonwebtoken::Algorithm::HS512);
        assert_eq!(header.kid.as_deref(), Some("test-a"));
        // Small enough for a redirect URL.
        assert!(state.len() < 512, "state is {} bytes", state.len());
    }

    #[test]
    fn tampered_oauth_state_is_refused() {
        let keys = TokenKeyring::for_testing();
        let (state, _) = create_oauth_state(&keys, "google", "link", Some("u-1")).unwrap();
        let mut parts: Vec<String> = state.split('.').map(str::to_owned).collect();
        let forged = OAuthStateClaims {
            nonce: "n".into(),
            mode: "link".into(),
            user_id: Some("u-2".into()),
            provider: "google".into(),
            exp: (Timestamp::now() + SignedDuration::from_mins(5)).as_second(),
        };
        parts[1] = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&forged).unwrap());
        assert_invalid_state(verify_oauth_state(&keys, &parts.join(".")));
    }

    #[test]
    fn expired_oauth_state_is_refused() {
        let keys = TokenKeyring::for_testing();
        let expired = OAuthStateClaims {
            nonce: "n".into(),
            mode: "login".into(),
            user_id: None,
            provider: "google".into(),
            exp: (Timestamp::now() - SignedDuration::from_mins(5)).as_second(),
        };
        let state = keys.sign(&expired).unwrap();
        assert_invalid_state(verify_oauth_state(&keys, &state));
    }

    #[test]
    fn oauth_state_from_an_untrusted_key_is_refused() {
        let other =
            TokenKeyring::from_config(&platform::crypto::keyring_test_entry("other", 9), None)
                .unwrap();
        let (state, _) = create_oauth_state(&other, "google", "login", None).unwrap();
        assert_invalid_state(verify_oauth_state(&TokenKeyring::for_testing(), &state));
    }
}
