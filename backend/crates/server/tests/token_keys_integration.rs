//! Token key rotation (OT-009, spec requirement "Signing keys rotate without
//! signing anyone out").
//!
//! Each "deploy" is a separate app built over the same database with different
//! `JWT_SIGNING_KEY` / `JWT_TRUSTED_KEYS`:
//! - before: signs with `old`
//! - rotating: signs with `new`, still trusts `old`
//! - retired: signs with `new`, `old` removed

mod common;

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

use server::infrastructure::crypto::Crypto;

const FINGERPRINT: &str = "test-device-rotation";
const PASSWORD: &str = "hunter2hunter2";

fn old_key() -> String {
    common::test_token_key("old", 0x31)
}

fn new_key() -> String {
    common::test_token_key("new", 0x32)
}

fn before() -> Arc<Crypto> {
    common::crypto_with_token_keys(&old_key(), None)
}

fn rotating() -> Arc<Crypto> {
    common::crypto_with_token_keys(&new_key(), Some(&old_key()))
}

fn retired() -> Arc<Crypto> {
    common::crypto_with_token_keys(&new_key(), None)
}

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", FINGERPRINT)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn send(app: &Router, req: Request<Body>) -> (axum::http::Response<Body>, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    // Rebuild a body-less response so callers can still read headers.
    (axum::http::Response::from_parts(parts, Body::empty()), json)
}

fn kid_of(token: &str) -> String {
    jsonwebtoken::decode_header(token)
        .unwrap()
        .kid
        .expect("token names its key")
}

struct Session {
    auth: String,
    refresh: String,
    csrf: String,
}

/// Sign up and sign in on `app`, returning the session cookies.
async fn signed_in(app: &Router, email: &str) -> Session {
    send(
        app,
        post_json("/api/v1/auth/register", &json!({ "email": email })),
    )
    .await;
    let (_, body) = send(
        app,
        post_json(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ),
    )
    .await;
    let signup_token = body["signup_token"]
        .as_str()
        .unwrap_or_else(|| panic!("verify-email: {body}"));
    send(
        app,
        post_json(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": PASSWORD }),
        ),
    )
    .await;
    let (resp, body) = send(
        app,
        post_json(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": PASSWORD }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "login failed: {body}");
    Session {
        auth: common::cookie_value(&resp, "__Host-auth_token").expect("auth cookie"),
        refresh: common::cookie_value(&resp, "__Host-refresh_token").expect("refresh cookie"),
        csrf: body["csrf_token"].as_str().unwrap().to_string(),
    }
}

async fn me(app: &Router, auth: &str) -> (StatusCode, Value) {
    let req = Request::builder()
        .uri("/api/v1/auth/me")
        .header(header::COOKIE, format!("__Host-auth_token={auth}"))
        .body(Body::empty())
        .unwrap();
    let (resp, body) = send(app, req).await;
    (resp.status(), body)
}

/// Refresh the way the dashboard does after a 401: refresh cookie plus the
/// double-submit CSRF pair, and the (possibly rejected) session token.
async fn refresh(app: &Router, s: &Session) -> axum::http::Response<Body> {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/refresh")
        .header(
            header::COOKIE,
            format!(
                "__Host-auth_token={}; __Host-refresh_token={}; __Host-csrf_token={}",
                s.auth, s.refresh, s.csrf
            ),
        )
        .header("x-csrf-token", &s.csrf)
        .header("x-device-fingerprint", FINGERPRINT)
        .body(Body::empty())
        .unwrap();
    let (resp, body) = send(app, req).await;
    assert_eq!(resp.status(), StatusCode::OK, "refresh failed: {body}");
    resp
}

#[sqlx::test(migrations = "../../migrations")]
async fn tokens_from_the_previous_key_keep_working_after_rotation(pool: PgPool) {
    let app = common::create_test_app_with_crypto(pool.clone(), before());
    let session = signed_in(&app, "test-rotate-keep@example.com").await;
    assert_eq!(kid_of(&session.auth), "old");

    let app = common::create_test_app_with_crypto(pool, rotating());
    let (status, body) = me(&app, &session.auth).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "previous key must still be accepted: {body}"
    );

    // The next token is signed with the new active key.
    let resp = refresh(&app, &session).await;
    let next = common::cookie_value(&resp, "__Host-auth_token").expect("new auth cookie");
    assert_eq!(kid_of(&next), "new");
    assert_eq!(me(&app, &next).await.0, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn retiring_a_key_signs_its_sessions_in_again_silently(pool: PgPool) {
    let app = common::create_test_app_with_crypto(pool.clone(), before());
    let session = signed_in(&app, "test-rotate-retire@example.com").await;

    let app = common::create_test_app_with_crypto(pool, retired());
    let (status, body) = me(&app, &session.auth).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(body["code"], "token_invalid", "{body}");

    // The opaque refresh token isn't signed, so refresh still works.
    let resp = refresh(&app, &session).await;
    let next = common::cookie_value(&resp, "__Host-auth_token").expect("new auth cookie");
    assert_eq!(kid_of(&next), "new");
    assert_eq!(me(&app, &next).await.0, StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn retiring_a_key_invalidates_its_reset_links(pool: PgPool) {
    let email = "test-rotate-reset@example.com";
    let app = common::create_test_app_with_crypto(pool.clone(), before());
    signed_in(&app, email).await;
    send(
        &app,
        post_json("/api/v1/auth/forgot-password", &json!({ "email": email })),
    )
    .await;
    let (_, body) = send(
        &app,
        post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "123456" }),
        ),
    )
    .await;
    let reset_token = body["reset_token"]
        .as_str()
        .expect("reset token")
        .to_string();
    assert_eq!(kid_of(&reset_token), "old");

    let app = common::create_test_app_with_crypto(pool, retired());
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/reset-password",
            &json!({ "reset_token": reset_token, "new_password": "a-new-password-1" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{body}");

    // The Visitor can request a new link, signed with the new key.
    send(
        &app,
        post_json("/api/v1/auth/forgot-password", &json!({ "email": email })),
    )
    .await;
    let (resp, body) = send(
        &app,
        post_json(
            "/api/v1/auth/verify-reset-code",
            &json!({ "email": email, "code": "123456" }),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK, "{body}");
    assert_eq!(kid_of(body["reset_token"].as_str().unwrap()), "new");
}
