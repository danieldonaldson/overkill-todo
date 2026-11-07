//! Negative security-boundary integration tests driven through the real router.
//!
//! Where the happy-path suites assert that good input is accepted, these assert
//! that BAD input is rejected: CSRF mismatches and cross-user tokens, expired /
//! tampered JWTs, the email-verification gate, lockout reset-on-success, and the
//! presence of the `X-Trace-Id` correlation header.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
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

async fn user_id(pool: &PgPool, email: &str) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A real session: the `auth_token` JWT cookie, the readable CSRF token, and the
/// authenticated user's id.
struct Session {
    auth_token: String,
    csrf_token: String,
    user_id: String,
}

/// Run the full signup flow for `email`/`password` and return the resulting
/// session (auth cookie + CSRF token), exactly as a browser would hold it.
async fn establish_session(app: &axum::Router, email: &str, password: &str) -> Session {
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "register failed for {email}");

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "verify-email failed");
    let signup_token = body_json(resp.into_body()).await["signup_token"]
        .as_str()
        .unwrap()
        .to_string();

    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": password }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "complete-signup failed");
    let auth_token = common::cookie_value(&resp, "auth_token").expect("auth_token cookie");
    let body = body_json(resp.into_body()).await;
    let csrf_token = body["csrf_token"].as_str().expect("csrf_token").to_string();
    let user_id = body["user"]["id"].as_str().expect("user id").to_string();

    Session {
        auth_token,
        csrf_token,
        user_id,
    }
}

/// A minimal authenticated mutating request to the CSRF-protected
/// change-password endpoint. Body content is irrelevant: CSRF runs first.
fn change_password_req(
    auth_token: &str,
    csrf_cookie: Option<&str>,
    csrf_header: Option<&str>,
) -> Request<Body> {
    let cookie = csrf_cookie.map_or_else(
        || format!("auth_token={auth_token}"),
        |c| format!("auth_token={auth_token}; csrf_token={c}"),
    );
    let mut builder = Request::builder()
        .method("POST")
        .uri("/api/v1/users/password/change")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookie);
    if let Some(h) = csrf_header {
        builder = builder.header("x-csrf-token", h);
    }
    builder
        .body(Body::from(
            serde_json::to_vec(&json!({
                "current_password": "irrelevant",
                "new_password": "irrelevantnew1"
            }))
            .unwrap(),
        ))
        .unwrap()
}

// ---------------------------------------------------------------------------
// CSRF — header≠cookie, cookie-without-header, cross-user binding
// ---------------------------------------------------------------------------

/// An authenticated mutating request whose CSRF header does not match the cookie
/// is rejected with 403 `csrf_invalid`.
#[sqlx::test(migrations = "./migrations")]
async fn csrf_header_cookie_mismatch_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = establish_session(&app, "e2e-csrf-mismatch@example.com", "hunter2hunter2").await;

    let resp = app
        .oneshot(change_password_req(
            &session.auth_token,
            Some(&session.csrf_token),
            Some("a-different-token-value"),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "csrf_invalid");
}

/// An authenticated mutating request with the CSRF cookie present but the header
/// missing is rejected with 403.
#[sqlx::test(migrations = "./migrations")]
async fn csrf_cookie_without_header_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = establish_session(&app, "e2e-csrf-noheader@example.com", "hunter2hunter2").await;

    let resp = app
        .oneshot(change_password_req(
            &session.auth_token,
            Some(&session.csrf_token),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "csrf_invalid");
}

/// A well-formed, internally-consistent CSRF token (header == cookie, valid
/// signature) that is bound to a DIFFERENT user must not satisfy this user's
/// request: the middleware binds verification to the authenticated principal.
#[sqlx::test(migrations = "./migrations")]
async fn csrf_token_bound_to_other_user_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);
    let victim = establish_session(&app, "e2e-csrf-victim@example.com", "hunter2hunter2").await;

    // Forge a CSRF token bound to some OTHER user id, signed with the real
    // secret (so the signature itself is valid — only the binding is wrong).
    let other_user = uuid::Uuid::now_v7().to_string();
    let foreign_csrf = common::mint_csrf_token(&other_user);

    // Present the victim's session (auth_token) but the foreign CSRF token in
    // both header and cookie — double-submit passes, user-binding must fail.
    let resp = app
        .oneshot(change_password_req(
            &victim.auth_token,
            Some(&foreign_csrf),
            Some(&foreign_csrf),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "csrf_invalid");

    // Sanity: the victim's OWN token (bound to their id) is not the same value.
    assert_ne!(victim.csrf_token, foreign_csrf);
    assert_ne!(victim.user_id, other_user);
}

