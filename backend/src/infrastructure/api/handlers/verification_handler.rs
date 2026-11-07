//! Email verification and password reset handlers.

use argon2::password_hash::{PasswordHasher, SaltString};
use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header::HeaderMap},
    response::{IntoResponse, Redirect, Response},
};
use chrono::{Duration, Utc};
use jsonwebtoken::{Header, encode};
use rand_core::OsRng;
use std::sync::Arc;

use crate::{
    application::{
        dto::{
            AuthResponse, CompleteRegistrationRequest, ForgotPasswordRequest,
            ResendVerificationRequest, ResendVerificationResponse, ResetPasswordRequest,
            UserResponse, VerificationErrorCode, VerificationErrorResponse, VerifyEmailRequest,
            VerifyEmailResponse, VerifyMagicLinkQuery, VerifyResetCodeRequest,
            VerifyResetCodeResponse, validation::sanitize_verification_code,
        },
        ports::TurnstileService,
        use_cases::auth::{
            CompleteSignupUseCase, RefreshTokenUseCase, RequestPasswordResetUseCase,
            ResendVerificationError, ResendVerificationUseCase, ResetPasswordUseCase,
            ValidateResetCodeUseCase, VerifyEmailError, VerifyEmailUseCase, VerifyMagicLinkError,
            VerifyMagicLinkUseCase,
        },
    },
    infrastructure::api::{
        error::AppError,
        extractors::ValidatedJson,
        handlers::{
            session::{build_session, extract_device_fingerprint, require_device_fingerprint},
            verify_turnstile,
        },
    },
    shared::crypto::Crypto,
};

/// JWT claims for signup completion or password reset tokens.
#[derive(Debug, serde::Serialize)]
struct ResetTokenClaims {
    sub: String,
    email: String,
    exp: i64,
    iat: i64,
    typ: String,
}

/// Dependencies for verification-related handlers.
pub struct VerificationHandlerDeps {
    pub verify_email_use_case: Arc<VerifyEmailUseCase>,
    pub verify_magic_link_use_case: Arc<VerifyMagicLinkUseCase>,
    pub resend_verification_use_case: Arc<ResendVerificationUseCase>,
    pub request_password_reset_use_case: Arc<RequestPasswordResetUseCase>,
    pub validate_reset_code_use_case: Arc<ValidateResetCodeUseCase>,
    pub reset_password_use_case: Arc<ResetPasswordUseCase>,
    pub complete_signup_use_case: Arc<CompleteSignupUseCase>,
    pub refresh_token_use_case: Arc<RefreshTokenUseCase>,
    pub turnstile_service: Arc<dyn TurnstileService>,
    /// Frontend base URL for redirects (e.g., `http://localhost:4321` or `https://app.example.com`)
    pub frontend_base_url: String,
    pub crypto: Arc<Crypto>,
}

