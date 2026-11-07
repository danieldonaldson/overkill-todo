//! JWT token creation and verification using RS256.

use super::error::AppError;
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, EncodingKey, Header, Validation, decode, encode};
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
pub const TOKEN_ISSUER: &str = "app";

/// Audience (`aud`) asserted on every session JWT and required by [`verify_jwt`].
/// See [`TOKEN_ISSUER`].
pub const TOKEN_AUDIENCE: &str = "app-api";

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
        let now = Utc::now();
        let exp = now + Duration::minutes(expiration_minutes);

        Self {
            sub: user_id.to_string(),
            typ: SESSION_TOKEN_TYPE.to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            verified: email_verified,
            role: role.to_string(),
            exp: exp.timestamp(),
            iat: now.timestamp(),
        }
    }

    /// Extracts the user ID from the claims.
    ///
    /// # Errors
    ///
    /// Returns [`AppError::Authorization`] if the subject is not a valid UUID.
    pub fn user_id(&self) -> Result<Uuid, AppError> {
        Uuid::parse_str(&self.sub).map_err(|_| AppError::token_invalid("Invalid user ID in token"))
    }
}

/// Creates a signed JWT token for the given user.
///
/// Uses RS256 algorithm for signing.
///
/// # Arguments
///
/// * `user_id` - The user's UUID to encode in the token
/// * `email_verified` - Whether the user's email is verified
/// * `role` - The user's role (e.g., "user", "admin", "`super_admin`")
/// * `encoding_key` - RSA private key for signing
/// * `expiration_minutes` - Token validity duration in minutes
///
/// # Errors
/// Returns an error if JWT encoding fails.
pub fn create_jwt(
    user_id: Uuid,
    email_verified: bool,
    role: &str,
    encoding_key: &EncodingKey,
    expiration_minutes: i64,
) -> Result<String, AppError> {
    let claims = Claims::new(user_id, email_verified, role, expiration_minutes);

    encode(&Header::new(Algorithm::RS256), &claims, encoding_key)
        .map_err(|e| AppError::Internal(format!("Failed to create JWT: {e}")))
}

