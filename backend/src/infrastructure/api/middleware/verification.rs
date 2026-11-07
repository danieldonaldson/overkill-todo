//! Email verification middleware.
//!
//! Blocks access to protected routes for users who haven't verified their email.

use axum::{
    Json,
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;

use super::AuthUser;

/// Middleware that requires email verification.
///
/// Returns 403 Forbidden with `email_not_verified` code if the user's email is not verified.
/// This middleware must be applied AFTER `auth_middleware` so that `AuthUser` is available.
///
/// # Errors
///
/// Returns a 403 response if the user's email is not verified.
pub async fn require_verified_email(req: Request, next: Next) -> Response {
    // Check if AuthUser is present (set by auth_middleware)
    if let Some(auth_user) = req.extensions().get::<AuthUser>()
        && !auth_user.email_verified
    {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "code": "email_not_verified",
                "message": "Please verify your email to access this resource"
            })),
        )
            .into_response();
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::value_objects::{UserId, UserRole};
    use axum::{
        Router,
        body::Body,
        http::{Request as HttpRequest, StatusCode},
        routing::get,
    };
    use tower::ServiceExt;

    async fn dummy_handler() -> &'static str {
        "OK"
    }

    fn create_test_router() -> Router {
        Router::new()
            .route("/test", get(dummy_handler))
            .layer(axum::middleware::from_fn(require_verified_email))
    }

    #[tokio::test]
    async fn test_verified_user_passes_through() {
        let router = create_test_router();

        let mut request = HttpRequest::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        request.extensions_mut().insert(AuthUser {
            user_id: UserId::new(),
            email_verified: true,
            role: UserRole::User,
            issued_at: 0,
        });

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_unverified_user_gets_403() {
        let router = create_test_router();

        let mut request = HttpRequest::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        request.extensions_mut().insert(AuthUser {
            user_id: UserId::new(),
            email_verified: false,
            role: UserRole::User,
            issued_at: 0,
        });

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["code"], "email_not_verified");
    }

    #[tokio::test]
    async fn test_no_auth_user_passes_through() {
        // If no AuthUser is present (unauthenticated route), middleware should pass through
        let router = create_test_router();

        let request = HttpRequest::builder()
            .uri("/test")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        // Should pass through - the auth middleware will handle authentication
        assert_eq!(response.status(), StatusCode::OK);
    }
}
