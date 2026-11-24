//! OAuth state JWT helpers for CSRF protection.

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

use jiff::{SignedDuration, Timestamp};
use rand::Rng;
use serde::{Deserialize, Serialize};

use super::error::AppError;
use application::dto::{OAuthMode, OAuthStateClaims};
use platform::crypto::{TokenKeyring, TokenType, token};

#[derive(Serialize, Deserialize)]
struct OAuthStatePayload {
    nonce: String,
    mode: OAuthMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_id: Option<String>,
    provider: String,
}

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
    mode: OAuthMode,
    user_id: Option<&str>,
) -> Result<(String, String), AppError> {
    let iat = Timestamp::now();
    let exp = iat + SignedDuration::from_mins(5);
    let nonce = generate_nonce();

    let payload = OAuthStatePayload {
        nonce: nonce.clone(),
        mode,
        user_id: user_id.map(String::from),
        provider: provider.to_string(),
    };

    let jwt = token::issue(keys, TokenType::OAuthState, &payload, iat, exp)
        .map_err(|e| AppError::Internal(format!("Failed to create OAuth state: {e}")))?;

    Ok((jwt, nonce))
}

/// Verify and decode an OAuth state JWT.
///
/// # Errors
///
/// Returns `AppError::Validation` if the JWT is invalid or expired.
pub fn verify_oauth_state(keys: &TokenKeyring, state: &str) -> Result<OAuthStateClaims, AppError> {
    let verified =
        token::verify::<OAuthStatePayload>(keys, state, TokenType::OAuthState).map_err(|err| {
            // The decoder's reason stays in the logs; clients get a fixed message.
            tracing::debug!(?err, "OAuth state rejected");
            AppError::Validation("Invalid OAuth state".into())
        })?;

    Ok(OAuthStateClaims {
        nonce: verified.payload.nonce,
        mode: verified.payload.mode,
        user_id: verified.payload.user_id,
        provider: verified.payload.provider,
        exp: verified.exp,
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

        let (state, nonce) =
            create_oauth_state(crypto.tokens(), "google", OAuthMode::Login, None).unwrap();
        let claims = verify_oauth_state(crypto.tokens(), &state).unwrap();

        assert_eq!(claims.provider, "google");
        assert_eq!(claims.mode, OAuthMode::Login);
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
        let (state, _) = create_oauth_state(&keys, "google", OAuthMode::Login, None).unwrap();
        let header = jsonwebtoken::decode_header(&state).unwrap();
        assert_eq!(header.alg, jsonwebtoken::Algorithm::HS512);
        assert_eq!(header.kid.as_deref(), Some("test-a"));
        // Small enough for a redirect URL.
        assert!(state.len() < 512, "state is {} bytes", state.len());
    }

    #[test]
    fn tampered_oauth_state_is_refused() {
        let keys = TokenKeyring::for_testing();
        let (state, _) = create_oauth_state(&keys, "google", OAuthMode::Link, Some("u-1")).unwrap();
        let mut parts: Vec<String> = state.split('.').map(str::to_owned).collect();
        let forged = OAuthStateClaims {
            nonce: "n".into(),
            mode: OAuthMode::Link,
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
        let past = Timestamp::now() - SignedDuration::from_mins(5);
        let iat = past - SignedDuration::from_mins(5);
        let expired = OAuthStatePayload {
            nonce: "n".into(),
            mode: OAuthMode::Login,
            user_id: None,
            provider: "google".into(),
        };
        let state = token::issue(&keys, TokenType::OAuthState, &expired, iat, past).unwrap();
        assert_invalid_state(verify_oauth_state(&keys, &state));
    }

    #[test]
    fn oauth_state_from_an_untrusted_key_is_refused() {
        let other =
            TokenKeyring::from_config(&platform::crypto::keyring_test_entry("other", 9), None)
                .unwrap();
        let (state, _) = create_oauth_state(&other, "google", OAuthMode::Login, None).unwrap();
        assert_invalid_state(verify_oauth_state(&TokenKeyring::for_testing(), &state));
    }

    #[test]
    fn oauth_state_with_unknown_mode_fails_closed() {
        #[derive(Serialize)]
        struct UnknownModePayload {
            nonce: &'static str,
            mode: &'static str,
            provider: &'static str,
        }

        let keys = TokenKeyring::for_testing();
        let iat = Timestamp::now();
        let exp = iat + SignedDuration::from_mins(5);
        let state = token::issue(
            &keys,
            TokenType::OAuthState,
            &UnknownModePayload {
                nonce: "n",
                mode: "unexpected",
                provider: "google",
            },
            iat,
            exp,
        )
        .unwrap();
        assert_invalid_state(verify_oauth_state(&keys, &state));
    }

    #[test]
    fn oauth_state_rejects_foreign_audience() {
        #[derive(Serialize)]
        struct ForeignAudState {
            nonce: &'static str,
            mode: &'static str,
            provider: &'static str,
            typ: &'static str,
            iss: &'static str,
            aud: &'static str,
            iat: i64,
            exp: i64,
        }

        let keys = TokenKeyring::for_testing();
        let now = Timestamp::now();
        let state = keys
            .sign(&ForeignAudState {
                nonce: "n",
                mode: "login",
                provider: "google",
                typ: TokenType::OAuthState.as_str(),
                iss: platform::crypto::TOKEN_ISSUER,
                aud: "other-audience",
                iat: now.as_second(),
                exp: (now + SignedDuration::from_mins(5)).as_second(),
            })
            .unwrap();
        assert_invalid_state(verify_oauth_state(&keys, &state));
    }
}
