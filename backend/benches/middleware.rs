//! The production router's middleware stack, without a database.
//!
//! Builds the real app (`create_router` via `tests/common`) on a lazy pool that
//! never connects, then drives only requests that finish before any handler
//! touches Postgres: health, CORS preflight, auth/CSRF rejections, and the
//! verified-email gate. Together these cover trace-id, the crypto extension,
//! CORS, security headers, timeout, CSRF (header/cookie/HMAC + JWT), auth
//! (JWT verify), and `require_verified_email`.

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use criterion::{Criterion, criterion_group, criterion_main};
use sqlx::postgres::PgPoolOptions;
use tower::ServiceExt;
use uuid::Uuid;

const ORIGIN: &str = "http://localhost:4321";

fn app(rt: &tokio::runtime::Runtime) -> Router {
    // `connect_lazy` only parses the URL; nothing here opens a connection.
    let pool = rt.block_on(async {
        PgPoolOptions::new()
            .connect_lazy("postgres://bench:bench@127.0.0.1:1/never_connected")
            .expect("valid lazy pool URL")
    });
    common::create_test_app(pool)
}

/// Send one request and assert the status, so a routing or behaviour change
/// fails the bench loudly instead of silently measuring a different path.
async fn send(app: &Router, req: Request<Body>, expect: StatusCode) {
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), expect);
    black_box(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    );
}

#[allow(clippy::too_many_lines)] // One linear list of scenarios reads best here.
fn stack(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let app = app(&rt);

    let user_id = Uuid::now_v7();
    // Unverified, so the request passes CSRF + auth and is stopped by the
    // verified-email gate before the handler (and its DB call) runs.
    let unverified = common::mint_session_jwt(user_id, false, "user", 15);
    let csrf = common::mint_csrf_token(&user_id.to_string());

    let mut g = c.benchmark_group("middleware");

    g.bench_function("health", |b| {
        b.to_async(&rt).iter(|| {
            send(
                &app,
                Request::get("/health").body(Body::empty()).unwrap(),
                StatusCode::OK,
            )
        });
    });

    g.bench_function("cors_preflight", |b| {
        b.to_async(&rt).iter(|| {
            send(
                &app,
                Request::options("/api/v1/users/password/change")
                    .header(header::ORIGIN, ORIGIN)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "content-type,x-csrf-token",
                    )
                    .body(Body::empty())
                    .unwrap(),
                StatusCode::OK,
            )
        });
    });

    g.bench_function("get_no_token_401", |b| {
        b.to_async(&rt).iter(|| {
            send(
                &app,
                Request::get("/api/v1/auth/me").body(Body::empty()).unwrap(),
                StatusCode::UNAUTHORIZED,
            )
        });
    });

    // Bearer path: auth middleware verifies the JWT, then the email gate.
    g.bench_function("get_bearer_auth", |b| {
        b.to_async(&rt).iter(|| {
            send(
                &app,
                Request::get("/api/v1/users/me/export")
                    .header(header::AUTHORIZATION, format!("Bearer {unverified}"))
                    .body(Body::empty())
                    .unwrap(),
                StatusCode::FORBIDDEN,
            )
        });
    });

    // Cookie path, as the browser sends it.
    g.bench_function("get_cookie_auth", |b| {
        b.to_async(&rt).iter(|| {
            send(
                &app,
                Request::get("/api/v1/users/me/export")
                    .header(
                        header::COOKIE,
                        format!("auth_token={unverified}; csrf_token={csrf}"),
                    )
                    .body(Body::empty())
                    .unwrap(),
                StatusCode::FORBIDDEN,
            )
        });
    });

    // Mutating request: double-submit check, JWT verify (once, shared with
    // auth), CSRF HMAC, then the email gate.
    g.bench_function("post_csrf_auth", |b| {
        b.to_async(&rt).iter(|| {
            send(
                &app,
                Request::post("/api/v1/users/password/change")
                    .header(header::CONTENT_TYPE, "application/json")
                    .header(
                        header::COOKIE,
                        format!("auth_token={unverified}; csrf_token={csrf}"),
                    )
                    .header("x-csrf-token", &csrf)
                    .body(Body::from(r#"{"current_password":"a","new_password":"b"}"#))
                    .unwrap(),
                StatusCode::FORBIDDEN,
            )
        });
    });

    g.bench_function("post_csrf_missing_403", |b| {
        b.to_async(&rt).iter(|| {
            send(
                &app,
                Request::post("/api/v1/users/password/change")
                    .header(header::COOKIE, format!("auth_token={unverified}"))
                    .body(Body::empty())
                    .unwrap(),
                StatusCode::FORBIDDEN,
            )
        });
    });

    g.finish();
}

criterion_group!(benches, stack);
criterion_main!(benches);
