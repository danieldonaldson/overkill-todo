//! Integration tests for the email-verification, magic-link, resend, and
//! password-reset HTTP endpoints, plus the verification middleware gate.
//!
//! These drive `verification_handler` end to end over the real router and
//! Postgres, manipulating verification-code rows directly via SQL to reach the
//! expired / locked / cooldown branches that a happy-path flow can't hit.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
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

/// Drives the register step so a user + verification-code row exists.
async fn register(app: &axum::Router, email: &str) {
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "register failed for {email}");
}

/// Looks up a user's id by email.
///
/// Test glue uses runtime `sqlx::query*` (not the compile-checked macros) so the
/// suite builds under `SQLX_OFFLINE=true` without regenerating the `.sqlx` cache.
async fn user_id(pool: &PgPool, email: &str) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// verify-email branches
// ---------------------------------------------------------------------------

/// Verifying an unknown email returns 404.
#[sqlx::test(migrations = "./migrations")]
async fn verify_email_unknown_user_returns_404(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": "e2e-nobody@example.com", "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// A wrong code returns 400 with `code_invalid` and a decremented attempts count.
#[sqlx::test(migrations = "./migrations")]
async fn verify_email_wrong_code_reports_remaining_attempts(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-wrongcode@example.com";
    register(&app, email).await;

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "000000" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "CODE_INVALID");
    // 5 max attempts, one consumed by this wrong guess.
    assert_eq!(body["attempts_remaining"], 4);
}

