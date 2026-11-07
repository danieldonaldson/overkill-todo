//! Application-level error types.

use crate::domain::errors::DomainError;

use crate::infrastructure::config::is_dev_mode;
use crate::shared::fingerprint::generate_error_fingerprint;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Structured error response returned to clients.
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub code: String,
    pub message: String,
}

/// Application-level errors that map to HTTP responses.
///
/// `Display` is derived (`thiserror`); the HTTP-semantic matches —
/// [`AppError::error_code`], [`AppError::fingerprint`], and the
/// [`IntoResponse`] status/redaction mapping — stay explicit because they encode
/// status codes, dev-mode message redaction, and fingerprint logging that a
/// `Display` derive cannot express.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Validation error: {0}")]
    Validation(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Authorization error: {message}")]
    Authorization { code: &'static str, message: String },
    #[error("Forbidden: {message}")]
    Forbidden { code: &'static str, message: String },
    #[error("Conflict: {0}")]
    Conflict(String),
    #[error("Internal error: {0}")]
    Internal(String),
    #[error("Database error: {0}")]
    Database(String),
    #[error("External service error: {0}")]
    ExternalService(String),
    #[error("Signup incomplete for: {email}")]
    SignupIncomplete { email: String },
}

/// Common error codes for 401 Unauthorized responses.
pub mod auth_codes {
    pub const TOKEN_MISSING: &str = "token_missing";
    pub const TOKEN_EXPIRED: &str = "token_expired";
    pub const TOKEN_INVALID: &str = "token_invalid";
    pub const INVALID_CREDENTIALS: &str = "invalid_credentials";
}

/// Common error codes for 403 Forbidden responses.
pub mod forbidden_codes {
    pub const EMAIL_NOT_VERIFIED: &str = "email_not_verified";
    pub const ACCESS_DENIED: &str = "access_denied";
    pub const CSRF_INVALID: &str = "csrf_invalid";
}

impl AppError {
    #[must_use]
    pub fn token_missing() -> Self {
        Self::Authorization {
            code: auth_codes::TOKEN_MISSING,
            message: "Missing authentication token".into(),
        }
    }

    #[must_use]
    pub fn token_expired() -> Self {
        Self::Authorization {
            code: auth_codes::TOKEN_EXPIRED,
            message: "Token has expired".into(),
        }
    }

    #[must_use]
    pub fn token_invalid(details: &str) -> Self {
        Self::Authorization {
            code: auth_codes::TOKEN_INVALID,
            message: format!("Invalid token: {details}"),
        }
    }

    #[must_use]
    pub fn invalid_credentials() -> Self {
        Self::Authorization {
            code: auth_codes::INVALID_CREDENTIALS,
            message: "Invalid email or password".into(),
        }
    }

    #[must_use]
    pub fn email_not_verified() -> Self {
        Self::Forbidden {
            code: forbidden_codes::EMAIL_NOT_VERIFIED,
            message: "Please verify your email to access this resource".into(),
        }
    }

    #[must_use]
    pub fn access_denied() -> Self {
        Self::Forbidden {
            code: forbidden_codes::ACCESS_DENIED,
            message: "You do not have permission to access this resource".into(),
        }
    }

    #[must_use]
    pub fn csrf_invalid() -> Self {
        Self::Forbidden {
            code: forbidden_codes::CSRF_INVALID,
            message: "Invalid or missing CSRF token".into(),
        }
    }

    const fn error_code(&self) -> &'static str {
        match self {
            Self::Validation(_) => "validation_error",
            Self::NotFound(_) => "not_found",
            Self::Authorization { code, .. } | Self::Forbidden { code, .. } => code,
            Self::Conflict(_) => "conflict",
            Self::Internal(_) => "internal_error",
            Self::Database(_) => "database_error",
            Self::ExternalService(_) => "external_service_error",
            Self::SignupIncomplete { .. } => "signup_incomplete",
        }
    }

    fn fingerprint(&self) -> Option<String> {
        match self {
            Self::Internal(msg) => Some(generate_error_fingerprint("Internal", msg)),
            Self::Database(msg) => Some(generate_error_fingerprint("Database", msg)),
            Self::ExternalService(msg) => Some(generate_error_fingerprint("ExternalService", msg)),
            _ => None,
        }
    }
}

impl From<DomainError> for AppError {
    fn from(error: DomainError) -> Self {
        match error {
            DomainError::Validation(msg) | DomainError::BusinessRule(msg) => Self::Validation(msg),
            DomainError::Authorization(msg) => Self::Authorization {
                code: auth_codes::TOKEN_INVALID,
                message: msg,
            },
            DomainError::Forbidden(msg) => Self::Forbidden {
                code: forbidden_codes::ACCESS_DENIED,
                message: msg,
            },
            DomainError::NotFound(msg) => Self::NotFound(msg),
            DomainError::Conflict(msg) => Self::Conflict(msg),
            DomainError::Persistence(msg) => Self::Database(msg),
            DomainError::ExternalService(msg) => Self::ExternalService(msg),
        }
    }
}

impl From<crate::application::use_cases::VerifyEmailError> for AppError {
    fn from(error: crate::application::use_cases::VerifyEmailError) -> Self {
        use crate::application::use_cases::VerifyEmailError;
        match error {
            VerifyEmailError::NotFound(msg) => Self::NotFound(msg),
            VerifyEmailError::CodeExpired => {
                Self::Validation("Verification code has expired".into())
            }
            VerifyEmailError::CodeLocked => {
                Self::Validation("Too many failed attempts. Please request a new code.".into())
            }
            VerifyEmailError::CodeInvalid { attempts_remaining } => Self::Validation(format!(
                "Invalid code. {attempts_remaining} attempts remaining."
            )),
            VerifyEmailError::AlreadyVerified(_) => Self::Conflict("Email already verified".into()),
            VerifyEmailError::Internal(msg) => Self::Internal(msg),
        }
    }
}

impl From<crate::application::use_cases::ResendVerificationError> for AppError {
    fn from(error: crate::application::use_cases::ResendVerificationError) -> Self {
        use crate::application::use_cases::ResendVerificationError;
        match error {
            ResendVerificationError::NotFound(msg) => Self::NotFound(msg),
            ResendVerificationError::AlreadyVerified => {
                Self::Conflict("Email already verified".into())
            }
            ResendVerificationError::CooldownActive { seconds_remaining } => Self::Validation(
                format!("Please wait {seconds_remaining} seconds before requesting another code"),
            ),
            ResendVerificationError::Locked {
                seconds_until_unlock,
            } => Self::Validation(format!(
                "Too many incorrect attempts. This code is locked for {seconds_until_unlock} seconds; please try again later"
            )),
            ResendVerificationError::Internal(msg) => Self::Internal(msg),
        }
    }
}

impl From<sqlx::Error> for AppError {
    fn from(error: sqlx::Error) -> Self {
        match error {
            sqlx::Error::RowNotFound => Self::NotFound("Resource not found".into()),
            sqlx::Error::Database(db_err) => {
                if let Some(code) = db_err.code()
                    && code == "23505"
                {
                    return Self::Conflict("Resource already exists".into());
                }
                Self::Database(db_err.to_string())
            }
            _ => Self::Database(error.to_string()),
        }
    }
}

impl From<argon2::password_hash::Error> for AppError {
    fn from(error: argon2::password_hash::Error) -> Self {
        Self::Internal(format!("Password hashing error: {error}"))
    }
}

impl From<argon2::Error> for AppError {
    fn from(error: argon2::Error) -> Self {
        Self::Internal(format!("Password hashing error: {error}"))
    }
}

impl From<jsonwebtoken::errors::Error> for AppError {
    fn from(error: jsonwebtoken::errors::Error) -> Self {
        Self::Authorization {
            code: auth_codes::TOKEN_INVALID,
            message: format!("JWT error: {error}"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let dev_mode = is_dev_mode();

        if let Self::SignupIncomplete { email } = self {
            let body = serde_json::json!({
                "error": "Please complete your registration",
                "error_code": "signup_incomplete",
                "email": email
            });
            return (StatusCode::FORBIDDEN, Json(body)).into_response();
        }

        let code = self.error_code().to_string();

        let (status, message) = match &self {
            Self::Validation(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Self::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Self::Authorization { message, .. } => (StatusCode::UNAUTHORIZED, message.clone()),
            Self::Forbidden { message, .. } => (StatusCode::FORBIDDEN, message.clone()),
            Self::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            Self::Internal(msg) => {
                let fingerprint = self.fingerprint().unwrap_or_default();
                tracing::error!(
                    error_fingerprint = %fingerprint,
                    error_type = "Internal",
                    "Internal error: {}", msg
                );
                let response_msg = if dev_mode {
                    format!("Internal error: {msg}")
                } else {
                    "Internal server error".to_string()
                };
                (StatusCode::INTERNAL_SERVER_ERROR, response_msg)
            }
            Self::Database(msg) => {
                let fingerprint = self.fingerprint().unwrap_or_default();
                tracing::error!(
                    error_fingerprint = %fingerprint,
                    error_type = "Database",
                    "Database error: {}", msg
                );
                let response_msg = if dev_mode {
                    format!("Database error: {msg}")
                } else {
                    "Database error".to_string()
                };
                (StatusCode::INTERNAL_SERVER_ERROR, response_msg)
            }
            Self::ExternalService(msg) => {
                let fingerprint = self.fingerprint().unwrap_or_default();
                tracing::error!(
                    error_fingerprint = %fingerprint,
                    error_type = "ExternalService",
                    "External service error: {}", msg
                );
                let response_msg = if dev_mode {
                    format!("External service error: {msg}")
                } else {
                    "External service unavailable".to_string()
                };
                (StatusCode::BAD_GATEWAY, response_msg)
            }
            Self::SignupIncomplete { .. } => unreachable!("handled above"),
        };

        let body = Json(ErrorResponse { code, message });
        (status, body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::StatusCode;

    #[test]
    fn test_display_validation_error() {
        let error = AppError::Validation("invalid email".into());
        assert_eq!(format!("{error}"), "Validation error: invalid email");
    }

    #[test]
    fn test_display_not_found_error() {
        let error = AppError::NotFound("user not found".into());
        assert_eq!(format!("{error}"), "Not found: user not found");
    }

    #[test]
    fn test_display_authorization_error() {
        let error = AppError::token_invalid("invalid token");
        assert_eq!(
            format!("{error}"),
            "Authorization error: Invalid token: invalid token"
        );
    }

    #[test]
    fn test_display_conflict_error() {
        let error = AppError::Conflict("email already exists".into());
        assert_eq!(format!("{error}"), "Conflict: email already exists");
    }

    #[test]
    fn test_display_internal_error() {
        let error = AppError::Internal("something went wrong".into());
        assert_eq!(format!("{error}"), "Internal error: something went wrong");
    }

    #[test]
    fn test_display_database_error() {
        let error = AppError::Database("connection failed".into());
        assert_eq!(format!("{error}"), "Database error: connection failed");
    }

    #[test]
    fn test_from_jwt_error() {
        let result = jsonwebtoken::decode::<serde_json::Value>(
            "invalid",
            &jsonwebtoken::DecodingKey::from_secret(b"secret"),
            &jsonwebtoken::Validation::default(),
        );
        let jwt_error = result.unwrap_err();
        let app_error: AppError = jwt_error.into();
        assert!(
            matches!(&app_error, AppError::Authorization { message, .. } if message.contains("JWT error"))
        );
    }

    #[tokio::test]
    async fn test_into_response_validation() {
        let error = AppError::Validation("invalid input".into());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "validation_error");
        assert_eq!(json["message"], "invalid input");
    }

    #[tokio::test]
    async fn test_into_response_not_found() {
        let error = AppError::NotFound("resource not found".into());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "not_found");
        assert_eq!(json["message"], "resource not found");
    }

    #[tokio::test]
    async fn test_into_response_authorization() {
        let error = AppError::token_missing();
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], auth_codes::TOKEN_MISSING);
        assert_eq!(json["message"], "Missing authentication token");
    }

    #[tokio::test]
    async fn test_into_response_conflict() {
        let error = AppError::Conflict("already exists".into());
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::CONFLICT);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "conflict");
        assert_eq!(json["message"], "already exists");
    }

    #[tokio::test]
    async fn test_into_response_signup_incomplete() {
        let error = AppError::SignupIncomplete {
            email: "test@example.com".into(),
        };
        let response = error.into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"], "Please complete your registration");
        assert_eq!(json["error_code"], "signup_incomplete");
        assert_eq!(json["email"], "test@example.com");
    }

    #[test]
    fn test_fingerprint_internal_error() {
        let error = AppError::Internal("something went wrong".into());
        let fp = error.fingerprint();
        assert!(fp.is_some());
        assert_eq!(fp.unwrap().len(), 8);
    }

    #[test]
    fn test_fingerprint_client_errors_none() {
        assert!(AppError::Validation("x".into()).fingerprint().is_none());
        assert!(AppError::NotFound("x".into()).fingerprint().is_none());
        assert!(AppError::token_invalid("x").fingerprint().is_none());
        assert!(AppError::access_denied().fingerprint().is_none());
        assert!(AppError::Conflict("x".into()).fingerprint().is_none());
    }

    #[test]
    fn test_display_forbidden_and_external_and_signup() {
        assert_eq!(
            format!("{}", AppError::access_denied()),
            "Forbidden: You do not have permission to access this resource"
        );
        assert_eq!(
            format!("{}", AppError::ExternalService("upstream down".into())),
            "External service error: upstream down"
        );
        assert_eq!(
            format!(
                "{}",
                AppError::SignupIncomplete {
                    email: "a@b.com".into()
                }
            ),
            "Signup incomplete for: a@b.com"
        );
    }

    #[test]
    fn test_error_code_for_each_variant() {
        assert_eq!(
            AppError::Validation("x".into()).error_code(),
            "validation_error"
        );
        assert_eq!(AppError::NotFound("x".into()).error_code(), "not_found");
        assert_eq!(
            AppError::token_invalid("x").error_code(),
            auth_codes::TOKEN_INVALID
        );
        assert_eq!(
            AppError::access_denied().error_code(),
            forbidden_codes::ACCESS_DENIED
        );
        assert_eq!(AppError::Conflict("x".into()).error_code(), "conflict");
        assert_eq!(
            AppError::Internal("x".into()).error_code(),
            "internal_error"
        );
        assert_eq!(
            AppError::Database("x".into()).error_code(),
            "database_error"
        );
        assert_eq!(
            AppError::ExternalService("x".into()).error_code(),
            "external_service_error"
        );
        assert_eq!(
            AppError::SignupIncomplete { email: "x".into() }.error_code(),
            "signup_incomplete"
        );
    }

    #[test]
    fn test_fingerprint_server_errors_some() {
        assert!(AppError::Database("x".into()).fingerprint().is_some());
        assert!(
            AppError::ExternalService("x".into())
                .fingerprint()
                .is_some()
        );
    }

    #[tokio::test]
    async fn test_into_response_forbidden() {
        let response = AppError::email_not_verified().into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], forbidden_codes::EMAIL_NOT_VERIFIED);
        assert_eq!(
            json["message"],
            "Please verify your email to access this resource"
        );
    }

    #[tokio::test]
    async fn test_into_response_csrf_invalid() {
        let response = AppError::csrf_invalid().into_response();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], forbidden_codes::CSRF_INVALID);
    }

    // In unit tests DEV_MODE is unset, so `is_dev_mode()` is false: server-side
    // errors must be redacted to a generic message and never leak `msg`.
    #[tokio::test]
    async fn test_into_response_internal_redacted_in_prod() {
        let response = AppError::Internal("db password leaked".into()).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "internal_error");
        assert_eq!(json["message"], "Internal server error");
        assert!(
            !json["message"].as_str().unwrap().contains("leaked"),
            "internal detail must not leak in prod mode"
        );
    }

    #[tokio::test]
    async fn test_into_response_database_redacted_in_prod() {
        let response = AppError::Database("constraint xyz".into()).into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "database_error");
        assert_eq!(json["message"], "Database error");
    }

    #[tokio::test]
    async fn test_into_response_external_service_is_bad_gateway() {
        let response = AppError::ExternalService("smtp timeout".into()).into_response();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["code"], "external_service_error");
        assert_eq!(json["message"], "External service unavailable");
    }

    #[test]
    fn test_from_domain_error_variants() {
        use crate::domain::errors::DomainError;
        assert!(matches!(
            AppError::from(DomainError::Validation("v".into())),
            AppError::Validation(_)
        ));
        assert!(matches!(
            AppError::from(DomainError::BusinessRule("b".into())),
            AppError::Validation(_)
        ));
        assert!(matches!(
            AppError::from(DomainError::Authorization("a".into())),
            AppError::Authorization { code, .. } if code == auth_codes::TOKEN_INVALID
        ));
        assert!(matches!(
            AppError::from(DomainError::Forbidden("f".into())),
            AppError::Forbidden { code, .. } if code == forbidden_codes::ACCESS_DENIED
        ));
        assert!(matches!(
            AppError::from(DomainError::NotFound("n".into())),
            AppError::NotFound(_)
        ));
        assert!(matches!(
            AppError::from(DomainError::Conflict("c".into())),
            AppError::Conflict(_)
        ));
        assert!(matches!(
            AppError::from(DomainError::Persistence("p".into())),
            AppError::Database(_)
        ));
        assert!(matches!(
            AppError::from(DomainError::ExternalService("e".into())),
            AppError::ExternalService(_)
        ));
    }

    #[test]
    fn test_from_sqlx_row_not_found_maps_to_not_found() {
        let err: AppError = sqlx::Error::RowNotFound.into();
        assert!(matches!(err, AppError::NotFound(_)));
    }

    #[test]
    fn test_from_argon2_errors_map_to_internal() {
        let err: AppError = argon2::Error::AlgorithmInvalid.into();
        assert!(matches!(err, AppError::Internal(msg) if msg.contains("Password hashing")));

        let ph_err: AppError = argon2::password_hash::Error::Password.into();
        assert!(matches!(ph_err, AppError::Internal(msg) if msg.contains("Password hashing")));
    }
}
