//! Bot-protection enforcement through the real router.
//!
//! Every other suite runs with the no-op Turnstile service, which reports bot
//! protection as *disabled*, so the enforcing branch never runs there. These
//! tests switch it on with a fake service and check that each protected
//! endpoint requires a valid token before doing anything else.

mod common;

use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use overkill_todo::application::ports::TurnstileService;
use overkill_todo::domain::errors::DomainError;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// Token the fake accepts.
const VALID: &str = "pass";
/// Token that makes the fake fail like an unreachable Cloudflare.
const UPSTREAM_DOWN: &str = "upstream-down";

/// Enabled Turnstile: accepts [`VALID`], errors on [`UPSTREAM_DOWN`], and
/// reports every other token as a bot.
struct FakeTurnstile;

#[async_trait]
impl TurnstileService for FakeTurnstile {
    async fn verify(&self, token: &str) -> Result<bool, DomainError> {
        match token {
            VALID => Ok(true),
            UPSTREAM_DOWN => Err(DomainError::ExternalService(
                "siteverify unreachable".into(),
            )),
            _ => Ok(false),
        }
    }

    fn is_enabled(&self) -> bool {
        true
    }
}

fn app(pool: PgPool) -> Router {
    common::create_test_app_with_turnstile(pool, Arc::new(FakeTurnstile))
}

/// The endpoints that must demand a Turnstile token, with a body that is
/// otherwise valid, so a 400 can only come from the bot check.
const PROTECTED: [&str; 4] = [
    "/api/v1/auth/register",
    "/api/v1/auth/login",
    "/api/v1/auth/resend-verification",
    "/api/v1/auth/forgot-password",
];

fn body_for(uri: &str, token: Option<&str>) -> Value {
    let mut body = json!({ "email": "e2e-bot@example.com" });
    if uri.ends_with("/login") {
        body["password"] = json!("hunter2hunter2");
    }
    if let Some(token) = token {
        body["turnstile_token"] = json!(token);
    }
    body
}

async fn post(app: &Router, uri: &str, body: &Value) -> (StatusCode, Value) {
    let resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-device-fingerprint", "test-device-abc123")
                .body(Body::from(serde_json::to_vec(body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn error_text(body: &Value) -> String {
    body.to_string()
}

#[sqlx::test]
async fn missing_token_is_rejected_on_every_protected_endpoint(pool: PgPool) {
    let app = app(pool);
    for uri in PROTECTED {
        let (status, body) = post(&app, uri, &body_for(uri, None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(
            error_text(&body).contains("Bot verification required"),
            "{uri}: {body}"
        );
    }
}

#[sqlx::test]
async fn blank_token_counts_as_missing(pool: PgPool) {
    let app = app(pool);
    for uri in PROTECTED {
        let (status, body) = post(&app, uri, &body_for(uri, Some("   "))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(
            error_text(&body).contains("Bot verification required"),
            "{uri}: {body}"
        );
    }
}

#[sqlx::test]
async fn failed_verification_is_rejected_on_every_protected_endpoint(pool: PgPool) {
    let app = app(pool);
    for uri in PROTECTED {
        let (status, body) = post(&app, uri, &body_for(uri, Some("bot-token"))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}: {body}");
        assert!(
            error_text(&body).contains("Bot verification failed"),
            "{uri}: {body}"
        );
    }
}

#[sqlx::test]
async fn valid_token_lets_each_request_through(pool: PgPool) {
    let app = app(pool);
    // Past the bot check each endpoint answers on its own terms. Order matters:
    // later requests see the account `register` creates.
    let expected = [
        ("/api/v1/auth/register", StatusCode::OK),
        // The account `register` just created has no password yet.
        ("/api/v1/auth/login", StatusCode::FORBIDDEN),
        // Inside the resend cooldown `register` just started: a 400 with
        // `COOLDOWN_ACTIVE`, not the bot check (see the assertion below).
        ("/api/v1/auth/resend-verification", StatusCode::BAD_REQUEST),
        ("/api/v1/auth/forgot-password", StatusCode::OK),
    ];
    for (uri, want) in expected {
        let (status, body) = post(&app, uri, &body_for(uri, Some(VALID))).await;
        assert_eq!(status, want, "{uri}: {body}");
        assert!(
            !error_text(&body).contains("Bot verification"),
            "{uri} was stopped by the bot check: {body}"
        );
    }
}

#[sqlx::test]
async fn rejected_registration_creates_no_account(pool: PgPool) {
    let app = app(pool.clone());

    let (status, _) = post(
        &app,
        "/api/v1/auth/register",
        &body_for("/api/v1/auth/register", Some("bot-token")),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(users, 0, "the bot check must run before any side effect");
}

#[sqlx::test]
async fn upstream_failure_fails_closed_without_leaking_details(pool: PgPool) {
    let app = app(pool);
    for uri in PROTECTED {
        let (status, body) = post(&app, uri, &body_for(uri, Some(UPSTREAM_DOWN))).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{uri}: {body}");
        let text = error_text(&body);
        assert!(!text.contains("siteverify"), "{uri} leaked: {body}");
        assert!(
            !text.contains("Turnstile verification error"),
            "{uri} leaked: {body}"
        );
    }
}
