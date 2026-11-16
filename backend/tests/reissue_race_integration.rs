//! P-11 under concurrency: no code-issuing path can lift a lock that a
//! concurrent wrong guess has just set (OT-010 slice 4).
//!
//! Each issuing path reads the current code, decides with `reissue`, hashes a
//! new code (Argon2, tens of milliseconds) and then writes it. A 5th wrong
//! guess that claims the last attempt inside that gap locks the code. The write
//! must then refuse to replace it, because the lock's window has only just
//! started.
//!
//! Every round starts from a code with 4 failed attempts, sent 6 minutes ago:
//! past the 5-minute resend cooldown, inside the 15-minute lock window. It
//! fires the 5th wrong guess and an issuing request at once, over the real
//! router and Postgres. Whatever the interleaving, an issued code may replace
//! the row only while it is below the cap. Replacing a row at the cap bumps
//! `lockouts`, so `lockouts > 0` afterwards means a lock inside its window was
//! overwritten.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

/// Rounds per path. Each is one race between a guess and an issue.
const ROUNDS: usize = 40;

/// A wrong guess: the `e2e-` test emails always get the fixed code `123456`.
const WRONG_CODE: &str = "000000";

fn post_json(uri: &str, body: &Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", "test-device-abc123")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn post(app: &axum::Router, uri: &str, body: Value) -> StatusCode {
    app.clone()
        .oneshot(post_json(uri, &body))
        .await
        .unwrap()
        .status()
}

async fn user_id(pool: &PgPool, email: &str) -> uuid::Uuid {
    sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM users WHERE email = $1")
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A code table and the requests that race on it.
struct Path {
    table: &'static str,
    /// Sets the row to 4 attempts, no lockouts, sent 6 minutes ago, live.
    arm: &'static str,
    /// Reads `(attempts, lockouts)`.
    read: &'static str,
    guess_uri: &'static str,
    issue_uri: &'static str,
}

const VERIFY_VS_RESEND: Path = Path {
    table: "email_verification_codes",
    arm: "UPDATE email_verification_codes SET attempts = 4, lockouts = 0, \
          last_sent_at = NOW() - INTERVAL '6 minutes', \
          expires_at = NOW() + INTERVAL '10 minutes' WHERE user_id = $1",
    read: "SELECT attempts, lockouts FROM email_verification_codes WHERE user_id = $1",
    guess_uri: "/api/v1/auth/verify-email",
    issue_uri: "/api/v1/auth/resend-verification",
};

const VERIFY_VS_REREGISTER: Path = Path {
    table: "email_verification_codes",
    arm: "UPDATE email_verification_codes SET attempts = 4, lockouts = 0, \
          last_sent_at = NOW() - INTERVAL '6 minutes', \
          expires_at = NOW() + INTERVAL '10 minutes' WHERE user_id = $1",
    read: "SELECT attempts, lockouts FROM email_verification_codes WHERE user_id = $1",
    guess_uri: "/api/v1/auth/verify-email",
    issue_uri: "/api/v1/auth/register",
};

const RESET_VS_FORGOT: Path = Path {
    table: "password_reset_codes",
    arm: "UPDATE password_reset_codes SET attempts = 4, lockouts = 0, \
          last_sent_at = NOW() - INTERVAL '6 minutes', \
          expires_at = NOW() + INTERVAL '10 minutes' WHERE user_id = $1",
    read: "SELECT attempts, lockouts FROM password_reset_codes WHERE user_id = $1",
    guess_uri: "/api/v1/auth/verify-reset-code",
    issue_uri: "/api/v1/auth/forgot-password",
};

/// Runs `ROUNDS` races on `path` and returns the `(attempts, lockouts)` each
/// round ended on.
async fn race(app: &axum::Router, pool: &PgPool, path: &Path, email: &str) -> Vec<(i32, i32)> {
    let id = user_id(pool, email).await;
    let mut ends = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let armed = sqlx::query(path.arm).bind(id).execute(pool).await.unwrap();
        assert_eq!(armed.rows_affected(), 1, "no {} row to arm", path.table);

        let guess = tokio::spawn({
            let app = app.clone();
            let (uri, body) = (
                path.guess_uri,
                json!({ "email": email, "code": WRONG_CODE }),
            );
            async move { post(&app, uri, body).await }
        });
        let issue = tokio::spawn({
            let app = app.clone();
            let (uri, body) = (path.issue_uri, json!({ "email": email }));
            async move { post(&app, uri, body).await }
        });
        assert_eq!(guess.await.unwrap(), StatusCode::BAD_REQUEST);
        assert_eq!(issue.await.unwrap(), StatusCode::OK);

        ends.push(
            sqlx::query_as::<_, (i32, i32)>(path.read)
                .bind(id)
                .fetch_one(pool)
                .await
                .unwrap(),
        );
    }
    ends
}

fn assert_no_lock_overwritten(path: &Path, ends: &[(i32, i32)]) {
    let overwritten = ends.iter().filter(|(_, lockouts)| *lockouts > 0).count();
    assert_eq!(
        overwritten,
        0,
        "{}: {overwritten} of {} rounds replaced a code locked inside its window \
         (attempts, lockouts per round: {ends:?})",
        path.issue_uri,
        ends.len()
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn resend_racing_the_locking_guess_keeps_the_lock(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-race-resend@example.com";
    assert_eq!(
        post(&app, "/api/v1/auth/register", json!({ "email": email })).await,
        StatusCode::OK
    );
    let ends = race(&app, &pool, &VERIFY_VS_RESEND, email).await;
    assert_no_lock_overwritten(&VERIFY_VS_RESEND, &ends);
}

#[sqlx::test(migrations = "./migrations")]
async fn reregister_racing_the_locking_guess_keeps_the_lock(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-race-register@example.com";
    assert_eq!(
        post(&app, "/api/v1/auth/register", json!({ "email": email })).await,
        StatusCode::OK
    );
    let ends = race(&app, &pool, &VERIFY_VS_REREGISTER, email).await;
    assert_no_lock_overwritten(&VERIFY_VS_REREGISTER, &ends);
}

#[sqlx::test(migrations = "./migrations")]
async fn reset_request_racing_the_locking_guess_keeps_the_lock(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-race-reset@example.com";
    assert_eq!(
        post(&app, "/api/v1/auth/register", json!({ "email": email })).await,
        StatusCode::OK
    );
    assert_eq!(
        post(
            &app,
            "/api/v1/auth/forgot-password",
            json!({ "email": email })
        )
        .await,
        StatusCode::OK
    );
    let ends = race(&app, &pool, &RESET_VS_FORGOT, email).await;
    assert_no_lock_overwritten(&RESET_VS_FORGOT, &ends);
}
