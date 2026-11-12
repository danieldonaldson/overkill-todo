//! The HTTP API keeps the promises its `OpenAPI` description makes (OT-002):
//! malformed requests are rejected with the standard JSON error body, never
//! axum's plain-text rejections.

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn insert_user_with_role(pool: &PgPool, role: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO users (id, email, role, email_verified)
         VALUES ($1, $2, $3::user_role, true)",
    )
    .bind(id)
    .bind(format!("contract-{id}@example.com"))
    .bind(role)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Assert a 400 whose body is the standard `{ code, message }` error.
async fn assert_standard_400(resp: axum::http::Response<Body>) {
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let content_type = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert!(
        content_type.starts_with("application/json"),
        "expected JSON, got {content_type:?}: {}",
        String::from_utf8_lossy(&bytes)
    );
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body["code"].is_string(), "missing code: {body}");
    assert!(body["message"].is_string(), "missing message: {body}");
}

#[sqlx::test(migrations = "./migrations")]
async fn malformed_json_body_gets_standard_error(pool: PgPool) {
    // No content type: axum's `Json` alone would answer 415 with plain text.
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/oauth/device-fingerprint")
        .body(Body::from("not json"))
        .unwrap();
    let resp = common::create_test_app(pool).oneshot(req).await.unwrap();
    assert_standard_400(resp).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn missing_query_parameter_gets_standard_error(pool: PgPool) {
    let req = Request::builder()
        .uri("/api/v1/auth/verify-magic")
        .body(Body::empty())
        .unwrap();
    let resp = common::create_test_app(pool).oneshot(req).await.unwrap();
    assert_standard_400(resp).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn malformed_query_parameter_gets_standard_error(pool: PgPool) {
    let admin = insert_user_with_role(&pool, "admin").await;
    let token = common::mint_session_jwt(admin, true, "admin", 15);
    let req = Request::builder()
        .uri("/api/v1/admin/users?limit=lots")
        .header(header::COOKIE, format!("auth_token={token}"))
        .body(Body::empty())
        .unwrap();
    let resp = common::create_test_app(pool).oneshot(req).await.unwrap();
    assert_standard_400(resp).await;
}

#[sqlx::test(migrations = "./migrations")]
async fn malformed_path_parameter_gets_standard_error(pool: PgPool) {
    let super_admin = insert_user_with_role(&pool, "super_admin").await;
    let token = common::mint_session_jwt(super_admin, true, "super_admin", 15);
    let csrf = common::mint_csrf_token(&super_admin.to_string());
    let req = Request::builder()
        .method("PUT")
        .uri("/api/v1/admin/users/not-a-uuid/role")
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!("auth_token={token}; csrf_token={csrf}"),
        )
        .header("x-csrf-token", csrf)
        .body(Body::from(r#"{"role":"admin"}"#))
        .unwrap();
    let resp = common::create_test_app(pool).oneshot(req).await.unwrap();
    assert_standard_400(resp).await;
}

/// Outside dev mode the description and its explorer aren't served, even in a
/// build with the `swagger-ui` feature (the test app never sets `DEV_MODE`).
#[sqlx::test(migrations = "./migrations")]
async fn description_is_not_served_outside_dev_mode(pool: PgPool) {
    let app = common::create_test_app(pool);
    for path in ["/docs", "/docs/", "/api-doc/openapi.json"] {
        let req = Request::builder().uri(path).body(Body::empty()).unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{path}");
    }
}
