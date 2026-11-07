use argon2::PasswordHasher as Argon2PasswordHasher;
use argon2::password_hash::{PasswordHash, PasswordVerifier, SaltString};
use axum::{
    Json,
    extract::State,
    http::header::{self, HeaderMap},
};
use rand_core::OsRng;
use std::sync::Arc;
use std::time::Duration;

use crate::{
    application::{
        dto::{
            AuthResponse, InitiateRegistrationRequest, InitiateRegistrationResponse, LoginRequest,
            UserResponse,
        },
        ports::{MxValidator, OAuthAccountRepository, TurnstileService},
        use_cases::{
            GetCurrentUserUseCase, LoginUserUseCase, RefreshTokenUseCase, RegisterUserUseCase,
            RevokeTokenUseCase, SendVerificationCodeUseCase,
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
    shared::crypto::Crypto,
};

/// How long registration will wait for the verification email to be handed to
/// the relay, retries included.
///
/// Registration sends inline because the caller needs to know whether the code
/// is actually coming — but the transport's full retry sequence (~24.9s) is far
/// too much of the 30s request budget to spend on one dependency, with a
/// Turnstile call and an MX lookup already ahead of it in this handler. This cap
/// leaves a fast-failing blip room to be retried and still succeed, while an
/// unreachable relay is written off in one attempt's worth of waiting.
const REGISTRATION_SEND_BUDGET: Duration = Duration::from_secs(10);

/// Returns a stable dummy Argon2 hash, computed once with the configured
/// parameters. Used to equalize login timing for unknown emails.
fn dummy_password_hash(crypto: &Crypto) -> &'static str {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    DUMMY.get_or_init(|| {
        let salt = SaltString::generate(&mut OsRng);
        crypto
            .argon2()
            .hash_password(b"timing-equalization-dummy", &salt)
            .expect("dummy password hash should always compute")
            .to_string()
    })
}

/// Run an Argon2 verification against a dummy hash and discard the result.
///
/// Called on the unknown-email login path so its response time matches that of
/// a real (wrong-password) verification, preventing user enumeration by timing.
fn verify_dummy_password(crypto: &Crypto, password: &str) {
    if let Ok(parsed) = PasswordHash::new(dummy_password_hash(crypto)) {
        let _ = crypto
            .argon2()
            .verify_password(password.as_bytes(), &parsed);
    }
}

pub struct AuthHandlerDeps {
    pub register_use_case: Arc<RegisterUserUseCase>,
    pub login_use_case: Arc<LoginUserUseCase>,
    pub get_current_user_use_case: Arc<GetCurrentUserUseCase>,
    pub refresh_token_use_case: Arc<RefreshTokenUseCase>,
    pub revoke_token_use_case: Arc<RevokeTokenUseCase>,
    pub send_verification_code_use_case: Arc<SendVerificationCodeUseCase>,
    pub turnstile_service: Arc<dyn TurnstileService>,
    pub mx_validator: Arc<dyn MxValidator>,
    pub oauth_account_repo: Arc<dyn OAuthAccountRepository>,
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
    path = "/api/v1/auth/register",
    tag = "Authentication",
    request_body = InitiateRegistrationRequest,
    responses(
        (status = 200, description = "Verification code sent", body = InitiateRegistrationResponse),
        (status = 400, description = "Invalid input"),
        (status = 409, description = "Email already exists (user has completed signup)"),
        (status = 502, description = "The verification email could not be delivered; the code was not sent, and resending is available immediately")
    )
)]
/// # Errors
///
/// Returns an error if registration fails due to validation, duplicate email, or service errors.
#[tracing::instrument(name = "handler.register", skip(deps, request), fields(email = %request.email))]
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

    // Validate email domain has MX records (log warning if check fails, don't block)
    let email_domain = request.email.split('@').nth(1).unwrap_or("");
    match deps.mx_validator.has_mx_record(email_domain).await {
        Ok(true) => {}
        Ok(false) => {
            tracing::warn!(email = %request.email, domain = %email_domain, "MX record check failed - domain may not receive emails");
            // Don't block - let them try (typos will just not receive the code)
        }
        Err(e) => {
            tracing::warn!(email = %request.email, domain = %email_domain, error = %e, "MX validation error");
            // Don't block on MX check failures
        }
    }

    // Execute use case (creates user without password, or reuses existing incomplete signup)
    let user = deps.register_use_case.execute(&request.email).await?;

    // Record successful registration

    // Issue the code and deliver it, both inside the request, bounded by
    // REGISTRATION_SEND_BUDGET.
    //
    // The failure is reported rather than logged and swallowed. The email is the
    // sole carrier of the code, and the flow has no password or session to fall
    // back on, so "registered" without it is a dead end: the caller sits on a
    // "check your email" screen forever. A 502 at least tells them to try again.
    deps.send_verification_code_use_case
        .issue_and_deliver(&user, REGISTRATION_SEND_BUDGET)
        .await
        .map_err(|e| {
            tracing::error!(user_id = %user.id(), error = %e, "Registration could not deliver the verification code");
            DomainError::from(e)
        })?;

    Ok(Json(InitiateRegistrationResponse {
        success: true,
        message: "Verification code sent to your email".to_string(),
    }))
}

