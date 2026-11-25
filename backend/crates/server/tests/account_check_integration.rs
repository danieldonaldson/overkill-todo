//! A change made for another account than the session's is refused (OT-015).

mod common;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use domain::value_objects::UserRole;
use persistence_postgres::testing::insert_verified_user;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn send(
    app: &axum::Router,
    method: &str,
    session: Option<Uuid>,
    accounts: &[&str],
) -> (StatusCode, Value) {
    let mut builder = Request::builder().method(method).uri("/api/v1/lists");
    if let Some(user) = session {
        let token = common::mint_session_jwt(user, true, "user", 15);
        let csrf = common::mint_csrf_token(&user.to_string());
        builder = builder
            .header(
                header::COOKIE,
                format!("__Host-auth_token={token}; __Host-csrf_token={csrf}"),
            )
            .header("x-csrf-token", csrf);
    }
    for account in accounts {
        builder = builder.header("x-account-id", *account);
    }
    let body = if method == "POST" {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
        Body::from(json!({ "key": Uuid::now_v7(), "name": "Private" }).to_string())
    } else {
        Body::empty()
    };
    let resp = app
        .clone()
        .oneshot(builder.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn lists(app: &axum::Router, user: Uuid) -> usize {
    let (status, body) = send(app, "GET", Some(user), &[]).await;
    assert_eq!(status, StatusCode::OK);
    body.as_array().unwrap().len()
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_change_for_another_account_saves_nothing(pool: PgPool) {
    let a = insert_verified_user(&pool, "account-a@example.com", UserRole::User).await;
    let b = insert_verified_user(&pool, "account-b@example.com", UserRole::User).await;
    let app = common::create_test_app(pool);
    let (own, other) = (b.to_string(), a.to_string());

    for refused in [
        vec![other.as_str()],
        vec![""],
        vec!["not-a-uuid"],
        vec![own.as_str(), own.as_str()],
        vec![own.as_str(), other.as_str()],
    ] {
        let (status, body) = send(&app, "POST", Some(b), &refused).await;
        assert_eq!(status, StatusCode::PRECONDITION_FAILED, "{refused:?}");
        assert_eq!(body["code"], "account_changed", "{refused:?}");
        let text = body.to_string();
        assert!(!text.contains(&own) && !text.contains(&other), "{text}");
    }
    assert_eq!(lists(&app, a).await, 0);
    assert_eq!(lists(&app, b).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_own_account_or_none_is_handled_as_before(pool: PgPool) {
    let a = insert_verified_user(&pool, "account-own@example.com", UserRole::User).await;
    let app = common::create_test_app(pool);
    assert_eq!(
        send(&app, "POST", Some(a), &[&a.to_string()]).await.0,
        StatusCode::OK
    );
    assert_eq!(send(&app, "POST", Some(a), &[]).await.0, StatusCode::OK);
    assert_eq!(lists(&app, a).await, 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn reads_and_visitors_are_unaffected(pool: PgPool) {
    let a = insert_verified_user(&pool, "account-read@example.com", UserRole::User).await;
    let app = common::create_test_app(pool);
    let other = Uuid::now_v7().to_string();
    assert_eq!(
        send(&app, "GET", Some(a), &[&other]).await.0,
        StatusCode::OK
    );
    for method in ["GET", "POST"] {
        assert_eq!(
            send(&app, method, None, &[&other]).await,
            send(&app, method, None, &[]).await
        );
    }
}
