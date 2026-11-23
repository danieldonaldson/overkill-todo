//! Session token creation and verification (HMAC-SHA-512 via the token keyring).

use super::error::AppError;
use jiff::{SignedDuration, Timestamp};
use jsonwebtoken::Validation;
use jsonwebtoken::errors::ErrorKind;
use std::sync::LazyLock;

use platform::crypto::TokenKeyring;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Token-type marker for full session tokens.
///
/// INVARIANT: every session JWT carries `typ == SESSION_TOKEN_TYPE` and
/// [`verify_jwt`] rejects anything else. Signup/reset tokens are signed with the
/// *same* RSA key but carry a different `typ` (`"signup"`/`"reset"`); without this
/// check they would be accepted as logged-in sessions (token-type confusion).
pub const SESSION_TOKEN_TYPE: &str = "session";

/// Issuer (`iss`) asserted on every session JWT and required by [`verify_jwt`].
///
/// INVARIANT: combined with [`TOKEN_AUDIENCE`], this scopes a token to *this*
/// service. A token minted by (or for) another service — even one signed with a
/// shared key — carries a different `iss`/`aud` and is rejected, closing the
/// cross-service token-reuse gap.
pub const TOKEN_ISSUER: &str = "overkill-todo";

/// Audience (`aud`) asserted on every session JWT and required by [`verify_jwt`].
/// See [`TOKEN_ISSUER`].
pub const TOKEN_AUDIENCE: &str = "overkill-todo-api";

/// JWT claims payload.
///
/// Contains the user ID, email verification status, role, and token timestamps.
/// Serialized into the JWT body.
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID as string).
    pub sub: String,
    /// Token type — always [`SESSION_TOKEN_TYPE`]; enforced by [`verify_jwt`].
    pub typ: String,
    /// Issuer — always [`TOKEN_ISSUER`]; enforced by [`verify_jwt`].
    pub iss: String,
    /// Audience — always [`TOKEN_AUDIENCE`]; enforced by [`verify_jwt`].
    pub aud: String,
    /// Whether the user's email is verified.
    #[serde(default)]
    pub verified: bool,
    /// User role (user, admin, `super_admin`).
    #[serde(default = "default_role")]
    pub role: String,
    /// Expiration time (Unix timestamp).
    pub exp: i64,
    /// Issued at (Unix timestamp).
    pub iat: i64,
}

/// Default role for backwards compatibility with tokens that don't have a role.
fn default_role() -> String {
    "user".to_string()
}

impl Claims {
    /// Creates new claims for the given user with an expiration time in minutes.
    #[must_use]
    pub fn new(user_id: Uuid, email_verified: bool, role: &str, expiration_minutes: i64) -> Self {
        let now = Timestamp::now();
        let exp = now + SignedDuration::from_mins(expiration_minutes);

        Self {
            sub: user_id.to_string(),
            typ: SESSION_TOKEN_TYPE.to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            verified: email_verified,
            role: role.to_string(),
            exp: exp.as_second(),
            iat: now.as_second(),
        }
    }

    /// Extracts the user ID from the claims.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Authorization`] if the subject is not a valid UUID.
    pub fn user_id(&self) -> Result<Uuid, AppError> {
        Uuid::parse_str(&self.sub).map_err(|_| {
            tracing::debug!("session token rejected: subject is not a UUID");
            AppError::token_rejected()
        })
    }
}

/// The session-token validation, built once: HS512 only, issuer and audience
/// pinned so a token scoped to another service (or to none) is refused.
static SESSION_VALIDATION: LazyLock<Validation> = LazyLock::new(|| {
    let mut validation = TokenKeyring::validation();
    validation.set_issuer(&[TOKEN_ISSUER]);
    validation.set_audience(&[TOKEN_AUDIENCE]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);
    validation
});

/// Creates a signed session token for the given user.
///
/// Signed with the keyring's active key (HMAC-SHA-512, `kid` header).
///
/// # Arguments
///
/// * `user_id` - The user's UUID to encode in the token
/// * `email_verified` - Whether the user's email is verified
/// * `role` - The user's role (e.g., "user", "admin", "`super_admin`")
/// * `keys` - The token keyring
/// * `expiration_minutes` - Token validity duration in minutes
///
/// # Errors
/// Returns an error if JWT encoding fails.
pub fn create_jwt(
    user_id: Uuid,
    email_verified: bool,
    role: &str,
    keys: &TokenKeyring,
    expiration_minutes: i64,
) -> Result<String, AppError> {
    let claims = Claims::new(user_id, email_verified, role, expiration_minutes);

    keys.sign(&claims)
        .map_err(|e| AppError::Internal(format!("Failed to create JWT: {e}")))
}

