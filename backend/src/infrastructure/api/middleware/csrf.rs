//! CSRF protection middleware using the double-submit cookie pattern.
//!
//! This middleware validates that state-changing requests (POST, PUT, PATCH, DELETE)
//! include a matching CSRF token in both the `X-CSRF-Token` header and `csrf_token` cookie.
//!
//! # Excluded Paths
//!
//! The following paths are excluded from CSRF validation:
//! - `/api/v1/auth/login` - Login doesn't have a token yet
//! - `/api/v1/auth/register` - Registration doesn't have a token yet
//! - `/api/v1/auth/refresh` - Refresh issues new tokens
//! - `/api/v1/auth/logout` - Force-logout CSRF is low-harm; accepted tradeoff
//! - `/api/v1/auth/oauth/*` - OAuth callbacks come from external providers
//! - `/api/v1/auth/verify-email` - Email verification uses signed codes
//! - `/api/v1/auth/resend-verification` - Uses rate limiting, no session
//! - `/api/v1/auth/forgot-password` - No session context
//! - `/api/v1/auth/verify-reset-code` - Uses signed codes
//! - `/api/v1/auth/reset-password` - Uses signed codes
//! - `/api/v1/auth/complete-signup` - Uses signed signup token, no session yet

use std::sync::Arc;

use axum::{extract::Request, http::Method, middleware::Next, response::Response};

use crate::infrastructure::api::cookies::extract_cookie;
use crate::infrastructure::api::error::AppError;
use crate::infrastructure::api::middleware::auth::ensure_authenticated;
use crate::infrastructure::api::verify_csrf_token;
use crate::shared::crypto::Crypto;

/// Header name for the CSRF token.
pub const CSRF_TOKEN_HEADER: &str = "x-csrf-token";

/// Cookie name for the CSRF token.
pub const CSRF_TOKEN_COOKIE: &str = "csrf_token";

/// Paths excluded from CSRF validation.
const EXCLUDED_PATHS: &[&str] = &[
    "/auth/login",
    "/auth/register",
    "/auth/refresh",
    "/auth/logout",
    "/auth/verify-email",
    "/auth/resend-verification",
    "/auth/forgot-password",
    "/auth/verify-reset-code",
    "/auth/reset-password",
    "/auth/complete-signup",
    // Unauthenticated pre-login endpoint: sets the device-fingerprint cookie
    // before any session (and therefore any CSRF token) exists.
    "/auth/oauth/device-fingerprint",
];

/// Check if a path should be excluded from CSRF validation.
fn is_excluded_path(path: &str) -> bool {
    let path = path.strip_prefix("/api/v1").unwrap_or(path);

    // Exact matches only. OAuth start/callback are GET (never CSRF-checked),
    // so they need no exemption. Authenticated, state-changing OAuth routes
    // such as `DELETE /auth/oauth/{provider}/unlink` MUST stay protected — a
    // blanket `/auth/oauth/` prefix exemption would expose them to CSRF.
    EXCLUDED_PATHS.contains(&path)
}

/// CSRF protection middleware.
///
/// Validates CSRF tokens for state-changing requests (POST, PUT, PATCH, DELETE).
///
/// # Errors
///
/// Returns 403 Forbidden if:
/// - The request is a state-changing method and the path is not excluded
/// - The `X-CSRF-Token` header is missing or empty
/// - The `csrf_token` cookie is missing
/// - The header and cookie tokens don't match
/// - The token signature is invalid or expired
/// - The token is not bound to the authenticated principal
#[tracing::instrument(name = "middleware.csrf", skip(req, next), fields(method = %req.method(), path = %req.uri().path()))]
pub async fn csrf_middleware(mut req: Request, next: Next) -> Result<Response, AppError> {
    let method = req.method();
    let path = req.uri().path();

    // Only check state-changing methods
    if !matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) {
        return Ok(next.run(req).await);
    }

    // Skip excluded paths
    if is_excluded_path(path) {
        return Ok(next.run(req).await);
    }

    // The crypto handle is injected as a request extension by the router's
    // `Extension(Arc<Crypto>)` layer; it is always present in a built router.
    let crypto = req
        .extensions()
        .get::<Arc<Crypto>>()
        .cloned()
        .ok_or_else(|| AppError::Internal("crypto handle not injected".into()))?;

    // Extract CSRF token from header (owned, so we can mutably borrow `req` below).
    let header_token = req
        .headers()
        .get(CSRF_TOKEN_HEADER)
        .and_then(|h| h.to_str().ok())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(AppError::csrf_invalid)?;

    // Extract CSRF token from cookie
    let cookie_token =
        extract_cookie(req.headers(), "csrf_token").ok_or_else(AppError::csrf_invalid)?;

    // Verify header and cookie match (double-submit validation)
    if header_token != cookie_token {
        return Err(AppError::csrf_invalid());
    }

    // Bind verification to the authenticated principal. Every non-excluded,
    // state-changing route is behind auth, so a missing/invalid session token
    // here means we can't establish the owner — fail closed. This stops a valid
    // token minted for one user from satisfying another user's request.
    //
    // Verifying here also populates the `AuthUser` extension, which the inner
    // `auth_middleware` reuses so the JWT is verified only once per request.
    let user =
        ensure_authenticated(crypto.decoding_key(), &mut req).ok_or_else(AppError::csrf_invalid)?;

    // Verify the token signature, expiration, and user binding.
    verify_csrf_token(
        &header_token,
        crypto.csrf_secret(),
        &user.user_id.to_string(),
    )?;

    Ok(next.run(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::StatusCode, middleware, routing::post};
    use tower::ServiceExt;

    #[test]
    fn test_excluded_paths() {
        assert!(is_excluded_path("/api/v1/auth/login"));
        assert!(is_excluded_path("/api/v1/auth/register"));
        assert!(is_excluded_path("/api/v1/auth/refresh"));
        assert!(is_excluded_path("/api/v1/auth/logout"));
        assert!(is_excluded_path("/api/v1/auth/verify-email"));
        assert!(is_excluded_path("/api/v1/auth/resend-verification"));
        assert!(is_excluded_path("/api/v1/auth/forgot-password"));
        assert!(is_excluded_path("/api/v1/auth/verify-reset-code"));
        assert!(is_excluded_path("/api/v1/auth/reset-password"));
        assert!(is_excluded_path("/api/v1/auth/complete-signup"));
        // Unauthenticated pre-login fingerprint endpoint is exempt.
        assert!(is_excluded_path("/api/v1/auth/oauth/device-fingerprint"));

        // Not excluded. OAuth start/callback are GET (never CSRF-checked), so
        // they are intentionally absent from the exemption list. The
        // authenticated `unlink` DELETE MUST stay protected.
        assert!(!is_excluded_path("/api/v1/auth/oauth/google/unlink"));
        assert!(!is_excluded_path("/api/v1/auth/oauth/google"));
        assert!(!is_excluded_path("/api/v1/auth/oauth/google/callback"));
        assert!(!is_excluded_path("/api/v1/items"));
        assert!(!is_excluded_path("/api/v1/auth/me"));
        assert!(!is_excluded_path("/api/v1/users/profile"));
    }

    #[tokio::test]
    async fn nested_register_route_is_excluded_from_csrf() {
        let api = Router::new()
            .route("/auth/register", post(|| async { StatusCode::OK }))
            .route_layer(middleware::from_fn(csrf_middleware));
        let router = Router::new().nest("/api/v1", api);

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/api/v1/auth/register")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