/// Verify email with code
///
/// For new signups (no password yet): Returns a `signup_token` (30 min expiry) to complete signup.
/// For existing users with password: Returns auth tokens for auto-login.
#[utoipa::path(
    post,
    path = "/api/v1/auth/verify-email",
    tag = "Authentication",
    request_body = VerifyEmailRequest,
    responses(
        (status = 200, description = "Email verified successfully", body = VerifyEmailResponse),
        (status = 400, description = "Invalid or expired code", body = VerificationErrorResponse),
        (status = 404, description = "User not found")
    )
)]
#[tracing::instrument(name = "handler.verify_email", skip(deps, request, headers), fields(email = %request.email))]
pub async fn verify_email(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<VerifyEmailRequest>,
) -> Response {
    // Strip non-digits from the code (handles copy-paste with spaces)
    let code = sanitize_verification_code(&request.code);

    match deps
        .verify_email_use_case
        .execute(&request.email, &code)
        .await
    {
        Ok(user) => {
            // Success - email verified
            issue_tokens_or_signup_token(&deps, &headers, user).await
        }
        Err(VerifyEmailError::AlreadyVerified(user)) => {
            // Already verified - still handle appropriately
            issue_tokens_or_signup_token(&deps, &headers, *user).await
        }
        Err(VerifyEmailError::NotFound(msg)) => {
            let app_error: AppError = VerifyEmailError::NotFound(msg).into();
            app_error.into_response()
        }
        Err(VerifyEmailError::CodeExpired) => {
            let response = VerificationErrorResponse::new(
                "This code has expired. Request a new one below.",
                VerificationErrorCode::CodeExpired,
            );
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
        Err(VerifyEmailError::CodeLocked) => {
            let response = VerificationErrorResponse::new(
                "Too many incorrect attempts. Request a new code below.",
                VerificationErrorCode::CodeLocked,
            )
            .with_attempts_remaining(0);
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
        Err(VerifyEmailError::CodeInvalid { attempts_remaining }) => {
            let response = VerificationErrorResponse::new(
                format!("Incorrect code. {attempts_remaining} attempts remaining."),
                VerificationErrorCode::CodeInvalid,
            )
            .with_attempts_remaining(attempts_remaining);
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
        Err(VerifyEmailError::Internal(msg)) => {
            let app_error: AppError = VerifyEmailError::Internal(msg).into();
            app_error.into_response()
        }
    }
}

/// Signup token validity in minutes.
const SIGNUP_TOKEN_EXPIRY_MINUTES: i64 = 30;

/// Issue appropriate response based on user state after email verification.
///
/// - If user has no password (new signup): returns `signup_token` for completing registration
/// - If user has password (existing user): returns auth tokens for auto-login
async fn issue_tokens_or_signup_token(
    deps: &VerificationHandlerDeps,
    headers: &HeaderMap,
    user: crate::domain::entities::User,
) -> Response {
    // Check if this is a new signup (no password yet)
    if !user.has_password() {
        return issue_signup_token(&deps.crypto, user);
    }

    // User has password - issue auth tokens for auto-login
    issue_auth_tokens(deps, headers, user).await
}

/// Issue a signup token for completing registration (setting password).
fn issue_signup_token(crypto: &Crypto, user: crate::domain::entities::User) -> Response {
    let signup_token = match create_signup_token(crypto, &user) {
        Ok(t) => t,
        Err(e) => {
            let app_error = AppError::Internal(format!("Failed to create signup token: {e}"));
            return app_error.into_response();
        }
    };

    tracing::info!(user_id = %user.id(), "Email verified, signup token issued (no password yet)");

    Json(VerifyEmailResponse {
        user: UserResponse::from(user),
        signup_token: Some(signup_token),
        expires_at: None,
    })
    .into_response()
}

/// Issue auth tokens for a verified user with password.
///
/// If device fingerprint is provided, creates a full session with refresh token.
/// Otherwise returns just the JWT (user will need to re-login for refresh capability).
async fn issue_auth_tokens(
    deps: &VerificationHandlerDeps,
    headers: &HeaderMap,
    user: crate::domain::entities::User,
) -> Response {
    // A device fingerprint upgrades this to a full session with a refresh token;
    // without one the user gets an access-only session and must re-login to
    // obtain refresh capability. (The user has just been verified, so
    // `build_session` mints a `verified=true` JWT from the reloaded entity.)
    let refresh_token = match extract_device_fingerprint(headers) {
        Some(device_fingerprint) => match deps
            .refresh_token_use_case
            .create_for_user(user.id(), &device_fingerprint)
            .await
        {
            Ok(refresh_token) => Some(refresh_token),
            Err(e) => {
                tracing::warn!(error = %e, "Failed to create refresh token during verification");
                // Continue without refresh token - user still gets the access JWT.
                None
            }
        },
        None => None,
    };

    let session = match build_session(&deps.crypto, &user, refresh_token.as_deref()) {
        Ok(session) => session,
        Err(e) => return e.into_response(),
    };

    tracing::info!(user_id = %user.id(), "Email verified, auth tokens issued");

    (
        session.headers,
        Json(VerifyEmailResponse {
            // Mirror the access cookie's lifetime in the body so the client can
            // schedule a proactive refresh (the JWT itself is HttpOnly).
            user: UserResponse::from(user),
            signup_token: None,
            expires_at: Some(session.expires_at),
        }),
    )
        .into_response()
}

/// Resend verification code
#[utoipa::path(
    post,
    path = "/api/v1/auth/resend-verification",
    tag = "Authentication",
    request_body = ResendVerificationRequest,
    responses(
        (status = 200, description = "Verification code sent", body = ResendVerificationResponse),
        (status = 400, description = "Rate limited - try again later", body = VerificationErrorResponse),
        (status = 404, description = "User not found")
    )
)]
#[tracing::instrument(name = "handler.resend_verification", skip(deps, request), fields(email = %request.email))]
pub async fn resend_verification(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    ValidatedJson(request): ValidatedJson<ResendVerificationRequest>,
) -> Response {
    // Verify Turnstile token (required when bot protection is enabled)
    if let Err(e) = verify_turnstile(
        deps.turnstile_service.as_ref(),
        request.turnstile_token.as_deref(),
    )
    .await
    {
        return e.into_response();
    }

    match deps
        .resend_verification_use_case
        .execute(&request.email)
        .await
    {
        Ok(result) => {
            let response = ResendVerificationResponse {
                success: true,
                message: "Verification code sent. Check your email.".into(),
                cooldown_seconds: result.cooldown_seconds,
            };
            Json(response).into_response()
        }
        Err(ResendVerificationError::AlreadyVerified) => {
            let response = VerificationErrorResponse::new(
                "Email is already verified.",
                VerificationErrorCode::AlreadyVerified,
            );
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
        Err(ResendVerificationError::CooldownActive { seconds_remaining }) => {
            let response = VerificationErrorResponse::new(
                "Please wait before requesting a new code.",
                VerificationErrorCode::CooldownActive,
            )
            .with_cooldown_seconds(seconds_remaining);
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
        Err(ResendVerificationError::Locked {
            seconds_until_unlock,
        }) => {
            // The current code is locked from too many failed attempts;
            // resending would reset the counter, so it is refused for a short
            // fixed window (seconds_until_unlock) before a fresh code is allowed.
            let response = VerificationErrorResponse::new(
                "Too many incorrect attempts. Please try again later.",
                VerificationErrorCode::CodeLocked,
            )
            .with_attempts_remaining(0)
            .with_cooldown_seconds(seconds_until_unlock);
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
        }
        Err(ResendVerificationError::NotFound(msg)) => {
            let app_error: AppError = ResendVerificationError::NotFound(msg).into();
            app_error.into_response()
        }
        Err(ResendVerificationError::Internal(msg)) => {
            let app_error: AppError = ResendVerificationError::Internal(msg).into();
            app_error.into_response()
        }
    }
}

/// Request password reset (forgot password)
#[utoipa::path(
    post,
    path = "/api/v1/auth/forgot-password",
    tag = "Authentication",
    request_body = ForgotPasswordRequest,
    responses(
        (status = 200, description = "Reset code sent if email exists"),
        (status = 400, description = "Invalid email format")
    )
)]
/// # Errors
///
/// Returns an error if service errors occur. Always returns success to prevent email enumeration.
#[tracing::instrument(name = "handler.forgot_password", skip(deps, request), fields(email = %request.email))]
pub async fn forgot_password(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    ValidatedJson(request): ValidatedJson<ForgotPasswordRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    // Verify Turnstile token (required when bot protection is enabled)
    verify_turnstile(
        deps.turnstile_service.as_ref(),
        request.turnstile_token.as_deref(),
    )
    .await?;

    // Execute use case - note: we always return success to prevent email enumeration
    // The use case handles the case where the email doesn't exist
    deps.request_password_reset_use_case
        .execute(&request.email)
        .await?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "If an account exists with that email, a reset code has been sent"
    })))
}

