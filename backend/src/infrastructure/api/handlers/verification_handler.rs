//! Email verification and password reset handlers.

use axum::{
    Json,
    extract::{Query, State},
    http::{StatusCode, header::HeaderMap},
    response::{IntoResponse, Redirect, Response},
};
use std::sync::Arc;

use crate::{
    application::{
        dto::{
            AuthResponse, CompleteRegistrationRequest, ForgotPasswordRequest,
            ResendVerificationRequest, ResendVerificationResponse, ResetPasswordRequest,
            UserResponse, VerificationErrorCode, VerificationErrorResponse, VerifyEmailRequest,
            VerifyEmailResponse, VerifyMagicLinkQuery, VerifyMagicLinkRequest,
            VerifyResetCodeRequest, VerifyResetCodeResponse,
            validation::sanitize_verification_code,
        },
        ports::TurnstileService,
        use_cases::auth::{
            CompleteSignupUseCase, RefreshTokenUseCase, RequestPasswordResetUseCase,
            ResendVerificationError, ResendVerificationUseCase, ResetPasswordUseCase,
            ValidateResetCodeUseCase, VerifyEmailError, VerifyEmailUseCase, VerifyMagicLinkError,
            VerifyMagicLinkUseCase,
        },
    },
    domain::entities::attempt_lockout::RESEND_COOLDOWN_MINUTES,
    infrastructure::api::{
        error::AppError,
        extractors::ValidatedJson,
        handlers::{
            session::{
                build_session, extract_device_fingerprint, require_device_fingerprint,
                wait_until_after_second_of,
            },
            verify_turnstile,
        },
    },
    infrastructure::crypto::Crypto,
};

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
        (status = 400, description = "Invalid or expired code, or email already verified", body = VerificationErrorResponse),
        (status = 404, description = "User not found")
    )
)]
#[tracing::instrument(name = "handler.verify_email", skip_all)]
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
        Err(VerifyEmailError::AlreadyVerified(_)) => {
            // SECURITY: never issue credentials here. The use case returns this
            // *before* checking the code (a verified account has no code left to
            // check), so issuing a session or signup token would let anyone who
            // knows the email log in, or set a password on an OAuth-only account.
            let response = VerificationErrorResponse::new(
                "This email is already verified. Please log in.",
                VerificationErrorCode::AlreadyVerified,
            );
            (StatusCode::BAD_REQUEST, Json(response)).into_response()
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
        return issue_signup_token(deps, user);
    }

    // User has password - issue auth tokens for auto-login
    issue_auth_tokens(deps, headers, user).await
}

