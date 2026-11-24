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

async fn body_json(body: axum::body::Body) -> Value {
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

fn get_authed(uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .header(header::COOKIE, format!("__Host-auth_token={token}"))
        .body(Body::empty())
        .unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn health_check_returns_200(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn health_probes_are_reachable_under_api_prefix(pool: PgPool) {
    // Health is also mounted under /api/v1 so it routes to the backend through
    // the edge proxy's single-origin `/api` rule (the root `/` goes to the frontend).
    let app = common::create_test_app(pool);

    for uri in ["/api/v1/health", "/api/v1/health/ready"] {
        let resp = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK, "{uri}");
    }
}

/// Login returns a JWT after a completed signup.
#[sqlx::test(migrations = "../../migrations")]
async fn login_returns_token_after_signup(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-login@example.com";
    let password = "securepassword99";

    // Complete signup first
    complete_signup_for(app.clone(), email, password).await;

    // Login
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": password }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "login failed");
    // The session JWT is delivered as the auth_token cookie, not in the body.
    assert!(
        common::cookie_value(&resp, "__Host-auth_token").is_some(),
        "Expected auth_token cookie in login response"
    );
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["email"], email);
}

/// Login with wrong password returns 401.
#[sqlx::test(migrations = "../../migrations")]
async fn login_wrong_password_returns_401(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-badpass@example.com";

    complete_signup_for(app.clone(), email, "correctpassword1").await;

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "wrongpassword99" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// /me without token returns 401.
#[sqlx::test(migrations = "../../migrations")]
async fn me_without_token_returns_401(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(
            Request::builder()
                .uri("/api/v1/auth/me")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// Register with an invalid email returns 400.
#[sqlx::test(migrations = "../../migrations")]
async fn register_invalid_email_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": "not-an-email" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// Helper: run the full signup flow for a given email/password
// ---------------------------------------------------------------------------

async fn complete_signup_for(app: axum::Router, email: &str, password: &str) {
    // Register
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "register failed for {email}");

    // Verify email
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "verify-email failed for {email}"
    );
    let body = body_json(resp.into_body()).await;
    let signup_token = body["signup_token"].as_str().unwrap().to_string();

    // Complete registration
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({
                "signup_token": signup_token,
                "email": email,
                "password": password
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "complete-signup failed for {email}"
    );
}

/// Full happy-path signup: register → verify email → complete registration → me
#[sqlx::test(migrations = "../../migrations")]
async fn signup_flow_succeeds(pool: PgPool) {
    let app = common::create_test_app(pool);
    let email = "e2e-signup@example.com";

    // Step 1 — initiate registration
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "register step failed");
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["success"], true);

    // Step 2 — verify email with dev code "123456"
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "verify-email step failed");
    let body = body_json(resp.into_body()).await;
    let signup_token = body["signup_token"]
        .as_str()
        .expect("Expected signup_token in verify-email response");

    // Step 3 — complete registration (set password)
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({
                "signup_token": signup_token,
                "email": email,
                "password": "hunter2hunter2"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "complete-signup step failed");
    // The session JWT is delivered as the auth_token cookie, not in the body.
    let jwt = common::cookie_value(&resp, "__Host-auth_token").expect("auth_token cookie");
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["email"], email);
    assert_eq!(body["user"]["email_verified"], true);

    // Step 4 — /me with JWT
    let resp = app
        .clone()
        .oneshot(get_authed("/api/v1/auth/me", &jwt))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "/me step failed");
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["email"], email);
    assert_eq!(body["email_verified"], true);
}

/// Mixed-case addresses normalize consistently across register, verify-email,
/// complete-signup, and re-registration (no 409 account-existence oracle).
#[sqlx::test(migrations = "../../migrations")]
async fn mixed_case_signup_verify_and_reregister_succeed(pool: PgPool) {
    let app = common::create_test_app(pool);
    let mixed = "e2e-Mixed.Case@Example.com";
    let upper = "e2e-MIXED.CASE@EXAMPLE.COM";
    let lower = "e2e-mixed.case@example.com";

    // Register with mixed case.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": mixed }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "mixed-case register failed");

    // Verify with different casing.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": upper, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "mixed-case verify-email failed"
    );
    let signup_token = body_json(resp.into_body()).await["signup_token"]
        .as_str()
        .expect("signup_token")
        .to_string();

    // Complete signup with mixed case; stored email is lowercase.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({
                "signup_token": signup_token,
                "email": mixed,
                "password": "hunter2hunter2"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "mixed-case complete-signup failed"
    );
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["email"], lower);

    // Re-registering with either casing returns 200 OK (never 409 Conflict).
    for candidate in [mixed, lower] {
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/v1/auth/register",
                &json!({ "email": candidate }),
            ))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "re-register for {candidate} must not leak account existence"
        );
    }
}

/// Login succeeds regardless of email casing.
#[sqlx::test(migrations = "../../migrations")]
async fn mixed_case_login_succeeds(pool: PgPool) {
    let app = common::create_test_app(pool);
    let password = "securepassword99";

    complete_signup_for(app.clone(), "e2e-Login.Case@Example.com", password).await;

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": "e2e-LOGIN.CASE@EXAMPLE.COM", "password": password }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "mixed-case login failed");
    assert!(common::cookie_value(&resp, "__Host-auth_token").is_some());
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["user"]["email"], "e2e-login.case@example.com");
}

/// Password reset (forgot-password → verify-reset-code → reset-password)
/// succeeds regardless of email casing.
#[sqlx::test(migrations = "../../migrations")]
async fn mixed_case_password_reset_flow_succeeds(pool: PgPool) {
    let app = common::create_test_app(pool);
    let mixed = "e2e-Reset.Case@Example.com";
    let upper = "e2e-RESET.CASE@EXAMPLE.COM";

    complete_signup_for(app.clone(), mixed, "oldpassword123").await;

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/forgot-password",
            &json!({ "email": upper }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "mixed-case forgot-password failed"
    );

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": mixed, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "mixed-case verify-reset-code failed"
    );
    let reset_token = body_json(resp.into_body()).await["reset_token"]
        .as_str()
        .expect("reset_token")
        .to_string();

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/reset-password",
            &json!({ "reset_token": reset_token, "new_password": "newpassword456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "reset-password failed");

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": upper, "password": "newpassword456" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "login after mixed-case reset failed"
    );
}