/// Verify reset code and get reset token
#[utoipa::path(
    post,
    path = "/api/v1/auth/verify-reset-code",
    tag = "Authentication",
    request_body = VerifyResetCodeRequest,
    responses(
        (status = 200, description = "Code verified, reset token returned", body = VerifyResetCodeResponse),
        (status = 400, description = "Invalid or expired code"),
        (status = 404, description = "User not found")
    )
)]
/// # Errors
///
/// Returns an error if the code is invalid, expired, or service errors occur.
#[tracing::instrument(name = "handler.verify_reset_code", skip(deps, request), fields(email = %request.email))]
pub async fn verify_reset_code(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    ValidatedJson(request): ValidatedJson<VerifyResetCodeRequest>,
) -> Result<Json<VerifyResetCodeResponse>, AppError> {
    let code = sanitize_verification_code(&request.code);

    let reset_token = deps
        .validate_reset_code_use_case
        .execute(&request.email, &code)
        .await?;

    Ok(Json(VerifyResetCodeResponse { reset_token }))
}

/// Reset password with token
///
/// Returns auth tokens for auto-login if device fingerprint is provided.
#[utoipa::path(
    post,
    path = "/api/v1/auth/reset-password",
    tag = "Authentication",
    request_body = ResetPasswordRequest,
    responses(
        (status = 200, description = "Password reset successfully with auto-login", body = AuthResponse),
        (status = 400, description = "Invalid or expired token"),
        (status = 401, description = "Unauthorized - invalid token")
    )
)]
/// # Errors
///
/// Returns an error if the reset token is invalid, expired, missing device fingerprint, or service errors occur.
#[tracing::instrument(name = "handler.reset_password", skip(deps, request, headers))]
pub async fn reset_password(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<ResetPasswordRequest>,
) -> Result<(HeaderMap, Json<AuthResponse>), AppError> {
    // Hash the new password with Argon2id
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = deps
        .crypto
        .argon2()
        .hash_password(request.new_password.as_bytes(), &salt)?
        .to_string();

    // Reset password and get the user
    let user = deps
        .reset_password_use_case
        .execute(&request.reset_token, &password_hash)
        .await?;

    // Extract device fingerprint for auto-login
    let device_fingerprint = require_device_fingerprint(&headers)?;

    // Create refresh token, then issue the session (JWT + CSRF + cookies).
    let refresh_token = deps
        .refresh_token_use_case
        .create_for_user(user.id(), &device_fingerprint)
        .await?;
    let session = build_session(&deps.crypto, &user, Some(&refresh_token))?;

    tracing::info!(user_id = %user.id(), "Password reset with auto-login");

    Ok((
        session.headers,
        Json(AuthResponse {
            expires_at: session.expires_at,
            csrf_token: session.csrf_token,
            user: UserResponse::from(user),
        }),
    ))
}

