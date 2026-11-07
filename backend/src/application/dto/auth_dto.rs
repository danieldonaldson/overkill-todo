//! Authentication-related DTOs.

use super::validation::validate_verification_code;
use crate::domain::entities::User;
use crate::domain::value_objects::Email;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::Validate;

/// Request body for initiating user registration (email-only step).
///
/// This is step 1 of signup: user provides email, we send verification code.
/// Password is set later after email verification.
/// Email validation: frontend does regex check, backend does MX record check.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct InitiateRegistrationRequest {
    #[validate(email(message = "Invalid email format"))]
    pub email: String,

    /// Cloudflare Turnstile token for bot protection.
    pub turnstile_token: Option<String>,
}

/// Request body for completing registration (set password after email verification).
///
/// This is step 3 of signup: user sets password after verifying email.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct CompleteRegistrationRequest {
    /// The signup completion token from verify-email endpoint (30 min expiry).
    pub signup_token: String,

    /// Email address (for password manager autofill, verified against token).
    #[validate(email(message = "Invalid email format"))]
    pub email: String,

    #[validate(length(min = 8, max = 256, message = "Password must be 8-256 characters"))]
    pub password: String,
}

/// Response body for initiating registration.
#[derive(Debug, Serialize, ToSchema)]
pub struct InitiateRegistrationResponse {
    pub success: bool,
    pub message: String,
}

/// Request body for user login.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    #[validate(email(message = "Invalid email format"))]
    pub email: String,
    #[validate(length(max = 256, message = "Password too long"))]
    pub password: String,

    /// Cloudflare Turnstile token for bot protection.
    pub turnstile_token: Option<String>,
}

/// Request body for email verification.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct VerifyEmailRequest {
    #[validate(email(message = "Invalid email format"))]
    pub email: String,

    #[validate(custom(function = "validate_verification_code"))]
    pub code: String,
}

/// Request body for resending verification code.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ResendVerificationRequest {
    #[validate(email(message = "Invalid email format"))]
    pub email: String,

    /// Cloudflare Turnstile token for bot protection.
    pub turnstile_token: Option<String>,
}

/// Request body for forgot password.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ForgotPasswordRequest {
    #[validate(email(message = "Invalid email format"))]
    pub email: String,

    /// Cloudflare Turnstile token for bot protection.
    pub turnstile_token: Option<String>,
}

/// Request body for verifying reset code.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct VerifyResetCodeRequest {
    #[validate(email(message = "Invalid email format"))]
    pub email: String,

    #[validate(custom(function = "validate_verification_code"))]
    pub code: String,
}

/// Response body for verify reset code.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyResetCodeResponse {
    /// Short-lived JWT token for password reset (5 min expiry).
    pub reset_token: String,
}

/// Response body for email verification during signup.
///
/// New users (no password yet) receive a `signup_token` to complete signup.
/// Existing users with a password are logged in via `HttpOnly` cookies (the
/// body-token path was removed), so no token fields are returned.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerifyEmailResponse {
    /// The verified user's information.
    pub user: UserResponse,
    /// Short-lived JWT token for completing signup (30 min expiry).
    /// Only present for users who haven't set a password yet.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signup_token: Option<String>,
    /// Unix-seconds access-token expiry, present only on the auto-login path
    /// (existing user with a password). Lets the client schedule a proactive
    /// refresh instead of waiting for the first 401; the signup path omits it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
}

/// Response body for resend verification.
#[derive(Debug, Serialize, ToSchema)]
pub struct ResendVerificationResponse {
    pub success: bool,
    pub message: String,
    /// Seconds until next resend is allowed (for countdown timer).
    pub cooldown_seconds: i64,
}

/// Error codes for verification-related errors.
/// These allow the frontend to show appropriate UI for each error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum VerificationErrorCode {
    /// The code entered is incorrect.
    CodeInvalid,
    /// The code has expired.
    CodeExpired,
    /// Too many failed attempts, code is locked.
    CodeLocked,
    /// Must wait before requesting a new code.
    CooldownActive,
    /// The reset token has expired.
    TokenExpired,
    /// Email is already verified.
    AlreadyVerified,
    /// User has not completed signup (no password set).
    SignupIncomplete,
}

/// Structured error response for verification endpoints.
#[derive(Debug, Serialize, ToSchema)]
pub struct VerificationErrorResponse {
    pub error: String,
    pub error_code: VerificationErrorCode,
    /// For `CODE_INVALID`: attempts remaining before lockout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempts_remaining: Option<i32>,
    /// For `COOLDOWN_ACTIVE`: seconds until resend is allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cooldown_seconds: Option<i64>,
    /// For `SIGNUP_INCOMPLETE`: the email address to continue signup.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

impl VerificationErrorResponse {
    /// Base constructor with the optional detail fields defaulted to `None`.
    /// Use the `with_*` builders to attach the fields a given variant carries.
    #[must_use]
    pub fn new(error: impl Into<String>, error_code: VerificationErrorCode) -> Self {
        Self {
            error: error.into(),
            error_code,
            attempts_remaining: None,
            cooldown_seconds: None,
            email: None,
        }
    }

    /// Attaches the remaining-attempts count (`CODE_INVALID` / `CODE_LOCKED`).
    #[must_use]
    pub const fn with_attempts_remaining(mut self, attempts: i32) -> Self {
        self.attempts_remaining = Some(attempts);
        self
    }

