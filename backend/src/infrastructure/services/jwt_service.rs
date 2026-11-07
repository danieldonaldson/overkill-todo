//! JWT service implementation.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use jsonwebtoken::{Algorithm, Header, Validation, decode, encode};
use serde::{Deserialize, Serialize};

use crate::application::ports::JwtService;
use crate::domain::errors::DomainError;
use crate::domain::value_objects::{ResetCodeId, UserId};
use crate::shared::crypto::Crypto;

/// JWT claims for signup/reset tokens.
#[derive(Debug, Serialize, Deserialize)]
struct TokenClaims {
    /// Subject (user ID)
    sub: String,
    /// Expiration time
    exp: i64,
    /// Token type
    typ: String,
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
        expires_at: DateTime<Utc>,
        token_type: &str,
        jti: Option<String>,
    ) -> Result<String, DomainError> {
        let claims = TokenClaims {
            sub: user_id.to_string(),
            exp: expires_at.timestamp(),
            typ: token_type.to_string(),
            jti,
        };

        encode(
            &Header::new(Algorithm::RS256),
            &claims,
            self.crypto.encoding_key(),
        )
        .map_err(|e| DomainError::Persistence(format!("Failed to create JWT: {e}")))
    }

    fn validate_token(&self, token: &str, expected_type: &str) -> Result<TokenClaims, DomainError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_required_spec_claims(&["sub", "exp", "typ"]);

        let token_data = decode::<TokenClaims>(token, self.crypto.decoding_key(), &validation)
            .map_err(|e| DomainError::Authorization(format!("Invalid token: {e}")))?;

        if token_data.claims.typ != expected_type {
            return Err(DomainError::Authorization(format!(
                "Invalid token type: expected {expected_type}"
            )));
        }

        Ok(token_data.claims)
    }
}

/// Parses a UUID-backed identifier from a token claim, mapping failures to
/// an authorization error.
fn parse_token_uuid(value: &str, field: &str) -> Result<uuid::Uuid, DomainError> {
    uuid::Uuid::parse_str(value)
        .map_err(|e| DomainError::Authorization(format!("Invalid {field} in token: {e}")))
}

impl JwtService for CryptoJwtService {
    fn create_signup_token(
        &self,
        user_id: &UserId,
        expires_at: DateTime<Utc>,
    ) -> Result<String, DomainError> {
        self.create_token(user_id, expires_at, "signup", None)
    }

    fn create_reset_token(
        &self,
        user_id: &UserId,
        reset_code_id: &ResetCodeId,
        expires_at: DateTime<Utc>,
    ) -> Result<String, DomainError> {
        self.create_token(
            user_id,
            expires_at,
            "reset",
            Some(reset_code_id.to_string()),
        )
    }

    fn validate_signup_token(&self, token: &str) -> Result<UserId, DomainError> {
        let claims = self.validate_token(token, "signup")?;
        Ok(UserId::from_uuid(parse_token_uuid(&claims.sub, "user ID")?))
    }

    fn validate_reset_token(&self, token: &str) -> Result<(UserId, ResetCodeId), DomainError> {
        let claims = self.validate_token(token, "reset")?;
        let user_id = UserId::from_uuid(parse_token_uuid(&claims.sub, "user ID")?);
        let jti = claims
            .jti
            .ok_or_else(|| DomainError::Authorization("Reset token missing jti".into()))?;
        let reset_code_id = ResetCodeId::from_uuid(parse_token_uuid(&jti, "reset code ID")?);
        Ok((user_id, reset_code_id))
    }

    fn jwt_expiration_minutes(&self) -> i64 {
        self.crypto.jwt_expiration_minutes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn test_service() -> CryptoJwtService {
        CryptoJwtService::new(Arc::new(Crypto::for_testing()))
    }

    #[test]
    fn test_create_and_validate_signup_token() {
        let service = test_service();
        let user_id = UserId::new();
        let expires_at = Utc::now() + Duration::hours(1);

        let token = service.create_signup_token(&user_id, expires_at).unwrap();
        let validated_id = service.validate_signup_token(&token).unwrap();

        assert_eq!(validated_id, user_id);
    }

    #[test]
    fn test_create_and_validate_reset_token() {
        let service = test_service();
        let user_id = UserId::new();
        let reset_code_id = ResetCodeId::new();
        let expires_at = Utc::now() + Duration::hours(1);

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
        let expires_at = Utc::now() + Duration::hours(1);

        // Signup tokens carry no jti and a different type; must not pass reset validation.
        let token = service.create_signup_token(&user_id, expires_at).unwrap();
        assert!(service.validate_reset_token(&token).is_err());
    }

    #[test]
    fn test_wrong_token_type_fails() {
        let service = test_service();
        let user_id = UserId::new();
        let expires_at = Utc::now() + Duration::hours(1);

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
        let expires_at = Utc::now() - Duration::hours(1); // Expired

        let token = service.create_signup_token(&user_id, expires_at).unwrap();
        let result = service.validate_signup_token(&token);

        assert!(result.is_err());
    }
}