/// Complete signup by setting password
///
/// Validates the signup token (from email verification) and sets the user's password.
/// Returns auth tokens for auto-login.
///
/// # Errors
///
/// Returns an error if:
/// - The signup token is invalid or expired
/// - The email doesn't match the token
/// - The user already has a password
/// - Password hashing fails
#[utoipa::path(
    post,
    path = "/api/v1/auth/complete-signup",
    tag = "Authentication",
    request_body = CompleteRegistrationRequest,
    responses(
        (status = 200, description = "Signup completed, user logged in", body = AuthResponse),
        (status = 400, description = "Invalid request or password already set"),
        (status = 401, description = "Invalid or expired signup token")
    )
)]
#[tracing::instrument(name = "handler.complete_signup", skip(deps, request, headers), fields(email = %request.email))]
pub async fn complete_signup(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<CompleteRegistrationRequest>,
) -> Result<(HeaderMap, Json<AuthResponse>), AppError> {
    // Hash the password with Argon2id
    let salt = SaltString::generate(&mut OsRng);
    let password_hash = deps
        .crypto
        .argon2()
        .hash_password(request.password.as_bytes(), &salt)?
        .to_string();

    // Complete signup and get the user
    let user = deps
        .complete_signup_use_case
        .execute(&request.signup_token, &request.email, &password_hash)
        .await?;

    // Extract device fingerprint for auto-login
    let device_fingerprint = require_device_fingerprint(&headers)?;

    // Create refresh token, then issue the session (JWT + CSRF + cookies).
    let refresh_token = deps
        .refresh_token_use_case
        .create_for_user(user.id(), &device_fingerprint)
        .await?;
    let session = build_session(&deps.crypto, &user, Some(&refresh_token))?;

    tracing::info!(user_id = %user.id(), "Signup completed with auto-login");

    Ok((
        session.headers,
        Json(AuthResponse {
            expires_at: session.expires_at,
            csrf_token: session.csrf_token,
            user: UserResponse::from(user),
        }),
    ))
}

