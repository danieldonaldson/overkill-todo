//! Data export includes lists and todos, streamed one list at a time (OT-013
//! design D10, task 1.9).

mod common;

use std::time::Duration;

use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode, header},
};
use persistence_postgres::testing;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

struct Session {
    token: String,
    csrf: String,
}

fn cookie(s: &Session) -> String {
    format!(
        "__Host-auth_token={}; __Host-csrf_token={}",
        s.token, s.csrf
    )
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

fn authed(method: &str, uri: &str, s: &Session, body: Option<&Value>) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie(s));
    if body.is_some() {
        builder = builder
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-csrf-token", &s.csrf);
    }
    let body = body.map_or_else(Body::empty, |value| {
        Body::from(serde_json::to_vec(value).unwrap())
    });
    builder.body(body).unwrap()
}

async fn body_json(body: Body) -> Value {
    let bytes = to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn signup_and_session(app: &axum::Router, email: &str) -> Session {
    app.clone()
        .oneshot(post_json(
            "/api/v1/auth/register",
            &json!({ "email": email }),
        ))
        .await
        .unwrap();
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
            &json!({ "signup_token": signup_token, "email": email, "password": "hunter2hunter2" }),
        ))
        .await
        .unwrap();
    let token = common::cookie_value(&resp, "__Host-auth_token").unwrap();
    let csrf = body_json(resp.into_body()).await["csrf_token"]
        .as_str()
        .unwrap()
        .to_string();
    Session { token, csrf }
}

async fn create_list(app: &axum::Router, s: &Session, name: &str) -> Value {
    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            "/api/v1/lists",
            s,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "name": name })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

async fn add_todo(app: &axum::Router, s: &Session, list: &Value, title: &str) -> Value {
    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            &format!("/api/v1/lists/{}/todos", list["id"].as_str().unwrap()),
            s,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "title": title })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

async fn export(app: &axum::Router, s: &Session) -> axum::http::Response<Body> {
    app.clone()
        .oneshot(authed("GET", "/api/v1/users/me/export", s, None))
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn exports_open_and_done_todos_in_order(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-export@example.com").await;
    let list = create_list(&app, &session, "Work").await;
    let first = add_todo(&app, &session, &list, "first").await;
    add_todo(&app, &session, &list, "second").await;
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!(
                "/api/v1/lists/{}/todos/{}",
                list["id"].as_str().unwrap(),
                first["id"].as_str().unwrap()
            ),
            &session,
            Some(&json!({ "version": 1, "done": true })),
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = export(&app, &session).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["lists"].as_array().unwrap().len(), 1);
    let exported = &body["lists"][0];
    assert_eq!(exported["name"], "Work");
    let titles: Vec<_> = exported["todos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["second", "first"]);
    assert_eq!(exported["todos"][1]["done"], true);
    assert!(exported["todos"][1]["completed_at"].is_string());
    assert!(!exported["todos"][0]["done"].as_bool().unwrap());
}

#[sqlx::test(migrations = "../../migrations")]
async fn export_with_no_lists_succeeds_with_an_empty_set(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-export-empty@example.com").await;
    let resp = export(&app, &session).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await["lists"], json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn export_contains_only_the_callers_lists(pool: PgPool) {
    let app = common::create_test_app(pool);
    let mine = signup_and_session(&app, "e2e-export-mine@example.com").await;
    let other = signup_and_session(&app, "e2e-export-other@example.com").await;
    create_list(&app, &other, "Not yours").await;
    create_list(&app, &mine, "Mine").await;

    let body = body_json(export(&app, &mine).await.into_body()).await;
    let lists = body["lists"].as_array().unwrap();
    assert_eq!(lists.len(), 1);
    assert_eq!(lists[0]["name"], "Mine");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_failure_part_way_aborts_the_response_after_the_audit_event(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let session = signup_and_session(&app, "e2e-export-fail@example.com").await;
    let first = create_list(&app, &session, "First").await;
    add_todo(&app, &session, &first, "fine").await;
    let second = create_list(&app, &session, "Second").await;
    let bad = add_todo(&app, &session, &second, "fine too").await;
    // A row the domain would never write, so only the second list's fetch
    // fails. The header and first list have already been streamed.
    testing::corrupt_todo_title(
        &pool,
        bad["id"].as_str().unwrap().parse::<uuid::Uuid>().unwrap(),
    )
    .await;

    let resp = export(&app, &session).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let result = to_bytes(resp.into_body(), usize::MAX).await;
    assert!(
        result.is_err(),
        "the body must fail, not end as partial JSON"
    );

    let exported = testing::count_audit_events_for_email(
        &pool,
        "e2e-export-fail@example.com",
        "data_exported",
    )
    .await;
    assert_eq!(
        exported, 1,
        "the audit event is recorded before the first byte"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_list_deleted_during_the_export_is_left_out(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-export-deleted@example.com").await;
    let mut lists = Vec::new();
    for n in 0..6 {
        lists.push(create_list(&app, &session, &format!("List {n}")).await);
    }

    // Nothing reads the body yet, so the stream waits after the first lists.
    let resp = export(&app, &session).await;
    assert_eq!(resp.status(), StatusCode::OK);
    for gone in [&lists[3], &lists[5]] {
        let deleted = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!(
                        "/api/v1/lists/{}?version=1&todo_count=0",
                        gone["id"].as_str().unwrap()
                    ))
                    .header(header::COOKIE, cookie(&session))
                    .header("x-csrf-token", &session.csrf)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(deleted.status(), StatusCode::NO_CONTENT);
    }

    let body = body_json(resp.into_body()).await;
    let names: Vec<_> = body["lists"]
        .as_array()
        .unwrap()
        .iter()
        .map(|list| list["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["List 0", "List 1", "List 2", "List 4"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_request_timeout_does_not_cut_off_a_slow_body(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-export-slow@example.com").await;
    let list = create_list(&app, &session, "Slow").await;
    add_todo(&app, &session, &list, "one").await;

    let resp = export(&app, &session).await;
    assert_eq!(resp.status(), StatusCode::OK);
    // The route's 30-second timeout covers the handler's response future, not
    // the body. Reading slowly past it must still deliver the whole export.
    tokio::time::sleep(Duration::from_secs(31)).await;
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["lists"][0]["name"], "Slow");
}