/// Verifies and decodes a JWT token.
///
/// # Errors
///
/// Returns [`AppError::Authorization`] if:
/// - The token has expired
/// - The token is malformed
/// - The signature is invalid
pub fn verify_jwt(token: &str, decoding_key: &DecodingKey) -> Result<Claims, AppError> {
    // Require and pin the issuer/audience so a token scoped to another service
    // (or with no scope at all) is rejected, not just an expired/forged one.
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[TOKEN_ISSUER]);
    validation.set_audience(&[TOKEN_AUDIENCE]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);

    let claims = decode::<Claims>(token, decoding_key, &validation)
        .map(|data| data.claims)
        .map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => AppError::token_expired(),
            jsonwebtoken::errors::ErrorKind::InvalidToken => AppError::token_invalid("malformed"),
            jsonwebtoken::errors::ErrorKind::InvalidAlgorithm => {
                AppError::token_invalid("algorithm mismatch")
            }
            jsonwebtoken::errors::ErrorKind::InvalidIssuer => {
                AppError::token_invalid("issuer mismatch")
            }
            jsonwebtoken::errors::ErrorKind::InvalidAudience => {
                AppError::token_invalid("audience mismatch")
            }
            _ => AppError::token_invalid(&format!("validation failed: {e}")),
        })?;

    // INVARIANT: reject single-purpose tokens (signup/reset) presented as a
    // session. They are signed with the same key but carry a different `typ`.
    if claims.typ != SESSION_TOKEN_TYPE {
        return Err(AppError::token_invalid("not a session token"));
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const TEST_PRIVATE_KEY: &str = r"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDy6CQ7n1LQQVzD
uFKBXnNBBth+WFGqo9X1x8zcpEIWee8lybAqhzS8pgn1nfD5JTjbCSYaBnXs+G5Y
uhsybBGVxlBAYKEdCvE3D0FN/5/APHREaKdELzwcC0knurncB91Nwc6rsQPo5GpZ
mRSXfA9IqZu8hq39JY1+fKwfDnK4VXzc2kSyguQP8vS07M2IIBoYAlIzWZmLQbCe
2e2iQNkuZk91MYCDXRnWOilzBjEeFjpDc9dUjOBUFWU3LRTT9MI36TaFY8R/S00i
Dn/HLUhrlxAlX1I0/C6378iQX1TTfvprvSY5k1psgc4DHev/lpW+YQEWRqOatUjW
Li2KFw2HAgMBAAECggEAEneYrZm+y4t0mIKubxx1qbdyBSUEzHLZRO7UQeAREDFj
/5z2Uj1mQoNjFXPVCrQMPwbSiT79fy8iAIfhonM54QerOUG/bfxTe2qi/eGVzSEr
e7nzIYlvyfgHr0gQckx8xDkcU8xI7k9TUI2FVcD+TnlqmCorFyhgOgITeeb4zM+5
HBxO+tzbjRr/XBhGyaUTJGGVyMMTojPpmODcYzrp/Xn6NONnO5PnsjRil5ki76VH
rrOVrV0AOYPAKeeW1SHNbVbPv7LaMwTTSPhDZ150MrPLe509QrWQoEkRSqyjZeLE
6vbPf1y76NRJZYnLqs/gsCZ6tmDX5XhrNvDw/9BVrQKBgQD6ALih6RIqVgtt0mZi
pSJI1PoVNiewxEBeE+F2RAXdkoCxUdYcN6pAJ5/U9M2zxBDAnfgGkaT9Lngz6YdJ
rK0HbFr1Lxydn8wk38oJRdy9q3xejTNAaloo4pF9z3WGFpLCllsJDCCrpKZRFS/v
yv80lvmYxkjOJEYgv1f2CRoPfQKBgQD4u9fms8QySnWET6YZZv5KcYTtt8MjxdIx
FGNZLYG/S3ATs7qKf2ExlljvGIc6PC8twmWTAWZ0/N8AQNdPwN+N1JypqcpoKGCy
+B+1BxBJvzSa914YHShqSencgXZ7WOc+jWNtFwjTiipye4xaWntM6Kkah90+srZV
dvxyS4KoUwKBgCSKdHId6ezMNNFxfkuOzWsNzhvJlZWNqcbWaw7pNF5FapAAYSYw
sK692+u5eFN9JeGxdS4onY/v+dvEl2SaW5QJL2ZE447aa3E8UIB4eKI0bXI4ezRA
FzuAmjlkWE4oAH12konI/wF3iGOzkBUhe5nJM2Q0HzIaNTHWJdotTGTVAoGBAJxb
EHwAcwueDMNwghwjxyQqugHo6y0YlGmvxEfxuEb09+4ao9+d9Q4D0cHy7EaqQIoG
Um2Xir1tnAz/tZl3x8/TX+uzRP42q5q1KNFWMGH7ZOkpKKgF5Dr0XaL0avNcznhe
zqpruWuoxcgTbAX8LNgKxQpdb7GTC9Urp8N9PhPrAoGBALOc4pqxt5ku+9RT06ld
bgfp/q0QK2ctgxJwEHtfPuG40hyrUd+nJ6xnM/faaSNF1ja+K/D3+2mEZEYo7cAg
0zN9HT6B7HX/hYBxhGRVKDBg0OmSxiA96ti6tDQzS0xp9myeGGo97BtWZNSKwGnm
DUoBCPh9JHjajqhSI09lGqHH
-----END PRIVATE KEY-----";

    const TEST_PUBLIC_KEY: &str = r"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA8ugkO59S0EFcw7hSgV5z
QQbYflhRqqPV9cfM3KRCFnnvJcmwKoc0vKYJ9Z3w+SU42wkmGgZ17PhuWLobMmwR
lcZQQGChHQrxNw9BTf+fwDx0RGinRC88HAtJJ7q53AfdTcHOq7ED6ORqWZkUl3wP
SKmbvIat/SWNfnysHw5yuFV83NpEsoLkD/L0tOzNiCAaGAJSM1mZi0GwntntokDZ
LmZPdTGAg10Z1jopcwYxHhY6Q3PXVIzgVBVlNy0U0/TCN+k2hWPEf0tNIg5/xy1I
a5cQJV9SNPwut+/IkF9U0376a70mOZNabIHOAx3r/5aVvmEBFkajmrVI1i4tihcN
hwIDAQAB
-----END PUBLIC KEY-----";

    #[test]
    fn test_create_and_verify_jwt() {
        let user_id = Uuid::now_v7();
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let token =
            create_jwt(user_id, true, "user", &encoding_key, 15).expect("Failed to create token");
        let claims = verify_jwt(&token, &decoding_key).expect("Failed to verify token");

        assert_eq!(claims.user_id().unwrap(), user_id);
        assert!(claims.verified);
        assert_eq!(claims.role, "user");
    }

    #[test]
    fn test_create_and_verify_jwt_unverified() {
        let user_id = Uuid::now_v7();
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let token =
            create_jwt(user_id, false, "user", &encoding_key, 15).expect("Failed to create token");
        let claims = verify_jwt(&token, &decoding_key).expect("Failed to verify token");

        assert_eq!(claims.user_id().unwrap(), user_id);
        assert!(!claims.verified);
    }

    #[test]
    fn test_create_and_verify_jwt_with_admin_role() {
        let user_id = Uuid::now_v7();
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let token =
            create_jwt(user_id, true, "admin", &encoding_key, 15).expect("Failed to create token");
        let claims = verify_jwt(&token, &decoding_key).expect("Failed to verify token");

        assert_eq!(claims.user_id().unwrap(), user_id);
        assert!(claims.verified);
        assert_eq!(claims.role, "admin");
    }

    /// A second, unrelated RSA keypair used to prove signature forgery is
    /// rejected: a token signed with [`TEST_PRIVATE_KEY`] must NOT verify under
    /// this public key.
    const OTHER_PUBLIC_KEY: &str = r"-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEApVr60DF0XOHqRZxKmwH8
Mr7IDH59+6cR4KK1V/vQo0/c1IOudbXtecTLii18xo53/sK1lUXdg+vKtSDEv9jq
/R7YH7tJ6pX2bmokIuLPSLp0m9p+PNGDTX1PmJCAIFullxOZdvpLZ7/B0oQAqfHe
RiMm8fo15gitxp+qdyIESg8ucuE5H/4MllkT9ekK+geCKe+o64ugcCNzinDXjPGd
fmtpnnKgBHWB6H2uQ/s8cGpaDx7r8Ml/CsMd0r2icOxlbx/byAUEAfVrrk0oX34U
NvLSvVZCCdzbmzPemwT6rB4lNm8u5sHlupQ8opKL6Kcj8zXraejD/BUPIMGLRGXW
WwIDAQAB
-----END PUBLIC KEY-----";

    /// Verifying with the MATCHING public key succeeds (sanity baseline for the
    /// forgery test below). Previously this test was misleadingly named
    /// `test_wrong_public_key` while actually using the matching key.
    #[test]
    fn test_matching_public_key_succeeds() {
        let user_id = Uuid::now_v7();
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let token =
            create_jwt(user_id, true, "user", &encoding_key, 15).expect("Failed to create token");
        // Using matching keys should succeed
        let result = verify_jwt(&token, &decoding_key);

        assert!(result.is_ok());
    }

    /// SECURITY: a token signed with one RSA keypair MUST NOT verify under a
    /// different, unrelated public key. This is the real forgery / wrong-key
    /// rejection test (the old `test_wrong_public_key` did not exercise it).
    #[test]
    fn test_forged_signature_wrong_public_key_rejected() {
        let user_id = Uuid::now_v7();
        // Sign with our real private key...
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let token =
            create_jwt(user_id, true, "user", &encoding_key, 15).expect("Failed to create token");

        // ...but verify against an UNRELATED public key.
        let other_key =
            DecodingKey::from_rsa_pem(OTHER_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let result = verify_jwt(&token, &other_key);
        assert!(
            result.is_err(),
            "token signed with a different key must NOT verify"
        );
        assert!(
            matches!(result.unwrap_err(), AppError::Authorization { .. }),
            "wrong-key verification must map to an Authorization error"
        );
    }

    /// SECURITY: a validly-signed session token whose `iss` is not
    /// [`TOKEN_ISSUER`] must be rejected (cross-service token reuse).
    #[test]
    fn test_wrong_issuer_rejected() {
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let mut claims = Claims::new(Uuid::now_v7(), true, "user", 15);
        claims.iss = "some-other-service".to_string();
        let token =
            encode(&Header::new(Algorithm::RS256), &claims, &encoding_key).expect("encode failed");

        let result = verify_jwt(&token, &decoding_key);
        assert!(
            result.is_err(),
            "token with a foreign issuer must not verify"
        );
        assert!(matches!(
            result.unwrap_err(),
            AppError::Authorization { .. }
        ));
    }

    /// SECURITY: a validly-signed session token whose `aud` is not
    /// [`TOKEN_AUDIENCE`] must be rejected (token minted for another audience).
    #[test]
    fn test_wrong_audience_rejected() {
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let mut claims = Claims::new(Uuid::now_v7(), true, "user", 15);
        claims.aud = "some-other-audience".to_string();
        let token =
            encode(&Header::new(Algorithm::RS256), &claims, &encoding_key).expect("encode failed");

        let result = verify_jwt(&token, &decoding_key);
        assert!(
            result.is_err(),
            "token with a foreign audience must not verify"
        );
        assert!(matches!(
            result.unwrap_err(),
            AppError::Authorization { .. }
        ));
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
            AppError::Authorization { message, .. } => assert!(message.contains("Invalid user ID")),
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

        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let now = Utc::now();
        let claims = ForeignClaims {
            sub: Uuid::now_v7().to_string(),
            typ: "signup".to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            exp: (now + Duration::minutes(15)).timestamp(),
            iat: now.timestamp(),
        };
        let token =
            encode(&Header::new(Algorithm::RS256), &claims, &encoding_key).expect("encode failed");

        let result = verify_jwt(&token, &decoding_key);
        assert!(
            result.is_err(),
            "signup-typ token must not verify as session"
        );
    }

    #[test]
    fn test_verify_jwt_invalid_token() {
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let result = verify_jwt("invalid.token.here", &decoding_key);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AppError::Authorization { .. }
        ));
    }

    #[test]
    fn test_verify_jwt_expired_token() {
        let user_id = Uuid::now_v7();
        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        // Create a token that expired 5 minutes ago (use longer time to avoid timing issues)
        let now = chrono::Utc::now();
        let claims = Claims {
            sub: user_id.to_string(),
            typ: SESSION_TOKEN_TYPE.to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            verified: true,
            role: "user".to_string(),
            exp: (now - chrono::Duration::minutes(5)).timestamp(),
            iat: (now - chrono::Duration::minutes(20)).timestamp(),
        };

        let token =
            encode(&Header::new(Algorithm::RS256), &claims, &encoding_key).expect("encode failed");

        let result = verify_jwt(&token, &decoding_key);
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Authorization { code, .. } => {
                assert_eq!(
                    code,
                    crate::infrastructure::api::error::auth_codes::TOKEN_EXPIRED
                );
            }
            _ => panic!("Expected Authorization error"),
        }
    }

    #[test]
    fn test_verify_jwt_wrong_algorithm() {
        // Create a token with HS256 but try to verify with RS256 key
        let claims = Claims::new(Uuid::now_v7(), true, "user", 15);
        let token = encode(
            &Header::new(Algorithm::HS256),
            &claims,
            &EncodingKey::from_secret(b"secret"),
        )
        .expect("encode failed");

        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let result = verify_jwt(&token, &decoding_key);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AppError::Authorization { .. }
        ));
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

    /// Requesting an RS256 signature with an HMAC secret key makes `encode` fail;
    /// `create_jwt` must surface that as an internal error rather than panicking.
    #[test]
    fn test_create_jwt_encode_failure_is_internal() {
        let hmac_key = EncodingKey::from_secret(b"not-an-rsa-key");
        let result = create_jwt(Uuid::now_v7(), true, "user", &hmac_key, 15);
        match result {
            Err(AppError::Internal(msg)) => assert!(msg.contains("Failed to create JWT")),
            other => panic!("expected Internal error, got {other:?}"),
        }
    }

    /// A structurally malformed token (not three base64 segments) takes the
    /// `InvalidToken` arm and maps to a `token_invalid("malformed")` error.
    #[test]
    fn test_verify_jwt_malformed_maps_to_token_invalid() {
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");
        let result = verify_jwt("only-one-segment", &decoding_key);
        match result.unwrap_err() {
            AppError::Authorization { code, .. } => {
                assert_eq!(
                    code,
                    crate::infrastructure::api::error::auth_codes::TOKEN_INVALID
                );
            }
            other => panic!("expected Authorization error, got {other:?}"),
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

        let encoding_key =
            EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).expect("Invalid private key");
        let decoding_key =
            DecodingKey::from_rsa_pem(TEST_PUBLIC_KEY.as_bytes()).expect("Invalid public key");

        let now = Utc::now();
        let claims = RolelessClaims {
            sub: Uuid::now_v7().to_string(),
            typ: SESSION_TOKEN_TYPE.to_string(),
            iss: TOKEN_ISSUER.to_string(),
            aud: TOKEN_AUDIENCE.to_string(),
            verified: true,
            exp: (now + Duration::minutes(15)).timestamp(),
            iat: now.timestamp(),
        };
        let token =
            encode(&Header::new(Algorithm::RS256), &claims, &encoding_key).expect("encode failed");

        let verified = verify_jwt(&token, &decoding_key).expect("should verify");
        assert_eq!(verified.role, "user");
    }
}