/// Verify email via magic link (backup to 6-digit code)
///
/// GET endpoint that verifies the email, issues a signup token, and redirects to /set-password.
/// This is a one-click verification for web users.
#[utoipa::path(
    get,
    path = "/api/v1/auth/verify-magic",
    tag = "Authentication",
    params(
        ("token" = String, Query, description = "The magic link token from the email")
    ),
    responses(
        (status = 302, description = "Redirects to /set-password with signup_token and email"),
        (status = 302, description = "Redirects to /login?error=... on error")
    )
)]
#[tracing::instrument(name = "handler.verify_magic", skip(deps, query))]
pub async fn verify_magic(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    Query(query): Query<VerifyMagicLinkQuery>,
) -> Response {
    match deps.verify_magic_link_use_case.execute(&query.token).await {
        Ok(user) => {
            // Success - email verified, issue signup token and redirect
            let signup_token = match create_signup_token(&deps.crypto, &user) {
                Ok(token) => token,
                Err(e) => {
                    let error_str = format!("Failed to create signup token: {e}");
                    let redirect_url = format!(
                        "{}/login?error={}",
                        deps.frontend_base_url,
                        urlencoding::encode(&error_str)
                    );
                    return Redirect::to(&redirect_url).into_response();
                }
            };

            let email_encoded = urlencoding::encode(user.email().as_str());
            let redirect_url = format!(
                "{}/set-password?token={}&email={}",
                deps.frontend_base_url, signup_token, email_encoded
            );

            tracing::info!(user_id = %user.id(), "Email verified via magic link, redirecting to set-password");
            Redirect::to(&redirect_url).into_response()
        }
        Err(VerifyMagicLinkError::AlreadyVerified(user)) => {
            // Already verified - if user has password, redirect to login; otherwise to set-password
            if (*user).has_password() {
                let redirect_url =
                    format!("{}/login?message=email_verified", deps.frontend_base_url);
                Redirect::to(&redirect_url).into_response()
            } else {
                // No password - issue signup token
                let signup_token = match create_signup_token(&deps.crypto, &user) {
                    Ok(token) => token,
                    Err(e) => {
                        let error_str = format!("Failed to create signup token: {e}");
                        let redirect_url = format!(
                            "{}/login?error={}",
                            deps.frontend_base_url,
                            urlencoding::encode(&error_str)
                        );
                        return Redirect::to(&redirect_url).into_response();
                    }
                };

                let email_encoded = urlencoding::encode(user.email().as_str());
                let redirect_url = format!(
                    "{}/set-password?token={}&email={}",
                    deps.frontend_base_url, signup_token, email_encoded
                );
                Redirect::to(&redirect_url).into_response()
            }
        }
        Err(VerifyMagicLinkError::InvalidToken) => {
            let error_msg =
                urlencoding::encode("Invalid or expired magic link. Please request a new code.");
            let redirect_url = format!("{}/verify-email?error={error_msg}", deps.frontend_base_url);
            Redirect::to(&redirect_url).into_response()
        }
        Err(VerifyMagicLinkError::TokenExpired) => {
            let error_msg =
                urlencoding::encode("This magic link has expired. Please request a new code.");
            let redirect_url = format!("{}/verify-email?error={error_msg}", deps.frontend_base_url);
            Redirect::to(&redirect_url).into_response()
        }
        Err(VerifyMagicLinkError::TokenLocked) => {
            let error_msg =
                urlencoding::encode("Too many failed attempts. Please request a new code.");
            let redirect_url = format!("{}/verify-email?error={error_msg}", deps.frontend_base_url);
            Redirect::to(&redirect_url).into_response()
        }
        Err(VerifyMagicLinkError::Internal(msg)) => {
            tracing::error!(error = %msg, "Internal error during magic link verification");
            let error_msg = urlencoding::encode("An error occurred. Please try again.");
            let redirect_url = format!("{}/verify-email?error={error_msg}", deps.frontend_base_url);
            Redirect::to(&redirect_url).into_response()
        }
    }
}

