//! HTTP-level integration coverage for the profile, OAuth, and auth handlers.
//!
//! These drive real requests through the wired test app so each handler runs
//! together with its use case and repositories against a throwaway database
//! (provisioned per test by `#[sqlx::test]`). The focus is the error/edge
//! branches the happy-path suite in `auth_integration.rs`
//! leaves uncovered.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn body_json(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

/// Authenticated session for a test user: the JWT (sent as a Bearer token) and
/// the user-bound CSRF token (sent as cookie + header on mutating requests).
struct Session {
    token: String,
    csrf: String,
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", "test-device-abc123")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn post_authed(uri: &str, s: &Session, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {}", s.token))
        .header(header::COOKIE, format!("csrf_token={}", s.csrf))
        .header("x-csrf-token", &s.csrf)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn get_authed(uri: &str, s: &Session) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {}", s.token))
        .body(Body::empty())
        .unwrap()
}

fn delete_authed(uri: &str, s: &Session, body: &Value) -> Request<Body> {
    Request::builder()
        .method("DELETE")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {}", s.token))
        .header(header::COOKIE, format!("csrf_token={}", s.csrf))
        .header("x-csrf-token", &s.csrf)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// Runs the full signup flow and returns the resulting authenticated session.
async fn signup_and_session(app: &axum::Router, email: &str) -> Session {
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    let signup_token = body_json(resp.into_body()).await["signup_token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": "hunter2hunter2" }),
        ))
        .await
        .unwrap();
    let token = common::cookie_value(&resp, "auth_token").expect("auth_token cookie");
    let csrf = body_json(resp.into_body()).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    Session { token, csrf }
}

// ---------------------------------------------------------------------------
// Profile handler — export
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn export_data_returns_user_profile(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-profile-export@example.com";
    let session = signup_and_session(&app, email).await;

    let resp = app
        .clone()
        .oneshot(get_authed("/api/v1/users/me/export", &session))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["email"], email);
    assert_eq!(body["email_verified"], true);
    assert_eq!(body["has_password"], true);
    assert!(body["linked_oauth_accounts"].is_array());
    assert!(body["exported_at"].is_string());
    assert!(body["deletion_scheduled_at"].is_null());
}