/// Login with email and password
#[utoipa::path(
    post,
    path = "/api/v1/auth/login",
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
#[tracing::instrument(name = "handler.login", skip(deps, request, headers), fields(email = %request.email))]
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

    // Execute use case to find user
    let Some(user) = deps.login_use_case.execute(&request.email).await? else {
        // Perform a dummy Argon2 verification so the response time for an
        // unknown email matches that of a wrong password, preventing user
        // enumeration via timing.
        verify_dummy_password(&deps.crypto, &request.password);
        return Err(AppError::invalid_credentials());
    };

    // A password-less user is one of two things: mid-signup (email registered
    // but no password set yet) or an OAuth-only account (registered via a
    // provider, never set a password). These need different answers, so don't
    // conflate them: an OAuth-only account is *complete* — it just can't log in
    // by password — whereas an incomplete signup must be finished.
    if !user.has_password() {
        // Equalize timing with the password-verification path so an attacker
        // can't distinguish these states from a wrong password by response time
        // (the unknown-email path does the same dummy verify).
        verify_dummy_password(&deps.crypto, &request.password);

        let has_oauth = !deps
            .oauth_account_repo
            .find_by_user_id(user.id())
            .await?
            .is_empty();
        if has_oauth {
            // OAuth-only account: must authenticate via the provider. Return the
            // generic invalid-credentials error rather than disclosing that the
            // account exists and is provider-linked.
            return Err(AppError::invalid_credentials());
        }
        return Err(AppError::SignupIncomplete {
            email: user.email().as_str().to_string(),
        });
    }

    // Verify password with Argon2. `has_password()` guaranteed Some above; the
    // fallback keeps this total without panicking.
    let password_hash = user
        .password_hash()
        .ok_or_else(AppError::invalid_credentials)?;
    let parsed_hash =
        PasswordHash::new(password_hash).map_err(|_| AppError::invalid_credentials())?;
    deps.crypto
        .argon2()
        .verify_password(request.password.as_bytes(), &parsed_hash)
        .map_err(|_| AppError::invalid_credentials())?;

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
    path = "/api/v1/auth/refresh",
    tag = "Authentication",
    responses(
        (status = 200, description = "Token refreshed successfully", body = AuthResponse),
        (status = 401, description = "Invalid or expired refresh token")
    )
)]
/// # Errors
///
/// Returns an error if the refresh token is missing, invalid, expired, or service errors occur.
#[tracing::instrument(name = "handler.refresh", skip(deps, headers))]
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
    path = "/api/v1/auth/logout",
    tag = "Authentication",
    responses(
        (status = 200, description = "Logout successful")
    )
)]
#[tracing::instrument(name = "handler.logout", skip(deps, headers))]
pub async fn logout(
    State(deps): State<Arc<AuthHandlerDeps>>,
    headers: HeaderMap,
) -> (HeaderMap, Json<serde_json::Value>) {
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

    (
        response_headers,
        Json(serde_json::json!({ "success": true })),
    )
}

/// Get current authenticated user
#[utoipa::path(
    get,
    path = "/api/v1/auth/me",
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
#[tracing::instrument(name = "handler.me", skip(deps))]
pub async fn me(
    State(deps): State<Arc<AuthHandlerDeps>>,
    auth_user: AuthUser,
) -> Result<Json<UserResponse>, AppError> {
    // Both lookups key off the same authenticated user id and are independent, so
    // run them concurrently to collapse two DB round-trips into one wall-clock
    // wait on this hot (per-page-load) endpoint.
    let (user, oauth_accounts) = tokio::try_join!(
        deps.get_current_user_use_case.execute(&auth_user.user_id),
        deps.oauth_account_repo.find_by_user_id(&auth_user.user_id),
    )?;

    let user = user.ok_or_else(|| AppError::token_invalid("user not found"))?;

    let oauth_providers: Vec<String> = oauth_accounts
        .iter()
        .map(|a| a.provider().as_str().to_string())
        .collect();

    Ok(Json(UserResponse::from_user_with_oauth(
        user,
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
