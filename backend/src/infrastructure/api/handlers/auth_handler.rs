use axum::{
    Json,
    extract::State,
    http::header::{self, HeaderMap},
};
use std::sync::Arc;
use std::time::Duration;

use crate::{
    application::{
        dto::{
            AuthResponse, InitiateRegistrationRequest, InitiateRegistrationResponse, LoginRequest,
            LogoutResponse, UserResponse,
        },
        ports::TurnstileService,
        use_cases::{
            GetCurrentUserUseCase, LoginUserUseCase, RefreshTokenUseCase, RegisterOutcome,
            RegisterUserUseCase, RevokeTokenUseCase, SendVerificationCodeUseCase,
        },
    },
    domain::errors::DomainError,
    infrastructure::api::{
        cookies::{clear_csrf_cookie, clear_jwt_cookie, clear_refresh_cookie},
        error::AppError,
        extractors::ValidatedJson,
        handlers::{
            session::{build_session, extract_refresh_token_cookie, require_device_fingerprint},
            verify_turnstile,
        },
        middleware::AuthUser,
    },
    infrastructure::crypto::Crypto,
};

/// How long registration will wait for the verification email to be handed to
/// the relay, retries included.
///
/// Registration sends inline because the caller needs to know whether the code
/// is actually coming — but the transport's full retry sequence (~24.9s) is far
/// too much of the 30s request budget to spend on one dependency, with a
/// Turnstile call and an MX lookup already ahead of it in this request. This cap
/// leaves a fast-failing blip room to be retried and still succeed, while an
/// unreachable relay is written off in one attempt's worth of waiting.
const REGISTRATION_SEND_BUDGET: Duration = Duration::from_secs(10);

pub struct AuthHandlerDeps {
    pub register_use_case: Arc<RegisterUserUseCase>,
    pub login_use_case: Arc<LoginUserUseCase>,
    pub get_current_user_use_case: Arc<GetCurrentUserUseCase>,
    pub refresh_token_use_case: Arc<RefreshTokenUseCase>,
    pub revoke_token_use_case: Arc<RevokeTokenUseCase>,
    pub send_verification_code_use_case: Arc<SendVerificationCodeUseCase>,
    pub turnstile_service: Arc<dyn TurnstileService>,
    pub crypto: Arc<Crypto>,
}

/// Initiate user registration (email-only step)
///
/// This is step 1 of the email-first signup flow:
/// 1. User provides email → verification code sent (this endpoint)
/// 2. User verifies email → signup token issued
/// 3. User sets password using signup token → account complete
#[utoipa::path(
    post,
    path = "/register",
    tag = "Authentication",
    request_body = InitiateRegistrationRequest,
    responses(
        (status = 200, description = "Verification code sent; for an email that already has an account, its owner is notified instead (same response, so registration can't be used to check for accounts)", body = InitiateRegistrationResponse),
        (status = 400, description = "Invalid input"),
        (status = 502, description = "The verification email could not be delivered; the code was not sent, and resending is available immediately")
    )
)]
/// # Errors
///
/// Returns an error if registration fails due to validation, duplicate email, or service errors.
#[tracing::instrument(name = "handler.register", skip_all)]
pub async fn register(
    State(deps): State<Arc<AuthHandlerDeps>>,
    ValidatedJson(request): ValidatedJson<InitiateRegistrationRequest>,
) -> Result<Json<InitiateRegistrationResponse>, AppError> {
    // Verify Turnstile token (required when bot protection is enabled)
    verify_turnstile(
        deps.turnstile_service.as_ref(),
        request.turnstile_token.as_deref(),
    )
    .await?;

    // Creates a user without a password, reuses an incomplete signup, or finds
    // a completed account. All three get the same response below, so the
    // endpoint can't be used to check whether an email is registered.
    match deps.register_use_case.execute(&request.email).await? {
        // Issue the code and deliver it, both inside the request, bounded by
        // REGISTRATION_SEND_BUDGET.
        //
        // The failure is reported rather than logged and swallowed. The email is
        // the sole carrier of the code, and the flow has no password or session to
        // fall back on, so "registered" without it is a dead end: the caller sits
        // on a "check your email" screen forever. A 502 at least tells them to
        // try again.
        RegisterOutcome::PendingVerification(user) => {
            deps.send_verification_code_use_case
                .issue_and_deliver(&user, REGISTRATION_SEND_BUDGET)
                .await
                .map_err(|e| {
                    tracing::error!(user_id = %user.id(), error = %e, "Registration could not deliver the verification code");
                    DomainError::from(e)
                })?;
        }
        // Tell the owner instead of sending a code. Sent inline under the same
        // budget, and a failure maps to the same 502, so neither the timing nor
        // the status separates this branch from a new signup.
        RegisterOutcome::AlreadyRegistered(user) => {
            let sent = tokio::time::timeout(
                REGISTRATION_SEND_BUDGET,
                deps.register_use_case.notify_existing(&user),
            )
            .await
            .unwrap_or_else(|_elapsed| {
                Err(DomainError::ExternalService(format!(
                    "account-exists notice not sent within {REGISTRATION_SEND_BUDGET:?}"
                )))
            });
            if let Err(e) = sent {
                tracing::error!(user_id = %user.id(), error = %e, "Registration could not deliver the account-exists notice");
                return Err(DomainError::ExternalService(e.to_string()).into());
            }
        }
    }

    Ok(Json(InitiateRegistrationResponse {
        success: true,
        message: "Verification code sent to your email".to_string(),
    }))
}

