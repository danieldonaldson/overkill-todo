//! Full requests through the real router against Postgres.
//!
//! Needs a reachable server at `DATABASE_URL` (`just bench-db` starts the
//! compose DB). It recreates a dedicated `<db>_bench` database on every run,
//! migrates it, and seeds users through the API, so results don't depend on
//! whatever is in the dev database. Without `DATABASE_URL` the bench prints a
//! note and exits successfully, so `cargo bench` works anywhere.
//!
//! Argon2 uses the cheap test parameters here; its production cost is measured
//! in the `crypto` bench. These numbers are dominated by query latency, so
//! compare them against a baseline taken on the same machine.

#[path = "../tests/common/mod.rs"]
mod common;

use std::hint::black_box;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use criterion::{Criterion, criterion_group, criterion_main};
use overkill_todo::application::ports::UserRepository;
use overkill_todo::domain::value_objects::UserRole;
use overkill_todo::infrastructure::persistence::{Database, PostgresUserRepository};
use serde_json::{Value, json};
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use tower::ServiceExt;

const PASSWORD: &str = "hunter2hunter2";
const FINGERPRINT: &str = "bench-device-0123456789abcdef";
/// Rows behind the admin list, so pagination does real work.
const SEEDED_USERS: usize = 200;

/// A logged-in browser: session cookie, CSRF token, and refresh cookie.
#[derive(Clone)]
struct Session {
    auth: String,
    refresh: String,
}

fn bench_db_url() -> Option<String> {
    let base = std::env::var("DATABASE_URL").ok()?;
    // Swap the database name (last path segment) for a dedicated bench DB.
    let (prefix, rest) = base.rsplit_once('/')?;
    let (name, query) = rest.split_once('?').map_or((rest, ""), |(n, q)| (n, q));
    let sep = if query.is_empty() { "" } else { "?" };
    Some(format!("{prefix}/{name}_bench{sep}{query}"))
}

async fn fresh_pool(url: &str) -> PgPool {
    if Postgres::database_exists(url).await.unwrap() {
        Postgres::drop_database(url).await.unwrap();
    }
    Postgres::create_database(url).await.unwrap();
    let pool = PgPool::connect(url).await.unwrap();
    sqlx::migrate!("./migrations").run(&pool).await.unwrap();
    pool
}

fn json_post(uri: &str, body: &Value) -> Request<Body> {
    Request::post(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", FINGERPRINT)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn ok_json(app: &Router, req: Request<Body>) -> (axum::http::Response<Body>, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let (parts, body) = resp.into_parts();
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "unexpected status, body: {value}");
    (
        axum::http::Response::from_parts(parts, Body::empty()),
        value,
    )
}

fn session_from(resp: &axum::http::Response<Body>) -> Session {
    Session {
        auth: common::cookie_value(resp, "auth_token").expect("auth_token cookie"),
        refresh: common::cookie_value(resp, "refresh_token").expect("refresh_token cookie"),
    }
}

/// Register → verify (fixed code for `test-` emails) → complete signup.
async fn sign_up(app: &Router, email: &str) {
    ok_json(
        app,
        json_post("/api/v1/auth/register", &json!({ "email": email })),
    )
    .await;
    let (_, body) = ok_json(
        app,
        json_post(
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
        ),
    )
    .await;
    let signup_token = body["signup_token"].as_str().unwrap();
    ok_json(
        app,
        json_post(
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": PASSWORD }),
        ),
    )
    .await;
}

async fn log_in(app: &Router, email: &str) -> Session {
    let (resp, _) = ok_json(
        app,
        json_post(
            "/api/v1/auth/login",
            &json!({ "email": email, "password": PASSWORD }),
        ),
    )
    .await;
    session_from(&resp)
}

async fn promote_to_super_admin(pool: &PgPool, email: &str) {
    let users = PostgresUserRepository::new(Arc::new(Database::from_pool(pool.clone())));
    let mut user = users.find_by_email(email).await.unwrap().unwrap();
    user.set_role(UserRole::SuperAdmin);
    users.save_role_change(&user, false).await.unwrap();
}

async fn seed(app: &Router, pool: &PgPool) -> (Session, Session) {
    for i in 0..SEEDED_USERS {
        sign_up(app, &format!("test-bench-{i}@example.com")).await;
    }
    let admin = "test-bench-admin@example.com";
    sign_up(app, admin).await;
    promote_to_super_admin(pool, admin).await;
    // The role change revoked earlier sessions, and a token is only accepted if
    // issued in a later whole second than the cutoff, so wait one out first.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    (
        log_in(app, "test-bench-0@example.com").await,
        log_in(app, admin).await,
    )
}

fn get(uri: &str, s: &Session) -> Request<Body> {
    Request::get(uri)
        .header(header::COOKIE, format!("auth_token={}", s.auth))
        .body(Body::empty())
        .unwrap()
}

async fn expect_ok(app: &Router, req: Request<Body>) {
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    black_box(
        axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap(),
    );
}

fn http(c: &mut Criterion) {
    let Some(url) = bench_db_url() else {
        eprintln!("http bench skipped: DATABASE_URL is not set (try `just bench-db`)");
        return;
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let pool = rt.block_on(fresh_pool(&url));
    let app = common::create_test_app(pool.clone());
    let (user, admin) = rt.block_on(seed(&app, &pool));
    let refresh = Arc::new(Mutex::new(user.refresh.clone()));

    let mut g = c.benchmark_group("http");

    g.bench_function("health_ready", |b| {
        b.to_async(&rt).iter(|| {
            expect_ok(
                &app,
                Request::get("/health/ready").body(Body::empty()).unwrap(),
            )
        });
    });

    g.bench_function("me", |b| {
        b.to_async(&rt)
            .iter(|| expect_ok(&app, get("/api/v1/auth/me", &user)));
    });

    g.bench_function("export", |b| {
        b.to_async(&rt)
            .iter(|| expect_ok(&app, get("/api/v1/users/me/export", &user)));
    });

    g.bench_function("admin_list_users_50", |b| {
        b.to_async(&rt)
            .iter(|| expect_ok(&app, get("/api/v1/admin/users?limit=50&offset=100", &admin)));
    });

    g.bench_function("login", |b| {
        b.to_async(&rt).iter(|| {
            expect_ok(
                &app,
                json_post(
                    "/api/v1/auth/login",
                    &json!({ "email": "test-bench-1@example.com", "password": PASSWORD }),
                ),
            )
        });
    });

    // Rotation invalidates the presented token, so each iteration chains off
    // the cookie the previous one returned.
    g.bench_function("refresh_rotate", |b| {
        b.to_async(&rt).iter(|| {
            let refresh = Arc::clone(&refresh);
            let app = app.clone();
            async move {
                let current = refresh.lock().unwrap().clone();
                let req = Request::post("/api/v1/auth/refresh")
                    .header(header::COOKIE, format!("refresh_token={current}"))
                    .header("x-device-fingerprint", FINGERPRINT)
                    .body(Body::empty())
                    .unwrap();
                let resp = app.oneshot(req).await.unwrap();
                assert_eq!(resp.status(), StatusCode::OK);
                *refresh.lock().unwrap() =
                    common::cookie_value(&resp, "refresh_token").expect("rotated refresh cookie");
            }
        });
    });

    g.finish();
}

criterion_group!(benches, http);
criterion_main!(benches);