/// Verifies and decodes a session token.
///
/// # Errors
///
/// Returns [`AppError::Authorization`]: `token_expired` if the token is valid
/// but expired, otherwise `token_invalid` with one fixed message (wrong
/// algorithm, unknown or missing key, bad signature, wrong issuer, audience or
/// type, or malformed).
pub fn verify_jwt(token: &str, keys: &TokenKeyring) -> Result<Claims, AppError> {
    let claims: Claims = keys.verify(token, &SESSION_VALIDATION).map_err(|e| {
        if *e.kind() == ErrorKind::ExpiredSignature {
            AppError::token_expired()
        } else {
            // The reason stays in debug logs; the token itself is never logged.
            tracing::debug!(reason = ?e.kind(), "session token rejected");
            AppError::token_rejected()
        }
    })?;

    // INVARIANT: reject single-purpose tokens (signup/reset) presented as a
    // session. They are signed with the same key but carry a different `typ`.
    if claims.typ != SESSION_TOKEN_TYPE {
        tracing::debug!("session token rejected: not a session token");
        return Err(AppError::token_rejected());
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::api::error::auth_codes;
    use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
    use platform::crypto::TEST_RSA_PRIVATE_KEY;
    use proptest::prelude::*;

    fn keys() -> TokenKeyring {
        TokenKeyring::for_testing()
    }

    /// Assert `result` is the generic invalid-token rejection.
    fn assert_rejected(result: Result<Claims, AppError>) {
        match result {
            Err(AppError::Authorization { code, message }) => {
                assert_eq!(code, auth_codes::TOKEN_INVALID);
                assert_eq!(message, "Invalid token");
            }
            other => panic!("expected the generic TOKEN_INVALID rejection, got {other:?}"),
        }
    }

    #[test]
    fn test_create_and_verify_jwt() {
        let user_id = Uuid::now_v7();
        let token = create_jwt(user_id, true, "user", &keys(), 15).expect("creates");
        let claims = verify_jwt(&token, &keys()).expect("verifies");

        assert_eq!(claims.user_id().unwrap(), user_id);
        assert!(claims.verified);
        assert_eq!(claims.role, "user");
    }

    #[test]
    fn test_session_token_names_hs512_and_the_active_key() {
        let token = create_jwt(Uuid::now_v7(), true, "user", &keys(), 15).expect("creates");
        let header = jsonwebtoken::decode_header(&token).unwrap();
        assert_eq!(header.alg, Algorithm::HS512);
        assert_eq!(header.kid.as_deref(), Some("test-a"));
    }

    #[test]
    fn test_create_and_verify_jwt_unverified() {
        let user_id = Uuid::now_v7();
        let token = create_jwt(user_id, false, "user", &keys(), 15).expect("creates");
        let claims = verify_jwt(&token, &keys()).expect("verifies");

        assert_eq!(claims.user_id().unwrap(), user_id);
        assert!(!claims.verified);
    }

    #[test]
    fn test_create_and_verify_jwt_with_admin_role() {
        let user_id = Uuid::now_v7();
        let token = create_jwt(user_id, true, "admin", &keys(), 15).expect("creates");
        let claims = verify_jwt(&token, &keys()).expect("verifies");

        assert_eq!(claims.user_id().unwrap(), user_id);
        assert_eq!(claims.role, "admin");
    }

    /// SECURITY: a token signed by a key the verifier doesn't trust is refused.
    #[test]
    fn test_token_from_an_untrusted_key_rejected() {
        let other =
            TokenKeyring::from_config(&platform::crypto::keyring_test_entry("other", 9), None)
                .unwrap();
        let token = create_jwt(Uuid::now_v7(), true, "user", &other, 15).expect("creates");
        assert_rejected(verify_jwt(&token, &keys()));
    }

    /// SECURITY: an RS256 session token signed by the old RSA key, naming a
    /// trusted `kid`, is refused.
    #[test]
    fn test_rs256_token_rejected() {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test-a".into());
        let rsa = EncodingKey::from_rsa_pem(TEST_RSA_PRIVATE_KEY.as_bytes()).unwrap();
        let claims = Claims::new(Uuid::now_v7(), true, "super_admin", 15);
        let token = encode(&header, &claims, &rsa).expect("encode failed");
        assert_rejected(verify_jwt(&token, &keys()));
    }

    /// SECURITY: an HS256 token under the right key is refused.
    #[test]
    fn test_verify_jwt_wrong_algorithm() {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test-a".into());
        let claims = Claims::new(Uuid::now_v7(), true, "user", 15);
        let token =
            encode(&header, &claims, &EncodingKey::from_secret(&[1u8; 64])).expect("encode failed");
        assert_rejected(verify_jwt(&token, &keys()));
    }

    /// SECURITY: a validly-signed session token whose `iss` is not
    /// [`TOKEN_ISSUER`] must be rejected (cross-service token reuse).
    #[test]
    fn test_wrong_issuer_rejected() {
        let mut claims = Claims::new(Uuid::now_v7(), true, "user", 15);
        claims.iss = "some-other-service".to_string();
        let token = keys().sign(&claims).unwrap();
        assert_rejected(verify_jwt(&token, &keys()));
    }

    /// SECURITY: a validly-signed session token whose `aud` is not
    /// [`TOKEN_AUDIENCE`] must be rejected (token minted for another audience).
    #[test]
    fn test_wrong_audience_rejected() {
        let mut claims = Claims::new(Uuid::now_v7(), true, "user", 15);
        claims.aud = "some-other-audience".to_string();
        let token = keys().sign(&claims).unwrap();
        assert_rejected(verify_jwt(&token, &keys()));
    }

    #[test]
    fn test_claims_new() {
        let user_id = Uuid::now_v7();
        let claims = Claims::new(user_id, true, "admin", 30);

        assert_eq!(claims.sub, user_id.to_string());
        assert!(claims.verified);
        assert_eq!(claims.role, "admin");
        assert!(claims.exp > claims.iat);
        // exp should be approximately 30 minutes after iat
        let diff = claims.exp - claims.iat;
        assert!((30 * 60 - 1..=30 * 60 + 1).contains(&diff));
    }

    #[test]
    fn test_claims_user_id_valid() {
        let user_id = Uuid::now_v7();
        let claims = Claims::new(user_id, true, "user", 15);

        let extracted = claims.user_id().expect("Should extract user_id");
        assert_eq!(extracted, user_id);
    }

    #[test]
    fn test_claims_user_id_invalid() {
        let claims = Claims {
            sub: "not-a-uuid".to_string(),
            typ: SESSION_TOKEN_TYPE.to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            verified: false,
            role: "user".to_string(),
            exp: 0,
            iat: 0,
        };

        let result = claims.user_id();
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Authorization { message, .. } => assert_eq!(message, "Invalid token"),
            _ => panic!("Expected Authorization error"),
        }
    }

    /// A single-purpose token (signup/reset) signed with the same key but a
    /// different `typ` must be rejected by the session validator.
    #[test]
    fn test_non_session_typ_rejected() {
        #[derive(serde::Serialize)]
        struct ForeignClaims {
            sub: String,
            typ: String,
            iss: String,
            aud: String,
            exp: i64,
            iat: i64,
        }

        let now = Timestamp::now();
        let claims = ForeignClaims {
            sub: Uuid::now_v7().to_string(),
            typ: "signup".to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            exp: (now + SignedDuration::from_mins(15)).as_second(),
            iat: now.as_second(),
        };
        let token = keys().sign(&claims).unwrap();
        assert_rejected(verify_jwt(&token, &keys()));
    }

    #[test]
    fn test_verify_jwt_invalid_token() {
        for token in ["invalid.token.here", "only-one-segment", ""] {
            assert_rejected(verify_jwt(token, &keys()));
        }
    }

    #[test]
    fn test_verify_jwt_expired_token() {
        // Expired 5 minutes ago, beyond the 60 s leeway.
        let now = jiff::Timestamp::now();
        let claims = Claims {
            sub: Uuid::now_v7().to_string(),
            typ: SESSION_TOKEN_TYPE.to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            verified: true,
            role: "user".to_string(),
            exp: (now - jiff::SignedDuration::from_mins(5)).as_second(),
            iat: (now - jiff::SignedDuration::from_mins(20)).as_second(),
        };
        let token = keys().sign(&claims).unwrap();

        match verify_jwt(&token, &keys()).unwrap_err() {
            AppError::Authorization { code, .. } => assert_eq!(code, auth_codes::TOKEN_EXPIRED),
            other => panic!("Expected Authorization error, got {other:?}"),
        }
    }

    // ==================== Property Tests ====================

    /// Strategy to generate valid UUID v4 strings.
    fn uuid_v4_strategy() -> impl Strategy<Value = String> {
        "[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"
    }

    proptest! {
        /// Valid UUID strings in sub should parse successfully.
        #[test]
        fn prop_valid_uuid_parses(uuid_str in uuid_v4_strategy()) {
            let claims = Claims {
                sub: uuid_str.clone(),
                typ: SESSION_TOKEN_TYPE.to_string(),
                iss: TOKEN_ISSUER.to_string(),
                aud: TOKEN_AUDIENCE.to_string(),
                verified: false,
                role: "user".to_string(),
                exp: 0,
                iat: 0,
            };
            let result = claims.user_id();
            prop_assert!(result.is_ok());
            let parsed = result.unwrap();
            prop_assert_eq!(parsed.to_string(), uuid_str);
        }

        /// Invalid UUID strings should return Authorization error.
        #[test]
        fn prop_invalid_uuid_fails(invalid_sub in "[a-zA-Z0-9]{1,36}") {
            // Skip if it accidentally generates a valid UUID
            prop_assume!(Uuid::parse_str(&invalid_sub).is_err());
            let claims = Claims {
                sub: invalid_sub,
                typ: SESSION_TOKEN_TYPE.to_string(),
                iss: TOKEN_ISSUER.to_string(),
                aud: TOKEN_AUDIENCE.to_string(),
                verified: false,
                role: "user".to_string(),
                exp: 0,
                iat: 0,
            };
            let result = claims.user_id();
            prop_assert!(result.is_err());
            let is_auth_err = matches!(result, Err(AppError::Authorization { .. }));
            prop_assert!(is_auth_err);
        }

        /// Claims::new always produces valid exp > iat.
        #[test]
        fn prop_claims_new_timestamps_valid(expiration_minutes in 1i64..10000) {
            let user_id = Uuid::now_v7();
            let claims = Claims::new(user_id, true, "user", expiration_minutes);
            prop_assert!(claims.exp > claims.iat);
            // exp should be approximately expiration_minutes * 60 seconds after iat
            let diff = claims.exp - claims.iat;
            let expected = expiration_minutes * 60;
            prop_assert!(diff >= expected - 1 && diff <= expected + 1);
        }

        /// Claims::new preserves the user_id.
        #[test]
        fn prop_claims_new_preserves_user_id(_seed in 0u64..1000) {
            let user_id = Uuid::now_v7();
            let claims = Claims::new(user_id, true, "user", 15);
            let extracted = claims.user_id().unwrap();
            prop_assert_eq!(extracted, user_id);
        }

        /// Claims::new preserves verified flag.
        #[test]
        fn prop_claims_new_preserves_verified(verified in proptest::bool::ANY) {
            let claims = Claims::new(Uuid::now_v7(), verified, "user", 15);
            prop_assert_eq!(claims.verified, verified);
        }

        /// Empty string is not a valid UUID.
        #[test]
        fn prop_empty_string_not_uuid(_seed in 0u64..1) {
            let claims = Claims {
                sub: String::new(),
                typ: SESSION_TOKEN_TYPE.to_string(),
                iss: TOKEN_ISSUER.to_string(),
                aud: TOKEN_AUDIENCE.to_string(),
                verified: false,
                role: "user".to_string(),
                exp: 0,
                iat: 0,
            };
            prop_assert!(claims.user_id().is_err());
        }
    }

    /// `default_role` supplies "user" when a token body omits the `role` claim,
    /// preserving backwards compatibility with older tokens.
    #[test]
    fn test_default_role_applied_when_role_claim_absent() {
        #[derive(serde::Serialize)]
        struct RolelessClaims {
            sub: String,
            typ: String,
            iss: String,
            aud: String,
            verified: bool,
            exp: i64,
            iat: i64,
        }

        let now = Timestamp::now();
        let claims = RolelessClaims {
            sub: Uuid::now_v7().to_string(),
            typ: SESSION_TOKEN_TYPE.to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            verified: true,
            exp: (now + SignedDuration::from_mins(15)).as_second(),
            iat: now.as_second(),
        };
        let token = keys().sign(&claims).unwrap();

        let verified = verify_jwt(&token, &keys()).expect("should verify");
        assert_eq!(verified.role, "user");
    }
}