/// Create a signup token for a user (used by magic link handler).
fn create_signup_token(
    crypto: &Crypto,
    user: &crate::domain::entities::User,
) -> Result<String, String> {
    let now = Utc::now();
    let claims = ResetTokenClaims {
        sub: user.id().to_string(),
        email: user.email().as_str().to_string(),
        exp: (now + Duration::minutes(SIGNUP_TOKEN_EXPIRY_MINUTES)).timestamp(),
        iat: now.timestamp(),
        typ: "signup".to_string(),
    };

    let header = Header::new(jsonwebtoken::Algorithm::RS256);
    encode(&header, &claims, crypto.encoding_key()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn sanitize_code_extracts_digits_only() {
        assert_eq!(sanitize_verification_code("123456"), "123456");
    }

    #[test]
    fn sanitize_code_removes_spaces() {
        assert_eq!(sanitize_verification_code("123 456"), "123456");
        assert_eq!(sanitize_verification_code(" 1 2 3 4 5 6 "), "123456");
    }

    #[test]
    fn sanitize_code_removes_dashes() {
        assert_eq!(sanitize_verification_code("123-456"), "123456");
    }

    #[test]
    fn sanitize_code_handles_empty_input() {
        assert_eq!(sanitize_verification_code(""), "");
    }

    #[test]
    fn sanitize_code_handles_no_digits() {
        assert_eq!(sanitize_verification_code("abc-def"), "");
    }

    proptest! {
        #[test]
        fn prop_sanitize_extracts_six_digit_codes(code in "[0-9]{6}") {
            let result = sanitize_verification_code(&code);
            prop_assert_eq!(result, code);
        }

        #[test]
        fn prop_sanitize_removes_spaces_from_valid_code(
            d1 in "[0-9]",
            d2 in "[0-9]",
            d3 in "[0-9]",
            d4 in "[0-9]",
            d5 in "[0-9]",
            d6 in "[0-9]"
        ) {
            let with_spaces = format!("{d1} {d2} {d3} {d4} {d5} {d6}");
            let expected = format!("{d1}{d2}{d3}{d4}{d5}{d6}");
            let result = sanitize_verification_code(&with_spaces);
            prop_assert_eq!(result, expected);
        }

        #[test]
        fn prop_sanitize_removes_dashes_from_valid_code(
            first_half in "[0-9]{3}",
            second_half in "[0-9]{3}"
        ) {
            let with_dash = format!("{first_half}-{second_half}");
            let expected = format!("{first_half}{second_half}");
            let result = sanitize_verification_code(&with_dash);
            prop_assert_eq!(result, expected);
        }

        #[test]
        fn prop_sanitize_result_contains_only_digits(input in "\\PC{0,50}") {
            let result = sanitize_verification_code(&input);
            prop_assert!(result.chars().all(|c| c.is_ascii_digit()));
        }

        #[test]
        fn prop_sanitize_preserves_all_digits(input in "\\PC{0,50}") {
            let expected_digits: String = input.chars().filter(char::is_ascii_digit).collect();
            let result = sanitize_verification_code(&input);
            prop_assert_eq!(result, expected_digits);
        }

        #[test]
        fn prop_sanitize_result_length_equals_digit_count(input in "\\PC{0,50}") {
            let digit_count = input.chars().filter(char::is_ascii_digit).count();
            let result = sanitize_verification_code(&input);
            prop_assert_eq!(result.len(), digit_count);
        }

        #[test]
        fn prop_sanitize_handles_unicode_mixed_with_digits(
            prefix in "[^0-9]{0,10}",
            digits in "[0-9]{6}",
            suffix in "[^0-9]{0,10}"
        ) {
            let input = format!("{prefix}{digits}{suffix}");
            let result = sanitize_verification_code(&input);
            prop_assert_eq!(result, digits);
        }
    }
}