// ---------------------------------------------------------------------------
// auth — expired / tampered JWT
// ---------------------------------------------------------------------------

/// An expired session JWT presented as a bearer token is rejected with 401.
#[sqlx::test(migrations = "./migrations")]
async fn expired_jwt_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);

    // Mint a token that expired in the past (negative expiry minutes).
    let expired = common::mint_session_jwt(uuid::Uuid::now_v7(), true, "user", -5);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {expired}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

/// A JWT whose signature has been corrupted is rejected with 401.
#[sqlx::test(migrations = "./migrations")]
async fn tampered_jwt_signature_is_rejected(pool: PgPool) {
    let app = common::create_test_app(pool);

    let valid = common::mint_session_jwt(uuid::Uuid::now_v7(), true, "user", 15);
    // Corrupt the trailing signature segment while keeping the JWT structure
    // (three dot-separated segments) intact.
    let (head, sig) = valid.rsplit_once('.').expect("jwt has a signature segment");
    let last = sig.chars().last().expect("signature non-empty");
    let replacement = if last == 'A' { 'B' } else { 'A' };
    let tampered_sig = format!("{}{}", &sig[..sig.len() - last.len_utf8()], replacement);
    let tampered = format!("{head}.{tampered_sig}");
    assert_ne!(tampered, valid, "tamper must change the token");

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {tampered}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// verification gate — authenticated but unverified
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// lockout — a correct attempt does not count against the cap (release-on-success)
// ---------------------------------------------------------------------------

/// Reset-code validation claims an attempt slot then releases it on success, so a
/// correct guess never counts against the lockout cap. With `attempts` already at
/// `MAX_ATTEMPTS - 1` (one slot left), a CORRECT code must still succeed and the
/// stored attempt count must be back at `MAX_ATTEMPTS - 1` afterwards — proving
/// the slot was released rather than consumed.
#[sqlx::test(migrations = "./migrations")]
async fn correct_reset_code_releases_attempt_slot(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-lockout-reset@example.com";

    // Complete signup so the user has a password (and thus a reset flow).
    let _ = establish_session(&app, email, "originalpass1").await;
    let id = user_id(&pool, email).await;

    // Request a reset code (dev code for e2e- emails is 123456).
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/forgot-password",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // First, a WRONG code increments the attempt counter (boundary: failures
    // ARE counted).
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "000000" }),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let attempts_after_wrong = sqlx::query_scalar::<_, i32>(
        "SELECT attempts FROM password_reset_codes WHERE user_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attempts_after_wrong, 1, "a wrong guess must be counted");

    // Drive the counter to MAX_ATTEMPTS - 1 (= 4) so only one slot remains.
    sqlx::query("UPDATE password_reset_codes SET attempts = 4 WHERE user_id = $1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    // A CORRECT code must still succeed (not be rejected as locked)...
    let resp = app
        .clone()
        .oneshot(post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "123456" }),
        ))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "correct code at the last slot must not be locked out"
    );

    // ...and the slot it claimed must have been released: still 4, not 5.
    let attempts_after_correct = sqlx::query_scalar::<_, i32>(
        "SELECT attempts FROM password_reset_codes WHERE user_id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        attempts_after_correct, 4,
        "a correct guess must release its claimed slot (reset-on-success)"
    );
}

// ---------------------------------------------------------------------------
// trace_id — correlation header is present on responses
// ---------------------------------------------------------------------------

/// Every response carries an `X-Trace-Id` header from the trace-id middleware.
#[sqlx::test(migrations = "./migrations")]
async fn responses_carry_trace_id_header(pool: PgPool) {
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

    let trace_id = resp
        .headers()
        .get("x-trace-id")
        .expect("X-Trace-Id header must be present")
        .to_str()
        .unwrap();
    assert!(!trace_id.is_empty(), "trace id must be non-empty");
}

/// A session whose JWT carries `verified = false` is blocked from a gated route
/// (`GET /api/v1/users/me/export`) with 403 `email_not_verified`, even though the token
/// itself is valid and authenticates the principal.
#[sqlx::test(migrations = "./migrations")]
async fn unverified_session_blocked_by_verification_gate(pool: PgPool) {
    let app = common::create_test_app(pool);

    // A valid, correctly-signed session token, but with verified=false.
    let unverified = common::mint_session_jwt(uuid::Uuid::now_v7(), false, "user", 15);

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/api/v1/users/me/export")
                .header(header::AUTHORIZATION, format!("Bearer {unverified}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "email_not_verified");
}
