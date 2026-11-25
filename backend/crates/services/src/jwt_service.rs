//! JWT service implementation.

use std::sync::Arc;

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use application::ports::JwtService;
use domain::errors::DomainError;
use domain::value_objects::{ResetCodeId, UserId};
use platform::crypto::{Crypto, TOKEN_REJECTED_MESSAGE as REJECTED, TokenType, token};

/// Payload for signup/reset tokens.
#[derive(Debug, Serialize, Deserialize)]
struct OneTimePayload {
    /// Subject (user ID)
    sub: String,
    /// JWT ID — for reset tokens, the reset-code row this token authorizes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    jti: Option<String>,
}

/// JWT service backed by an injected [`Crypto`] handle.
pub struct CryptoJwtService {
    crypto: Arc<Crypto>,
}

impl CryptoJwtService {
    /// Creates a new JWT service using the given crypto handle.
    #[must_use]
    pub const fn new(crypto: Arc<Crypto>) -> Self {
        Self { crypto }
    }

    fn create_token(
        &self,
        user_id: &UserId,
        expires_at: Timestamp,
        token_type: TokenType,
        jti: Option<String>,
    ) -> Result<String, DomainError> {
        let payload = OneTimePayload {
            sub: user_id.to_string(),
            jti,
        };

        token::issue(
            self.crypto.tokens(),
            token_type,
            &payload,
            Timestamp::now(),
            expires_at,
        )
        .map_err(|e| DomainError::Persistence(format!("Failed to create JWT: {e}")))
    }

    fn validate_token(
        &self,
        token_str: &str,
        expected_type: TokenType,
    ) -> Result<OneTimePayload, DomainError> {
        let verified = token::verify::<OneTimePayload>(
            self.crypto.tokens(),
            token_str,
            expected_type,
        )
        .map_err(|err| {
            // The reason stays in debug logs; the token is never logged.
            tracing::debug!(?err, expected_type = %expected_type, "one-time token rejected");
            DomainError::Authorization(REJECTED.into())
        })?;

        Ok(verified.payload)
    }
}

/// Parses a UUID-backed identifier from a token claim, logging the parse error
/// at debug and returning the generic rejection message so internal detail
/// never reaches clients.
fn parse_token_uuid(value: &str, field: &str) -> Result<uuid::Uuid, DomainError> {
    uuid::Uuid::parse_str(value).map_err(|e| {
        tracing::debug!(error = %e, field, "one-time token rejected: invalid UUID claim");
        DomainError::Authorization(REJECTED.into())
    })
}

impl JwtService for CryptoJwtService {
    fn create_signup_token(
        &self,
        user_id: &UserId,
        expires_at: Timestamp,
    ) -> Result<String, DomainError> {
        self.create_token(user_id, expires_at, TokenType::Signup, None)
    }

    fn create_reset_token(
        &self,
        user_id: &UserId,
        reset_code_id: &ResetCodeId,
        expires_at: Timestamp,
    ) -> Result<String, DomainError> {
        self.create_token(
            user_id,
            expires_at,
            TokenType::Reset,
            Some(reset_code_id.to_string()),
        )
    }

    fn validate_signup_token(&self, token: &str) -> Result<UserId, DomainError> {
        let claims = self.validate_token(token, TokenType::Signup)?;
        Ok(UserId::from_uuid(parse_token_uuid(&claims.sub, "user ID")?))
    }