/// Issue a signup token for completing registration (setting password).
fn issue_signup_token(
    deps: &VerificationHandlerDeps,
    user: crate::domain::entities::User,
) -> Response {
    let signup_token = match deps.complete_signup_use_case.issue_signup_token(&user) {
        Ok(t) => t,
        Err(e) => return AppError::from(e).into_response(),
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
        (status = 200, description = "Same response whether or not a code was sent (unknown email, already verified, cooldown, and lock are not distinguished)", body = ResendVerificationResponse)
    )
)]
#[tracing::instrument(name = "handler.resend_verification", skip_all)]
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

    // Every outcome that depends on the account — no such user, already
    // verified, inside the cooldown, code locked — gets the same 200 and the
    // full cooldown. Anything else would let this endpoint answer "does this
    // email have an account?". A caller that hit a cooldown or lock just gets no
    // new email, and its timer still keeps it from retrying too early.
    let cooldown_seconds = match deps
        .resend_verification_use_case
        .execute(&request.email)
        .await
    {
        Ok(result) => result.cooldown_seconds,
        Err(
            ResendVerificationError::NotFound(_)
            | ResendVerificationError::AlreadyVerified
            | ResendVerificationError::CooldownActive { .. }
            | ResendVerificationError::Locked { .. },
        ) => RESEND_COOLDOWN_MINUTES * 60,
        Err(ResendVerificationError::Internal(msg)) => {
            let app_error: AppError = ResendVerificationError::Internal(msg).into();
            return app_error.into_response();
        }
    };

    Json(ResendVerificationResponse {
        success: true,
        message: "If this email is awaiting verification, a new code is on its way.".into(),
        cooldown_seconds,
    })
    .into_response()
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
#[tracing::instrument(name = "handler.forgot_password", skip_all)]
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
#[tracing::instrument(name = "handler.verify_reset_code", skip_all)]
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
#[tracing::instrument(name = "handler.reset_password", skip_all)]
pub async fn reset_password(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<ResetPasswordRequest>,
) -> Result<(HeaderMap, Json<AuthResponse>), AppError> {
    // Reset password (hashed in the use case) and get the user
    let reset = deps
        .reset_password_use_case
        .execute(&request.reset_token, &request.new_password)
        .await?;
    let user = reset.user;

    // Extract device fingerprint for auto-login
    let device_fingerprint = require_device_fingerprint(&headers)?;

    // The reset cut off every access token issued up to its cutoff second;
    // this device's new one must come after it.
    wait_until_after_second_of(reset.sessions_valid_after).await;

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
#[tracing::instrument(name = "handler.complete_signup", skip_all)]
pub async fn complete_signup(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<CompleteRegistrationRequest>,
) -> Result<(HeaderMap, Json<AuthResponse>), AppError> {
    // Complete signup (password hashed in the use case) and get the user
    let user = deps
        .complete_signup_use_case
        .execute(&request.signup_token, &request.email, &request.password)
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

/// Legacy magic-link entry point: redirect to the frontend confirmation page.
///
/// Emails used to link here directly, and this GET verified the email on the
/// spot. Mail scanners that prefetch links used the token up before the user
/// clicked, so it no longer has side effects: it only forwards the token, in the
/// URL fragment, to `/verify-magic`, where the user confirms with a button
/// (`POST /api/v1/auth/verify-magic`). Kept so links in emails sent before the
/// change still work; new emails link to the frontend page directly.
#[utoipa::path(
    get,
    path = "/api/v1/auth/verify-magic",
    tag = "Authentication",
    params(
        ("token" = String, Query, description = "The magic link token from the email")
    ),
    responses(
        (status = 303, description = "Redirects to the /verify-magic confirmation page with the token in the fragment")
    )
)]
#[tracing::instrument(name = "handler.verify_magic_redirect", skip_all)]
pub async fn verify_magic_redirect(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    Query(query): Query<VerifyMagicLinkQuery>,
) -> Redirect {
    Redirect::to(&format!(
        "{}/verify-magic#token={}",
        deps.frontend_base_url.trim_end_matches('/'),
        urlencoding::encode(&query.token)
    ))
}

/// Confirm a magic link (backup to the 6-digit code).
///
/// Answers exactly like `verify-email`: a `signup_token` in the body when the
/// account has no password yet, otherwise a session (cookies) for auto-login.
/// Nothing here goes into a URL, so the signup token never reaches browser
/// history or access logs.
#[utoipa::path(
    post,
    path = "/api/v1/auth/verify-magic",
    tag = "Authentication",
    request_body = VerifyMagicLinkRequest,
    responses(
        (status = 200, description = "Email verified", body = VerifyEmailResponse),
        (status = 400, description = "Invalid, expired or locked link, or the account is already verified and has a password", body = VerificationErrorResponse)
    )
)]
#[tracing::instrument(name = "handler.verify_magic", skip_all)]
pub async fn verify_magic(
    State(deps): State<Arc<VerificationHandlerDeps>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<VerifyMagicLinkRequest>,
) -> Response {
    match deps
        .verify_magic_link_use_case
        .execute(&request.token)
        .await
    {
        Ok(user) => issue_tokens_or_signup_token(&deps, &headers, user).await,
        // Holding the link proves ownership of the address, so an account that
        // is verified but still has no password (OAuth-only, or a signup that
        // stopped after verifying) may set one. With a password there is
        // nothing to do: tell them to log in, and never issue a session here.
        Err(VerifyMagicLinkError::AlreadyVerified(user)) if !user.has_password() => {
            issue_signup_token(&deps, *user)
        }
        Err(VerifyMagicLinkError::AlreadyVerified(_)) => verification_error(
            "This email is already verified. Please log in.",
            VerificationErrorCode::AlreadyVerified,
        ),
        Err(VerifyMagicLinkError::InvalidToken) => verification_error(
            "This link is invalid or has already been used. Request a new code.",
            VerificationErrorCode::CodeInvalid,
        ),
        Err(VerifyMagicLinkError::TokenExpired) => verification_error(
            "This link has expired. Request a new code.",
            VerificationErrorCode::CodeExpired,
        ),
        Err(VerifyMagicLinkError::TokenLocked) => verification_error(
            "Too many failed attempts. Request a new code.",
            VerificationErrorCode::CodeLocked,
        ),
        Err(VerifyMagicLinkError::Internal(msg)) => AppError::Internal(msg).into_response(),
    }
}

fn verification_error(message: &str, code: VerificationErrorCode) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(VerificationErrorResponse::new(message, code)),
    )
        .into_response()
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