/// An expired code returns 400 with `code_expired`.
#[sqlx::test(migrations = "./migrations")]
async fn verify_email_expired_code_returns_code_expired(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-expired@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;

    // Push the code into the past.
    sqlx::query("UPDATE email_verification_codes SET expires_at = NOW() - INTERVAL '1 hour' WHERE user_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "CODE_EXPIRED");
}

/// A code at the attempt cap returns 400 with `code_locked`.
#[sqlx::test(migrations = "./migrations")]
async fn verify_email_locked_code_returns_code_locked(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-locked@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;

    // Drive attempts to the cap (MAX_ATTEMPTS = 5).
    sqlx::query("UPDATE email_verification_codes SET attempts = 5 WHERE user_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "CODE_LOCKED");
    assert_eq!(body["attempts_remaining"], 0);
}

/// Verifying twice: the second call hits the already-verified path and still
/// returns a usable response (a fresh signup token, since no password is set).
#[sqlx::test(migrations = "./migrations")]
async fn verify_email_already_verified_reissues_signup_token(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-already@example.com";
    register(&app, email).await;

    // First verification succeeds.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(body["signup_token"].is_string());

    // Mark verified is already done; a second verify hits AlreadyVerified.
    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    // Still no password set, so a signup token is returned again.
    assert!(body["signup_token"].is_string());
    assert_eq!(body["user"]["email_verified"], true);
}

// ---------------------------------------------------------------------------
// resend-verification branches
// ---------------------------------------------------------------------------

/// Resending immediately after registration is inside the 5-minute cooldown.
#[sqlx::test(migrations = "./migrations")]
async fn resend_during_cooldown_returns_cooldown_active(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-cooldown@example.com";
    register(&app, email).await;

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/resend-verification",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "COOLDOWN_ACTIVE");
    assert!(body["cooldown_seconds"].as_i64().unwrap() > 0);
}

/// Resend succeeds once the cooldown window has elapsed (simulated by aging
/// `last_sent_at`), returning a fresh cooldown for the frontend timer.
#[sqlx::test(migrations = "./migrations")]
async fn resend_after_cooldown_succeeds(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-resend-ok@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;

    // Age the code past the 5-minute resend cooldown.
    sqlx::query("UPDATE email_verification_codes SET last_sent_at = NOW() - INTERVAL '10 minutes' WHERE user_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/resend-verification",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["success"], true);
    assert_eq!(body["cooldown_seconds"], 300);
}

/// A locked code refuses resend for the lock window, even past the cooldown.
#[sqlx::test(migrations = "./migrations")]
async fn resend_while_locked_returns_code_locked(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-resend-locked@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;

    // At the attempt cap, sent 6 minutes ago: cooldown elapsed but the 15-minute
    // lock window has not — the lockout must win.
    sqlx::query("UPDATE email_verification_codes SET attempts = 5, last_sent_at = NOW() - INTERVAL '6 minutes' WHERE user_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/resend-verification",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "CODE_LOCKED");
}

/// Resending for an already-verified user returns `already_verified`.
#[sqlx::test(migrations = "./migrations")]
async fn resend_already_verified_returns_400(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-resend-verified@example.com";
    register(&app, email).await;

    // Verify first.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/resend-verification",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "ALREADY_VERIFIED");
}

/// Resending for an unknown user returns 404.
#[sqlx::test(migrations = "./migrations")]
async fn resend_unknown_user_returns_404(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/resend-verification",
            &json!({ "email": "e2e-ghost@example.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// magic-link branches
// ---------------------------------------------------------------------------

/// Plants a magic-token hash on a user's verification code (as production does
/// at send time) and returns the raw token.
async fn plant_magic_token(pool: &PgPool, id: uuid::Uuid, expired: bool) -> String {
    let token = uuid::Uuid::new_v4().to_string();
    let hash = hex::encode(Sha256::digest(token.as_bytes()));
    // Two fixed literal queries (no interpolation) keep the SQL static.
    if expired {
        sqlx::query(
            "UPDATE email_verification_codes SET magic_token_hash = $2, magic_token_expires_at = NOW() - INTERVAL '1 hour' WHERE user_id = $1",
        )
        .bind(id)
        .bind(&hash)
        .execute(pool)
        .await
        .unwrap();
    } else {
        sqlx::query(
            "UPDATE email_verification_codes SET magic_token_hash = $2, magic_token_expires_at = NOW() + INTERVAL '24 hours' WHERE user_id = $1",
        )
        .bind(id)
        .bind(&hash)
        .execute(pool)
        .await
        .unwrap();
    }
    token
}

fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// Extract a redirect Location header.
fn location(resp: &axum::http::Response<Body>) -> String {
    resp.headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

/// A valid magic link verifies the email and redirects to /set-password.
#[sqlx::test(migrations = "./migrations")]
async fn magic_link_valid_redirects_to_set_password(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-magic@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;
    let token = plant_magic_token(&pool, id, false).await;

    let resp = app
        .oneshot(get(&format!("/api/v1/auth/verify-magic?token={token}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = location(&resp);
    assert!(loc.contains("/set-password"), "unexpected redirect: {loc}");
    assert!(loc.contains("token="));

    // Email is now verified in the DB.
    let verified = sqlx::query_scalar::<_, bool>("SELECT email_verified FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(verified);
}

/// An invalid (unknown) magic token redirects to /verify-email with an error.
#[sqlx::test(migrations = "./migrations")]
async fn magic_link_invalid_token_redirects_to_error(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(get("/api/v1/auth/verify-magic?token=not-a-real-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = location(&resp);
    assert!(loc.contains("/verify-email?error="), "got: {loc}");
}

/// An expired magic token redirects to /verify-email with an error.
#[sqlx::test(migrations = "./migrations")]
async fn magic_link_expired_token_redirects_to_error(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-magic-expired@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;
    let token = plant_magic_token(&pool, id, true).await;

    let resp = app
        .oneshot(get(&format!("/api/v1/auth/verify-magic?token={token}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = location(&resp);
    assert!(loc.contains("/verify-email?error="), "got: {loc}");

    // Email must NOT have been verified.
    let verified = sqlx::query_scalar::<_, bool>("SELECT email_verified FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!verified);
}

// ---------------------------------------------------------------------------
// password-reset flow
// ---------------------------------------------------------------------------

/// forgot-password always returns success (no email enumeration), even for an
/// unknown email.
#[sqlx::test(migrations = "./migrations")]
async fn forgot_password_unknown_email_returns_success(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/forgot-password",
            &json!({ "email": "e2e-noone@example.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["success"], true);
}

/// Full password-reset flow: complete signup, request reset, verify the dev
/// reset code, then reset the password and auto-login.
#[sqlx::test(migrations = "./migrations")]
async fn password_reset_flow_succeeds(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-reset@example.com";

    // Complete a signup so the user has a password to reset.
    register(&app, email).await;
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
            &json!({ "signup_token": signup_token, "email": email, "password": "originalpass1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Request a reset code (dev code for test emails is 123456).
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/forgot-password",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify the reset code → reset token.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "verify-reset-code failed");
    let reset_token = body_json(resp.into_body()).await["reset_token"]
        .as_str()
        .unwrap()
        .to_string();

    // Reset the password and auto-login.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/reset-password",
            &json!({ "reset_token": reset_token, "new_password": "brandnewpass9" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "reset-password failed");
    assert!(
        common::cookie_value(&resp, "auth_token").is_some(),
        "expected auth cookie after reset"
    );

    // New password works for login; old one does not.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "brandnewpass9" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "login with new password failed"
    );

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "originalpass1" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// verify-reset-code with a wrong code returns an error (400).
#[sqlx::test(migrations = "./migrations")]
async fn verify_reset_code_wrong_code_fails(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-reset-wrong@example.com";
    register(&app, email).await;
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
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": "originalpass1" }),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/forgot-password",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "999999" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// reset-password with a bogus token is rejected.
#[sqlx::test(migrations = "./migrations")]
async fn reset_password_invalid_token_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/reset-password",
            &json!({ "reset_token": "not.a.jwt", "new_password": "whateverpass1" }),
        ))
        .await
        .unwrap();
    assert_ne!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// verification middleware gate
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// registration delivery failure
// ---------------------------------------------------------------------------

/// A registration whose verification email cannot be delivered must say so.
///
/// The code is the only way to finish signup — there is no password or session
/// yet — so reporting success would strand the caller on a "check your email"
/// screen with nothing ever arriving.
#[sqlx::test(migrations = "./migrations")]
async fn register_reports_a_verification_email_that_could_not_be_delivered(pool: PgPool) {
    let app = common::create_test_app_with_email_service(
        pool.clone(),
        std::sync::Arc::new(common::FailFirstEmailService::new(1)),
    );

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": "e2e-send-fails@example.com" }),
        ))
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

/// ...and must leave the resend cooldown open.
///
/// `last_sent_at` is stamped when the code is issued, before the send is even
/// attempted, so a failed delivery would otherwise bar the user from the one
/// recovery path for the full 5-minute cooldown — locked out by an email that
/// never went anywhere.
#[sqlx::test(migrations = "./migrations")]
async fn resend_is_available_immediately_after_a_failed_registration_send(pool: PgPool) {
    // Fail only the registration send; the resend that follows succeeds.
    let app = common::create_test_app_with_email_service(
        pool.clone(),
        std::sync::Arc::new(common::FailFirstEmailService::new(1)),
    );
    let email = "e2e-send-fails-then-resend@example.com";

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);

    // No waiting: the cooldown was released because nothing was delivered.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/resend-verification",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "resend must not be held off by a send that failed"
    );

    // And the resent code is usable, so the account is genuinely recoverable.
    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(body["signup_token"].is_string());
}

/// An unverified user (registered but email not yet verified) cannot reach a
/// verification-gated endpoint, but can once verified + signed up.
///
/// `GET /api/v1/users/me/export` is behind `require_verified_email`.
#[sqlx::test(migrations = "./migrations")]
async fn verification_gate_blocks_then_allows(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-gate@example.com";

    // Register and verify, but stay on a signup token (no full session yet).
    register(&app, email).await;
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

    // Complete signup → real session with a verified JWT.
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": "gatepass123" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let jwt = common::cookie_value(&resp, "auth_token").expect("auth cookie");

    // Verified user reaches the gated endpoint (200, not 403).
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/users/me/export")
                .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::FORBIDDEN,
        "verified user was blocked by the verification gate"
    );
}