    fn validate_reset_token(&self, token: &str) -> Result<(UserId, ResetCodeId), DomainError> {
        let claims = self.validate_token(token, TokenType::Reset)?;
        let user_id = UserId::from_uuid(parse_token_uuid(&claims.sub, "user ID")?);
        let jti = claims.jti.ok_or_else(|| {
            tracing::debug!("one-time token rejected: reset token missing jti");
            DomainError::Authorization(REJECTED.into())
        })?;
        let reset_code_id = ResetCodeId::from_uuid(parse_token_uuid(&jti, "reset code ID")?);
        Ok((user_id, reset_code_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::SignedDuration;

    fn test_service() -> CryptoJwtService {
        CryptoJwtService::new(Arc::new(Crypto::for_testing()))
    }

    #[test]
    fn test_create_and_validate_signup_token() {
        let service = test_service();
        let user_id = UserId::new();
        let expires_at = Timestamp::now() + SignedDuration::from_hours(1);

        let token = service.create_signup_token(&user_id, expires_at).unwrap();
        let validated_id = service.validate_signup_token(&token).unwrap();

        assert_eq!(validated_id, user_id);
    }

    #[test]
    fn test_create_and_validate_reset_token() {
        let service = test_service();
        let user_id = UserId::new();
        let reset_code_id = ResetCodeId::new();
        let expires_at = Timestamp::now() + SignedDuration::from_hours(1);

        let token = service
            .create_reset_token(&user_id, &reset_code_id, expires_at)
            .unwrap();
        let (validated_id, validated_code_id) = service.validate_reset_token(&token).unwrap();

        assert_eq!(validated_id, user_id);
        assert_eq!(validated_code_id, reset_code_id);
    }

    #[test]
    fn test_signup_token_rejected_as_reset_token() {
        let service = test_service();
        let user_id = UserId::new();
        let expires_at = Timestamp::now() + SignedDuration::from_hours(1);

        // Signup tokens carry no jti and a different type; must not pass reset validation.
        let token = service.create_signup_token(&user_id, expires_at).unwrap();
        assert!(service.validate_reset_token(&token).is_err());
    }

    #[test]
    fn test_wrong_token_type_fails() {
        let service = test_service();
        let user_id = UserId::new();
        let expires_at = Timestamp::now() + SignedDuration::from_hours(1);

        // Create a signup token
        let token = service.create_signup_token(&user_id, expires_at).unwrap();

        // Try to validate as reset token - should fail
        let result = service.validate_reset_token(&token);
        assert!(result.is_err());
    }

    #[test]
    fn test_expired_token_fails() {
        let service = test_service();
        let user_id = UserId::new();
        let expires_at = Timestamp::now() - SignedDuration::from_hours(1); // Expired

        let token = service.create_signup_token(&user_id, expires_at).unwrap();
        let result = service.validate_signup_token(&token);

        assert!(result.is_err());
    }

    fn assert_rejected<T: std::fmt::Debug>(result: Result<T, DomainError>) {
        match result {
            Err(DomainError::Authorization(msg)) => assert_eq!(msg, REJECTED),
            other => panic!("expected the generic rejection, got {other:?}"),
        }
    }

    #[test]
    fn rejections_never_say_which_check_failed() {
        let service = test_service();
        let user_id = UserId::new();
        let signup = service
            .create_signup_token(&user_id, Timestamp::now() + SignedDuration::from_hours(1))
            .unwrap();
        let expired = service
            .create_signup_token(&user_id, Timestamp::now() - SignedDuration::from_hours(1))
            .unwrap();
        assert_rejected(service.validate_reset_token(&signup));
        assert_rejected(service.validate_signup_token(&expired));
        assert_rejected(service.validate_signup_token("not.a.token"));
    }

    #[test]
    fn reset_token_from_a_retired_key_is_rejected() {
        let old = Crypto::for_testing_with_tokens(
            platform::crypto::TokenKeyring::from_config(
                &platform::crypto::keyring_test_entry("retired", 7),
                None,
            )
            .unwrap(),
        );
        let token = CryptoJwtService::new(Arc::new(old))
            .create_reset_token(
                &UserId::new(),
                &ResetCodeId::new(),
                Timestamp::now() + SignedDuration::from_hours(1),
            )
            .unwrap();
        assert_rejected(test_service().validate_reset_token(&token));
    }

    #[derive(Serialize)]
    struct RawOneTime {
        sub: String,
        typ: &'static str,
        iss: &'static str,
        aud: &'static str,
        iat: i64,
        exp: i64,
        #[serde(skip_serializing_if = "Option::is_none")]
        jti: Option<String>,
    }

    #[test]
    fn one_time_tokens_reject_foreign_audience_and_issuer() {
        let crypto = Arc::new(Crypto::for_testing());
        let service = CryptoJwtService::new(Arc::clone(&crypto));
        let now = Timestamp::now();
        let exp = (now + SignedDuration::from_hours(1)).as_second();

        for (typ, is_signup) in [("signup", true), ("reset", false)] {
            let foreign_aud = crypto
                .tokens()
                .sign(&RawOneTime {
                    sub: UserId::new().to_string(),
                    typ,
                    iss: platform::crypto::TOKEN_ISSUER,
                    aud: "foreign-audience",
                    iat: now.as_second(),
                    exp,
                    jti: (!is_signup).then(|| ResetCodeId::new().to_string()),
                })
                .unwrap();
            let foreign_iss = crypto
                .tokens()
                .sign(&RawOneTime {
                    sub: UserId::new().to_string(),
                    typ,
                    iss: "foreign-issuer",
                    aud: platform::crypto::TOKEN_AUDIENCE,
                    iat: now.as_second(),
                    exp,
                    jti: (!is_signup).then(|| ResetCodeId::new().to_string()),
                })
                .unwrap();

            if is_signup {
                assert_rejected(service.validate_signup_token(&foreign_aud));
                assert_rejected(service.validate_signup_token(&foreign_iss));
            } else {
                assert_rejected(service.validate_reset_token(&foreign_aud));
                assert_rejected(service.validate_reset_token(&foreign_iss));
            }
        }
    }

    #[test]
    fn malformed_uuid_claims_and_missing_jti_use_the_generic_rejection() {
        let crypto = Arc::new(Crypto::for_testing());
        let service = CryptoJwtService::new(Arc::clone(&crypto));
        let now = Timestamp::now();
        let exp = (now + SignedDuration::from_hours(1)).as_second();

        for (typ, sub, jti) in [
            ("signup", "not-a-uuid".to_string(), None),
            (
                "reset",
                "not-a-uuid".to_string(),
                Some(ResetCodeId::new().to_string()),
            ),
            (
                "reset",
                UserId::new().to_string(),
                Some("not-a-uuid".into()),
            ),
            ("reset", UserId::new().to_string(), None),
        ] {
            let token = crypto
                .tokens()
                .sign(&RawOneTime {
                    sub,
                    typ,
                    iss: platform::crypto::TOKEN_ISSUER,
                    aud: platform::crypto::TOKEN_AUDIENCE,
                    iat: now.as_second(),
                    exp,
                    jti,
                })
                .unwrap();
            if typ == "signup" {
                assert_rejected(service.validate_signup_token(&token));
            } else {
                assert_rejected(service.validate_reset_token(&token));
            }
        }
    }
}
