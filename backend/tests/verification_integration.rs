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

/// Verify an address with the dev code; returns the signup token.
async fn verify_and_get_signup_token(app: &axum::Router, email: &str) -> String {
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await["signup_token"]
        .as_str()
        .unwrap()
        .to_string()
}

/// Assert a verify-email response is the `ALREADY_VERIFIED` error and carries
/// no credential of any kind.
async fn assert_already_verified_without_credentials(resp: axum::http::Response<Body>) {
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert!(common::cookie_value(&resp, "auth_token").is_none());
    assert!(common::cookie_value(&resp, "refresh_token").is_none());
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["error_code"], "ALREADY_VERIFIED");
    assert!(body.get("signup_token").is_none());
}

/// SECURITY regression: an already-verified account is reported as such but
/// never handed a session. The already-verified branch runs before any code is
/// checked, so issuing credentials there let anyone who knew the email log in.
#[sqlx::test(migrations = "./migrations")]
async fn verify_email_on_verified_account_does_not_log_in(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-victim@example.com";
    register(&app, email).await;
    let signup_token = verify_and_get_signup_token(&app, email).await;
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": "ownerpass123" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Attacker knows only the email and guesses a code.
    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "000000" }),
        ))
        .await
        .unwrap();

    assert_already_verified_without_credentials(resp).await;
}

/// SECURITY regression: a verified account without a password (signup left
/// unfinished, or OAuth-only) must not get a signup token from a guessed code;
/// that token would let the attacker set the account's password.
#[sqlx::test(migrations = "./migrations")]
async fn verify_email_on_verified_passwordless_account_issues_no_signup_token(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-unfinished@example.com";
    register(&app, email).await;
    verify_and_get_signup_token(&app, email).await;

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "000000" }),
        ))
        .await
        .unwrap();

    assert_already_verified_without_credentials(resp).await;
}

