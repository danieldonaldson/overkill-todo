//! Handler error/branch coverage through the real router.
//!
//! These tests drive untested handler error paths end-to-end via `create_test_app`:
//! login validation/branch errors and the readiness probe's happy path.

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

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", "test-device-abc123")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// A POST without the `x-device-fingerprint` header.
fn post_json_no_fingerprint(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

/// Runs the full signup flow so the account has a password.
async fn complete_signup(app: &axum::Router, email: &str) {
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
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// login branches
// ---------------------------------------------------------------------------

/// A login missing the `x-device-fingerprint` header fails validation → 400.
/// (The user must exist and have a password so we get past the enumeration and
/// signup-incomplete guards to the fingerprint check.)
#[sqlx::test(migrations = "./migrations")]
async fn login_missing_device_fingerprint_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-login-nofp@example.com";
    // Complete signup so the account has a password.
    complete_signup(&app, email).await;

    let resp = app
        .oneshot(post_json_no_fingerprint(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "hunter2hunter2" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// A user who started signup (registered + verified) but never set a password
/// AND has no linked OAuth account is mid-signup: the handler returns 403 with
/// `error_code: "signup_incomplete"`.
///
/// The companion case — a password-less user *with* a linked OAuth account — is
/// an OAuth-only account and gets 401 instead (see
/// `login_oauth_only_user_returns_401`). `login` distinguishes the two by
/// checking for linked OAuth accounts, so a provider user is not wrongly told
/// their signup is "incomplete".
#[sqlx::test(migrations = "./migrations")]
async fn login_signup_incomplete_returns_403(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-login-incomplete@example.com";

    // Register + verify, but do NOT complete signup (no password set).
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "anything-goes-1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "signup_incomplete");
}

/// A password-less user *with* a linked OAuth account is an OAuth-only account:
/// password login is refused with a generic 401 (NOT 403 `signup_incomplete`),
/// and the response must not disclose that the account is provider-linked.
#[sqlx::test(migrations = "./migrations")]
async fn login_oauth_only_user_returns_401(pool: PgPool) {
    use app::application::ports::{OAuthAccountRepository, UserRepository};
    use app::domain::entities::{OAuthAccount, OAuthProvider, User};
    use app::domain::value_objects::Email;
    use app::infrastructure::persistence::{
        Database, PostgresOAuthAccountRepository, PostgresUserRepository,
    };
    use std::sync::Arc;

    let email = "e2e-oauth-only@example.com";

    // Seed an OAuth-only account directly: a user with NO password plus a linked
    // Google account. (Going through the real OAuth flow would need a fake
    // provider; seeding via the repos is equivalent for the login branch.)
    let db = Arc::new(Database::from_pool(pool.clone()));
    let user = User::new(
        Email::new(email).unwrap(),
        None, // OAuth-only: no password set
        "Oauth".to_string(),
        "User".to_string(),
    );
    PostgresUserRepository::new(db.clone())
        .save(&user)
        .await
        .expect("save user");
    PostgresOAuthAccountRepository::new(db.clone())
        .save(&OAuthAccount::new(
            *user.id(),
            OAuthProvider::Google,
            "google-sub-1".to_string(),
            Some(email.to_string()),
        ))
        .await
        .expect("save oauth account");

    let resp = common::create_test_app(pool)
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "anything-goes-1" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(resp.into_body()).await;
    assert_ne!(body["error_code"], "signup_incomplete");
}

// ---------------------------------------------------------------------------
// health_ready
// ---------------------------------------------------------------------------

/// With a live pool the readiness probe returns 200. The 503 DB-down branch is
/// not covered: it requires killing the pool mid-test, which `sqlx::test` owns.
#[sqlx::test(migrations = "./migrations")]
async fn health_ready_returns_200_when_db_up(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