/// Login with email and password
#[utoipa::path(
    post,
    path = "/login",
    tag = "Authentication",
    request_body = LoginRequest,
    responses(
        (status = 200, description = "Login successful", body = AuthResponse),
        (status = 401, description = "Invalid credentials")
    )
)]
/// # Errors
///
/// Returns an error if login fails due to invalid credentials, validation errors, or service errors.
#[tracing::instrument(name = "handler.login", skip_all)]
pub async fn login(
    State(deps): State<Arc<AuthHandlerDeps>>,
    headers: HeaderMap,
    ValidatedJson(request): ValidatedJson<LoginRequest>,
) -> Result<(HeaderMap, Json<AuthResponse>), AppError> {
    // Verify Turnstile token (required when bot protection is enabled)
    verify_turnstile(
        deps.turnstile_service.as_ref(),
        request.turnstile_token.as_deref(),
    )
    .await?;

    // Extract device fingerprint
    let device_fingerprint = require_device_fingerprint(&headers)?;

    // Verify credentials (timing-equalized across all failure cases).
    let user = deps
        .login_use_case
        .execute(&request.email, &request.password)
        .await?;

    // Create refresh token, then issue the session (JWT + CSRF + cookies).
    let refresh_token = deps
        .refresh_token_use_case
        .create_for_user(user.id(), &device_fingerprint)
        .await?;
    let session = build_session(&deps.crypto, &user, Some(&refresh_token))?;

    Ok((
        session.headers,
        Json(AuthResponse {
            expires_at: session.expires_at,
            csrf_token: session.csrf_token,
            user: UserResponse::from(user),
        }),
    ))
}

/// Refresh the JWT access token using a refresh token
#[utoipa::path(
    post,
    path = "/refresh",
    tag = "Authentication",
    responses(
        (status = 200, description = "Token refreshed successfully", body = AuthResponse),
        (status = 401, description = "Invalid or expired refresh token")
    )
)]
/// # Errors
///
/// Returns an error if the refresh token is missing, invalid, expired, or service errors occur.
#[tracing::instrument(name = "handler.refresh", skip_all)]
pub async fn refresh(
    State(deps): State<Arc<AuthHandlerDeps>>,
    headers: HeaderMap,
) -> Result<(HeaderMap, Json<AuthResponse>), AppError> {
    // The refresh token is always read from the httpOnly cookie.
    let refresh_token =
        extract_refresh_token_cookie(&headers).ok_or_else(AppError::token_missing)?;

    // Extract device fingerprint
    let device_fingerprint = require_device_fingerprint(&headers)?;

    // Execute use case (validates token + fingerprint, rotates token)
    let result = deps
        .refresh_token_use_case
        .execute(&refresh_token, &device_fingerprint)
        .await?;

    // Issue a fresh session around the rotated refresh token.
    let session = build_session(&deps.crypto, &result.user, Some(&result.new_refresh_token))?;

    Ok((
        session.headers,
        Json(AuthResponse {
            expires_at: session.expires_at,
            csrf_token: session.csrf_token,
            user: UserResponse::from(result.user),
        }),
    ))
}