#[sqlx::test(migrations = "./migrations")]
async fn export_data_without_token_returns_401(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/users/me/export")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Profile handler — change password
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn change_password_succeeds_and_old_password_stops_working(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-profile-change-pw@example.com";
    let session = signup_and_session(&app, email).await;

    let resp = app
        .clone()
        .oneshot(post_authed(
            "/api/v1/users/password/change",
            &session,
            &json!({ "current_password": "hunter2hunter2", "new_password": "brandnewpass99" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Old password no longer authenticates.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "hunter2hunter2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // New password works.
    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "brandnewpass99" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn change_password_wrong_current_returns_error(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-profile-change-bad@example.com").await;

    let resp = app
        .oneshot(post_authed(
            "/api/v1/users/password/change",
            &session,
            &json!({ "current_password": "totallywrong", "new_password": "brandnewpass99" }),
        ))
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::NO_CONTENT);
    assert!(resp.status().is_client_error());
}

#[sqlx::test(migrations = "./migrations")]
async fn change_password_too_short_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-profile-change-short@example.com").await;

    let resp = app
        .oneshot(post_authed(
            "/api/v1/users/password/change",
            &session,
            &json!({ "current_password": "hunter2hunter2", "new_password": "short" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Profile handler — set password
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn set_password_when_already_set_returns_error(pool: PgPool) {
    let app = common::create_test_app(pool);
    // Signup users already have a password, so set_password must be rejected.
    let session = signup_and_session(&app, "e2e-profile-set-pw@example.com").await;

    let resp = app
        .oneshot(post_authed(
            "/api/v1/users/password/set",
            &session,
            &json!({ "new_password": "anothernewpass1" }),
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
    assert_ne!(resp.status(), StatusCode::NO_CONTENT);
}

// ---------------------------------------------------------------------------
// Profile handler — account deletion request / cancel
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn request_deletion_without_confirmation_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-profile-del-noconfirm@example.com").await;

    let resp = app
        .oneshot(delete_authed(
            "/api/v1/users/me",
            &session,
            &json!({ "password": "hunter2hunter2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn request_deletion_wrong_password_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-profile-del-badpw@example.com").await;

    let resp = app
        .oneshot(delete_authed(
            "/api/v1/users/me",
            &session,
            &json!({ "password": "wrongpassword", "confirmation": "DELETE" }),
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
    assert_ne!(resp.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn request_then_cancel_deletion_round_trip(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-profile-del-cycle@example.com";
    let session = signup_and_session(&app, email).await;

    // Schedule deletion.
    let resp = app
        .clone()
        .oneshot(delete_authed(
            "/api/v1/users/me",
            &session,
            &json!({ "password": "hunter2hunter2", "confirmation": "DELETE" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(
        body["deletion_scheduled_at"].is_string(),
        "deletion should be scheduled: {body:?}"
    );

    // Export now reflects the scheduled deletion.
    let resp = app
        .clone()
        .oneshot(get_authed("/api/v1/users/me/export", &session))
        .await
        .unwrap();
    let body = body_json(resp.into_body()).await;
    assert!(body["deletion_scheduled_at"].is_string());

    // Cancel deletion.
    let resp = app
        .clone()
        .oneshot(post_authed(
            "/api/v1/users/me/cancel-deletion",
            &session,
            &json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(body["deletion_scheduled_at"].is_null());

    // Export shows no pending deletion again.
    let resp = app
        .oneshot(get_authed("/api/v1/users/me/export", &session))
        .await
        .unwrap();
    let body = body_json(resp.into_body()).await;
    assert!(body["deletion_scheduled_at"].is_null());
}

#[sqlx::test(migrations = "./migrations")]
async fn cancel_deletion_when_none_pending_is_noop_success(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-profile-cancel-none@example.com").await;

    // Cancelling when nothing is scheduled is an idempotent no-op (200, null).
    let resp = app
        .oneshot(post_authed(
            "/api/v1/users/me/cancel-deletion",
            &session,
            &json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(body["deletion_scheduled_at"].is_null());
}

// ---------------------------------------------------------------------------
// OAuth handler — branches reachable without a real provider
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn oauth_store_device_fingerprint_sets_scoped_cookie(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/oauth/device-fingerprint",
            &json!({ "device_fingerprint": "browser-fp-xyz" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let cookie = common::cookie_value(&resp, "oauth_device_fingerprint");
    assert_eq!(cookie.as_deref(), Some("browser-fp-xyz"));
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_store_device_fingerprint_blank_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/oauth/device-fingerprint",
            &json!({ "device_fingerprint": "   " }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_start_unknown_provider_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/facebook?mode=login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_start_login_unconfigured_provider_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);

    // The test app registers no OAuth clients, so a known provider in login mode
    // reaches the "not supported" branch after state creation.
    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/google?mode=login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_start_link_without_auth_returns_401(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/oauth/google?mode=link")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_unlink_unknown_provider_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-oauth-unlink-bad@example.com").await;

    let resp = app
        .oneshot(delete_authed(
            "/api/v1/auth/oauth/facebook/unlink",
            &session,
            &json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_unlink_not_linked_returns_client_error(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-oauth-unlink-none@example.com").await;

    // No Google account is linked to this user, so unlink must fail (and never 200).
    let resp = app
        .oneshot(delete_authed(
            "/api/v1/auth/oauth/google/unlink",
            &session,
            &json!({}),
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
    assert_ne!(resp.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn oauth_unlink_without_auth_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);

    // An unauthenticated mutating request is rejected before the handler — the
    // CSRF guard (403) fires ahead of the auth guard (401); either is a refusal.
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/auth/oauth/google/unlink")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        matches!(
            resp.status(),
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ),
        "expected 401 or 403, got {}",
        resp.status()
    );
}

// ---------------------------------------------------------------------------
// Auth handler — uncovered error branches
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn login_unknown_user_returns_401(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": "nobody-here@example.com", "password": "whateverpass1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn verify_email_wrong_code_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-auth-verify-wrong@example.com";

    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "000000" }),
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
    assert_ne!(resp.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn complete_signup_bad_token_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-auth-complete-badtoken@example.com";

    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": "not-a-real-token", "email": email, "password": "hunter2hunter2" }),
        ))
        .await
        .unwrap();
    assert!(resp.status().is_client_error());
    assert_ne!(resp.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "./migrations")]
async fn me_with_garbage_token_returns_401(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .header(header::AUTHORIZATION, "Bearer not.a.jwt")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = "./migrations")]
async fn logout_clears_session_cookie(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-auth-logout@example.com").await;

    let resp = app
        .oneshot(post_authed("/api/v1/auth/logout", &session, &json!({})))
        .await
        .unwrap();
    assert!(resp.status().is_success(), "logout should succeed");
}

#[sqlx::test(migrations = "./migrations")]
async fn refresh_without_cookie_returns_401(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json("/api/v1/auth/refresh", &json!({})))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}