    /// Attaches the resend cooldown (`COOLDOWN_ACTIVE` / lockout window).
    #[must_use]
    pub const fn with_cooldown_seconds(mut self, seconds: i64) -> Self {
        self.cooldown_seconds = Some(seconds);
        self
    }
}

/// Request body for resetting password.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct ResetPasswordRequest {
    /// The reset token from verify-reset-code endpoint.
    pub reset_token: String,

    #[validate(length(min = 8, max = 256, message = "Password must be 8-256 characters"))]
    pub new_password: String,
}

/// User information in API responses.
#[derive(Debug, Serialize, ToSchema)]
pub struct UserResponse {
    pub id: Uuid,
    pub email: String,
    pub first_name: String,
    pub last_name: String,
    pub email_verified: bool,
    /// Whether the user has a password set (false for OAuth-only users).
    pub has_password: bool,
    /// List of linked OAuth providers (e.g., `["google", "apple"]`).
    pub oauth_providers: Vec<String>,
    /// Pending email address awaiting verification (for email change flow).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_email: Option<String>,
    /// When the pending email change was requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_email_requested_at: Option<DateTime<Utc>>,
    /// When the account is scheduled for deletion (during 14-day grace period).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deletion_scheduled_at: Option<DateTime<Utc>>,
    /// User's role ("user", "admin", or "`super_admin`").
    pub role: String,
    pub created_at: DateTime<Utc>,
}

impl UserResponse {
    /// Creates a `UserResponse` from a User with OAuth provider info.
    pub fn from_user_with_oauth(user: User, oauth_providers: Vec<String>) -> Self {
        let has_password = user.has_password();
        let parts = user.into_parts();
        Self {
            id: *parts.id.as_uuid(),
            email: parts.email.into_string(),
            first_name: parts.first_name,
            last_name: parts.last_name,
            email_verified: parts.email_verified,
            has_password,
            oauth_providers,
            pending_email: parts.pending_email.map(Email::into_string),
            pending_email_requested_at: parts.pending_email_requested_at,
            deletion_scheduled_at: parts.deletion_scheduled_at,
            role: parts.role.to_db_str().to_string(),
            created_at: parts.created_at,
        }
    }
}

impl From<User> for UserResponse {
    /// Maps a user with no OAuth-provider context (the providers list is empty).
    /// Delegates to [`UserResponse::from_user_with_oauth`] to keep the
    /// `into_parts` destructure in one place.
    fn from(user: User) -> Self {
        Self::from_user_with_oauth(user, Vec::new())
    }
}

/// Response body for successful authentication and for token refresh.
///
/// The JWT access token and refresh token are delivered exclusively as
/// `HttpOnly` cookies (the native-client body-token path was removed), so only
/// the non-secret session metadata is returned in the body. Login and refresh
/// return the identical shape, so they share this one DTO rather than a
/// duplicate `RefreshResponse`.
#[derive(Debug, Serialize, ToSchema)]
pub struct AuthResponse {
    /// The authenticated user's information.
    pub user: UserResponse,
    /// Unix timestamp when the access token expires.
    pub expires_at: i64,
    /// CSRF token for state-changing requests (also set as a readable cookie).
    pub csrf_token: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use validator::Validate;

    fn user_response() -> UserResponse {
        UserResponse {
            id: Uuid::nil(),
            email: "test@example.com".to_string(),
            first_name: String::new(),
            last_name: String::new(),
            email_verified: true,
            has_password: true,
            oauth_providers: Vec::new(),
            pending_email: None,
            pending_email_requested_at: None,
            deletion_scheduled_at: None,
            role: "user".to_string(),
            created_at: Utc::now(),
        }
    }

    #[test]
    fn verify_email_request_accepts_formatted_code() {
        let request = VerifyEmailRequest {
            email: "test@example.com".to_string(),
            code: "123-456".to_string(),
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn verify_reset_code_request_accepts_spaced_code() {
        let request = VerifyResetCodeRequest {
            email: "test@example.com".to_string(),
            code: "123 456".to_string(),
        };

        assert!(request.validate().is_ok());
    }

    #[test]
    fn verify_code_requests_reject_codes_without_six_digits() {
        let request = VerifyEmailRequest {
            email: "test@example.com".to_string(),
            code: "12-34".to_string(),
        };

        assert!(request.validate().is_err());
    }

    /// Tokens are never returned in the body — only `user`, `expires_at`,
    /// and the (cookie-mirrored) CSRF token. Covers both login and refresh, which
    /// now share `AuthResponse`.
    #[test]
    fn auth_response_never_carries_tokens_in_body() {
        let response = AuthResponse {
            user: user_response(),
            expires_at: 1_735_689_600,
            csrf_token: "csrf-token".to_string(),
        };

        let value = serde_json::to_value(response).unwrap();

        assert!(value.get("token").is_none());
        assert!(value.get("refresh_token").is_none());
        assert_eq!(value["csrf_token"], "csrf-token");
        assert_eq!(value["expires_at"], 1_735_689_600_i64);
    }
}

/// Query parameters for magic link verification.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VerifyMagicLinkQuery {
    /// The magic link token from the email.
    pub token: String,
}