/// Logout and clear auth cookies
#[utoipa::path(
    post,
    path = "/logout",
    tag = "Authentication",
    responses(
        (status = 200, description = "Logout successful", body = LogoutResponse)
    )
)]
#[tracing::instrument(name = "handler.logout", skip_all)]
pub async fn logout(
    State(deps): State<Arc<AuthHandlerDeps>>,
    headers: HeaderMap,
) -> (HeaderMap, Json<LogoutResponse>) {
    // Try to revoke the refresh token if present
    if let Some(refresh_token) = extract_refresh_token_cookie(&headers) {
        // Ignore errors during revocation - we're logging out anyway
        let _ = deps.revoke_token_use_case.execute(&refresh_token).await;
    }

    // Clear all cookies
    let mut response_headers = HeaderMap::new();
    response_headers.insert(header::SET_COOKIE, clear_jwt_cookie());
    response_headers.append(header::SET_COOKIE, clear_refresh_cookie());
    response_headers.append(header::SET_COOKIE, clear_csrf_cookie());

    (response_headers, Json(LogoutResponse { success: true }))
}

/// Get current authenticated user
#[utoipa::path(
    get,
    path = "/me",
    tag = "Authentication",
    responses(
        (status = 200, description = "Current user info", body = UserResponse),
        (status = 401, description = "Not authenticated")
    ),
    security(
        ("bearer_auth" = [])
    )
)]
/// # Errors
///
/// Returns an error if the user is not found or service errors occur.
#[tracing::instrument(name = "handler.me", skip_all)]
pub async fn me(
    State(deps): State<Arc<AuthHandlerDeps>>,
    auth_user: AuthUser,
) -> Result<Json<UserResponse>, AppError> {
    let current = deps
        .get_current_user_use_case
        .execute(&auth_user.user_id)
        .await?
        .ok_or_else(|| AppError::token_invalid("user not found"))?;

    let oauth_providers = current
        .oauth_providers
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();

    Ok(Json(UserResponse::from_user_with_oauth(
        current.user,
        oauth_providers,
    )))
}

#[cfg(test)]
mod tests {
    use super::REGISTRATION_SEND_BUDGET;
    use std::time::Duration;

    /// `DEFAULT_REQUEST_TIMEOUT` in `infrastructure/api/routes.rs`. Keep in step
    /// if either moves.
    const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

    /// `SEND_TIMEOUT` in `infrastructure/services/email_service.rs` — the cap on
    /// a single SMTP attempt. Keep in step if either moves.
    const SEND_TIMEOUT: Duration = Duration::from_secs(8);

    /// Registration awaits the verification email so it can report a failure
    /// instead of returning a hollow success, which only works if the wait
    /// cannot itself exhaust the request. Turnstile and the MX lookup run before
    /// it in the same handler, so leave most of the budget for them.
    #[test]
    fn registration_send_budget_leaves_room_for_the_rest_of_the_request() {
        assert!(
            REGISTRATION_SEND_BUDGET * 2 < DEFAULT_REQUEST_TIMEOUT,
            "send budget ({REGISTRATION_SEND_BUDGET:?}) should be well under DEFAULT_REQUEST_TIMEOUT ({DEFAULT_REQUEST_TIMEOUT:?})"
        );
    }

    /// The budget is useless if it cannot cover a single SMTP attempt: every
    /// send would be cut off mid-flight and reported as a delivery failure even
    /// against a healthy relay.
    #[test]
    fn registration_send_budget_covers_at_least_one_smtp_attempt() {
        assert!(
            REGISTRATION_SEND_BUDGET > SEND_TIMEOUT,
            "send budget ({REGISTRATION_SEND_BUDGET:?}) must exceed one attempt ({SEND_TIMEOUT:?})"
        );
    }
}