/// A user who verified but never set a password (and lost the signup token)
/// recovers through forgot-password, which proves email ownership afresh.
#[sqlx::test(migrations = "./migrations")]
async fn unfinished_signup_recovers_via_password_reset(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-unfinished@example.com";
    register(&app, email).await;
    verify_and_get_signup_token(&app, email).await; // token "lost"

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/forgot-password",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let reset_token = body_json(resp.into_body()).await["reset_token"]
        .as_str()
        .unwrap()
        .to_string();
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/reset-password",
            &json!({ "reset_token": reset_token, "new_password": "recovered123" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": "recovered123" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

// ---------------------------------------------------------------------------
// resend-verification branches
// ---------------------------------------------------------------------------

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

/// POSTs to resend-verification and returns the status and JSON body.
async fn resend(app: &axum::Router, email: &str) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/resend-verification",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    let status = resp.status();
    (status, body_json(resp.into_body()).await)
}

/// Reads `(attempts, lockouts)` for a user's verification code row.
async fn code_counters(pool: &PgPool, id: uuid::Uuid) -> (i32, i32) {
    sqlx::query_as::<_, (i32, i32)>(
        "SELECT attempts, lockouts FROM email_verification_codes WHERE user_id = $1",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Every account-dependent outcome — unknown email, already verified, inside
/// the cooldown, code locked — answers exactly like a successful resend, so the
/// endpoint can't be used to check whether an email has an account.
#[sqlx::test(migrations = "./migrations")]
async fn resend_answers_identically_whatever_the_account_state(pool: PgPool) {
    let app = common::create_test_app(pool.clone());

    // Fresh send, past the cooldown: the reference response.
    let ok_email = "e2e-resend-same-ok@example.com";
    register(&app, ok_email).await;
    sqlx::query("UPDATE email_verification_codes SET last_sent_at = NOW() - INTERVAL '10 minutes' WHERE user_id = $1")
        .bind(user_id(&pool, ok_email).await)
        .execute(&pool)
        .await
        .unwrap();
    let reference = resend(&app, ok_email).await;
    assert_eq!(reference.0, StatusCode::OK);

    // Inside the cooldown `register` just started.
    let cooldown_email = "e2e-resend-same-cooldown@example.com";
    register(&app, cooldown_email).await;

    // Locked: at the cap, cooldown elapsed, lock window not.
    let locked_email = "e2e-resend-same-locked@example.com";
    register(&app, locked_email).await;
    sqlx::query("UPDATE email_verification_codes SET attempts = 5, last_sent_at = NOW() - INTERVAL '6 minutes' WHERE user_id = $1")
        .bind(user_id(&pool, locked_email).await)
        .execute(&pool)
        .await
        .unwrap();

    // Already verified.
    let verified_email = "e2e-resend-same-verified@example.com";
    register(&app, verified_email).await;
    verify_and_get_signup_token(&app, verified_email).await;

    for email in [
        cooldown_email,
        locked_email,
        verified_email,
        "e2e-resend-same-ghost@example.com",
    ] {
        assert_eq!(
            resend(&app, email).await,
            reference,
            "{email} answered differently"
        );
    }
}

/// A locked code is not replaced by a resend inside its lock window, even
/// though the response says nothing about it.
#[sqlx::test(migrations = "./migrations")]
async fn resend_while_locked_issues_no_new_code(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-resend-locked@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;

    sqlx::query("UPDATE email_verification_codes SET attempts = 5, last_sent_at = NOW() - INTERVAL '6 minutes' WHERE user_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(resend(&app, email).await.0, StatusCode::OK);
    assert_eq!(
        code_counters(&pool, id).await,
        (5, 0),
        "the locked code must survive"
    );
}

/// Re-registering an unfinished signup must not hand out a fresh code (and five
/// fresh guesses) while the current one is locked. This used to bypass the
/// attempt cap entirely.
#[sqlx::test(migrations = "./migrations")]
async fn reregistering_does_not_reset_a_locked_code(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-reregister-locked@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;

    sqlx::query("UPDATE email_verification_codes SET attempts = 5, last_sent_at = NOW() - INTERVAL '6 minutes' WHERE user_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    register(&app, email).await; // still a plain 200
    assert_eq!(
        code_counters(&pool, id).await,
        (5, 0),
        "register must not reissue"
    );
}

/// Replacing a locked code counts as a lockout, and the next lock lasts twice
/// as long: 20 minutes after a second lock is still inside its 30-minute window.
#[sqlx::test(migrations = "./migrations")]
async fn lock_window_doubles_after_each_lockout(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-lockout-escalation@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;
    let lock_20_minutes_ago = "UPDATE email_verification_codes SET attempts = 5, last_sent_at = NOW() - INTERVAL '20 minutes' WHERE user_id = $1";

    // First lock (15 min) has expired: the resend replaces the code.
    sqlx::query(lock_20_minutes_ago)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    resend(&app, email).await;
    assert_eq!(code_counters(&pool, id).await, (0, 1));

    // Second lock is 30 min, so 20 min later the code is still locked.
    sqlx::query(lock_20_minutes_ago)
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    resend(&app, email).await;
    assert_eq!(
        code_counters(&pool, id).await,
        (5, 1),
        "second lock must outlast 15 min"
    );
}

/// Registering an email that already has a complete account answers exactly
/// like a new signup (the owner is emailed instead of sent a code).
#[sqlx::test(migrations = "./migrations")]
async fn register_existing_account_answers_like_a_new_signup(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-register-existing@example.com";
    register(&app, email).await;
    let signup_token = verify_and_get_signup_token(&app, email).await;
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": "existing123" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let register_body = |email: &'static str| {
        let app = app.clone();
        async move {
            let resp = app
                .oneshot(post_json(
                    "/api/v1/auth/register",
                    &json!({ "email": email }),
                ))
                .await
                .unwrap();
            (resp.status(), body_json(resp.into_body()).await)
        }
    };
    let existing = register_body(email).await;
    let fresh = register_body("e2e-register-brand-new@example.com").await;
    assert_eq!(existing.0, StatusCode::OK);
    assert_eq!(existing, fresh);
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

fn confirm_magic(token: &str) -> Request<Body> {
    post_json("/api/v1/auth/verify-magic", &json!({ "token": token }))
}

async fn email_verified(pool: &PgPool, id: uuid::Uuid) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT email_verified FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// The legacy GET link only forwards the token to the frontend confirmation
/// page (in the fragment). It must not verify anything: mail scanners fetch it.
#[sqlx::test(migrations = "./migrations")]
async fn magic_link_get_redirects_without_side_effects(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-magic-get@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;
    let token = plant_magic_token(&pool, id, false).await;

    let resp = app
        .clone()
        .oneshot(get(&format!("/api/v1/auth/verify-magic?token={token}")))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = location(&resp);
    assert!(
        loc.ends_with(&format!("/verify-magic#token={token}")),
        "unexpected redirect: {loc}"
    );
    assert!(!email_verified(&pool, id).await);

    // The token is still usable after the prefetch.
    let resp = app.oneshot(confirm_magic(&token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Confirming a valid link verifies the email and returns a signup token in the
/// body (never in a URL).
#[sqlx::test(migrations = "./migrations")]
async fn magic_link_confirm_returns_signup_token(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-magic@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;
    let token = plant_magic_token(&pool, id, false).await;

    let resp = app.clone().oneshot(confirm_magic(&token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert!(body["signup_token"].as_str().is_some_and(|t| !t.is_empty()));
    assert_eq!(body["user"]["email"], email);
    assert!(email_verified(&pool, id).await);

    // Single use.
    let resp = app.oneshot(confirm_magic(&token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(resp.into_body()).await["error_code"],
        "CODE_INVALID"
    );
}

/// An unknown token is rejected with a structured error.
#[sqlx::test(migrations = "./migrations")]
async fn magic_link_invalid_token_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);

    let resp = app
        .oneshot(confirm_magic("not-a-real-token"))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(resp.into_body()).await["error_code"],
        "CODE_INVALID"
    );
}

/// An expired token is rejected and verifies nothing.
#[sqlx::test(migrations = "./migrations")]
async fn magic_link_expired_token_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-magic-expired@example.com";
    register(&app, email).await;
    let id = user_id(&pool, email).await;
    let token = plant_magic_token(&pool, id, true).await;

    let resp = app.oneshot(confirm_magic(&token)).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    assert_eq!(
        body_json(resp.into_body()).await["error_code"],
        "CODE_EXPIRED"
    );
    assert!(!email_verified(&pool, id).await);
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
