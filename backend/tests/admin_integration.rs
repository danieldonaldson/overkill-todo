//! Integration tests for the admin panel endpoints (`/api/v1/admin`).
//!
//! Authorization is driven by forged session JWTs (`mint_session_jwt`) carrying
//! the role under test — the same technique the security tests use. Mutating
//! routes additionally need a matching CSRF cookie + header.

mod common;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

/// GET request authenticated as a persisted user with `role`.
async fn admin_get(pool: &PgPool, role: &str, path: &str) -> (Uuid, Request<Body>) {
    let user_id = Uuid::now_v7();
    insert_requester(pool, user_id, role).await;
    let token = common::mint_session_jwt(user_id, true, role, 15);
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::empty())
        .unwrap();
    (user_id, req)
}

/// Mutating request (PUT/POST) authenticated as `role`, with a matching CSRF
/// cookie + header bound to the session user.
async fn admin_mutate(
    pool: &PgPool,
    method: &str,
    role: &str,
    path: &str,
    body: &Value,
) -> Request<Body> {
    let user_id = Uuid::now_v7();
    insert_requester(pool, user_id, role).await;
    let token = common::mint_session_jwt(user_id, true, role, 15);
    let csrf = common::mint_csrf_token(&user_id.to_string());
    Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!("auth_token={token}; csrf_token={csrf}"),
        )
        .header("x-csrf-token", csrf)
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

async fn insert_requester(pool: &PgPool, id: Uuid, role: &str) {
    sqlx::query(
        "INSERT INTO users (id, email, role, email_verified)
         VALUES ($1, $2, $3::user_role, true)",
    )
    .bind(id)
    .bind(format!("admin-requester-{id}@example.com"))
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
}

async fn json_body(resp: axum::http::Response<Body>) -> Value {
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[sqlx::test(migrations = "./migrations")]
async fn list_users_forbidden_for_non_admin(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let (_, req) = admin_get(&pool, "user", "/api/v1/admin/users").await;
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "./migrations")]
async fn role_change_forbidden_for_admin(pool: PgPool) {
    let target = insert_user(&pool, "promote-me@example.com").await;
    let app = common::create_test_app(pool.clone());
    // An `admin` (not super_admin) cannot use the role endpoint at all.
    let resp = app
        .oneshot(
            admin_mutate(
                &pool,
                "PUT",
                "admin",
                &format!("/api/v1/admin/users/{target}/role"),
                &json!({ "role": "admin" }),
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = "./migrations")]
async fn role_change_ok_for_super_admin(pool: PgPool) {
    let target = insert_user(&pool, "promote-ok@example.com").await;
    let app = common::create_test_app(pool.clone());
    let resp = app
        .oneshot(
            admin_mutate(
                &pool,
                "PUT",
                "super_admin",
                &format!("/api/v1/admin/users/{target}/role"),
                &json!({ "role": "admin" }),
            )
            .await,
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(json_body(resp).await["role"], "admin");
}

/// Insert a verified user directly, returning its id — for the routes that load
/// a target user (role change).
async fn insert_user(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query!(
        "INSERT INTO users (email, role, email_verified) VALUES ($1, 'user', true) RETURNING id",
        email
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .id
}

#[sqlx::test(migrations = "./migrations")]
async fn list_users_ok_for_admin(pool: PgPool) {
    insert_user(&pool, "listed@example.com").await;
    let app = common::create_test_app(pool.clone());
    let (_, req) = admin_get(&pool, "admin", "/api/v1/admin/users").await;
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = json_body(resp).await;
    assert!(
        body["users"]
            .as_array()
            .unwrap()
            .iter()
            .any(|u| { u["email"] == "listed@example.com" })
    );
}
