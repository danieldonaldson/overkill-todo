//! HTTP-level coverage for lists and todos (OT-013 slice 1).
//!
//! Real requests through the wired app: session + CSRF + guard middleware, use
//! cases and Postgres.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use persistence_postgres::testing;
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

async fn body_json(body: Body) -> Value {
    let bytes = axum::body::to_bytes(body, usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

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

fn authed(method: &str, uri: &str, s: &Session, body: Option<&Value>, csrf: bool) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::COOKIE, cookie(s));
    if csrf {
        builder = builder.header("x-csrf-token", &s.csrf);
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    let body = body.map_or_else(Body::empty, |value| {
        Body::from(serde_json::to_vec(value).unwrap())
    });
    builder.body(body).unwrap()
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
    let token = common::cookie_value(&resp, "__Host-auth_token").expect("auth_token cookie");
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
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    body_json(resp.into_body()).await
}

#[sqlx::test(migrations = "../../migrations")]
async fn empty_lists_and_create_with_counts(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-lists@example.com").await;

    let resp = app
        .clone()
        .oneshot(authed("GET", "/api/v1/lists", &session, None, false))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await, json!([]));

    let list = create_list(&app, &session, "  Work ").await;
    assert_eq!(list["name"], "Work");
    assert_eq!(list["version"], 1);
    assert_eq!(list["open_count"], 0);
    assert_eq!(list["todo_count"], 0);

    let resp = app
        .clone()
        .oneshot(authed("GET", "/api/v1/lists", &session, None, false))
        .await
        .unwrap();
    let lists = body_json(resp.into_body()).await;
    assert_eq!(lists.as_array().unwrap().len(), 1);
    assert_eq!(lists[0]["name"], "Work");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_repeated_creation_key_returns_one_list(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-repeat@example.com").await;
    let key = uuid::Uuid::now_v7();
    let body = json!({ "key": key, "name": "Work" });

    let first = app
        .clone()
        .oneshot(authed("POST", "/api/v1/lists", &session, Some(&body), true))
        .await
        .unwrap();
    let first = body_json(first.into_body()).await;
    let second = app
        .clone()
        .oneshot(authed("POST", "/api/v1/lists", &session, Some(&body), true))
        .await
        .unwrap();
    let second = body_json(second.into_body()).await;
    assert_eq!(first["id"], second["id"]);

    let resp = app
        .clone()
        .oneshot(authed("GET", "/api/v1/lists", &session, None, false))
        .await
        .unwrap();
    assert_eq!(
        body_json(resp.into_body()).await.as_array().unwrap().len(),
        1
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn blank_names_and_titles_are_400(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-blank@example.com").await;

    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            "/api/v1/lists",
            &session,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "name": "  \n " })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let list = create_list(&app, &session, "Work").await;
    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            &format!("/api/v1/lists/{}/todos", list["id"].as_str().unwrap()),
            &session,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "title": "a\nb" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_limit_answers_409_and_names_the_limit(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let session = signup_and_session(&app, "e2e-limit@example.com").await;
    testing::insert_lists_for_email(&pool, "e2e-limit@example.com", 100).await;

    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            "/api/v1/lists",
            &session,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "name": "Over" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "limit_reached");
    assert!(body["message"].as_str().unwrap().contains("100"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_purged_accounts_unexpired_token_gets_401_on_create(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let session = signup_and_session(&app, "e2e-purged@example.com").await;
    testing::mark_user_deleted_by_email(&pool, "e2e-purged@example.com").await;

    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            "/api/v1/lists",
            &session,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "name": "Work" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "token_invalid");
}

#[sqlx::test(migrations = "../../migrations")]
async fn adding_and_marking_todos_orders_them_and_answers_409_on_stale(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-todos@example.com").await;
    let list = create_list(&app, &session, "Work").await;
    let list_id = list["id"].as_str().unwrap();

    let mut todo_ids = Vec::new();
    for title in ["first", "second", "third"] {
        let resp = app
            .clone()
            .oneshot(authed(
                "POST",
                &format!("/api/v1/lists/{list_id}/todos"),
                &session,
                Some(&json!({ "key": uuid::Uuid::now_v7(), "title": title })),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let todo = body_json(resp.into_body()).await;
        assert_eq!(todo["title"], title);
        assert_eq!(todo["done"], false);
        todo_ids.push(todo["id"].as_str().unwrap().to_string());
    }

    let first = &todo_ids[0];
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{first}"),
            &session,
            Some(&json!({ "version": 1, "done": true })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let done = body_json(resp.into_body()).await;
    assert_eq!(done["done"], true);
    assert_eq!(done["version"], 2);

    // The same state again succeeds with the version unchanged.
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{first}"),
            &session,
            Some(&json!({ "done": true, "version": 1 })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await["version"], 2);

    // A conflicting change is a 409 with the current row and code `stale`.
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{first}"),
            &session,
            Some(&json!({ "version": 1, "done": false })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let stale = body_json(resp.into_body()).await;
    assert_eq!(stale["code"], "stale");
    assert_eq!(stale["current"]["id"], *first);
    assert_eq!(stale["current"]["version"], 2);
    assert_eq!(stale["current"]["done"], true);

    // A body without a version is a 400.
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{first}"),
            &session,
            Some(&json!({ "done": false })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    // Contract order: open todos by position, then done newest first.
    let resp = app
        .clone()
        .oneshot(authed(
            "GET",
            &format!("/api/v1/lists/{list_id}"),
            &session,
            None,
            false,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let detail = body_json(resp.into_body()).await;
    let titles: Vec<_> = detail["todos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, vec!["second", "third", "first"]);
    assert_eq!(detail["list"]["open_count"], 2);
    assert_eq!(detail["list"]["todo_count"], 3);
}

#[sqlx::test(migrations = "../../migrations")]
async fn todo_limit_answers_409_and_keeps_the_list_at_1000(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let session = signup_and_session(&app, "e2e-todo-limit@example.com").await;
    let list = create_list(&app, &session, "Work").await;
    let list_id = list["id"].as_str().unwrap();
    let list_uuid: uuid::Uuid = list_id.parse().unwrap();
    testing::insert_todos_for_list(&pool, list_uuid, 1000).await;

    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            &format!("/api/v1/lists/{list_id}/todos"),
            &session,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "title": "Over" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body = body_json(resp.into_body()).await;
    assert_eq!(body["code"], "limit_reached");
    assert!(body["message"].as_str().unwrap().contains("1,000"));
}

#[sqlx::test(migrations = "../../migrations")]
async fn unknown_and_foreign_ids_share_the_404_codes(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let session = signup_and_session(&app, "e2e-codes@example.com").await;
    let other = signup_and_session(&app, "e2e-other@example.com").await;
    let other_list = create_list(&app, &other, "Other's").await;
    let other_id = other_list["id"].as_str().unwrap();
    let unknown = uuid::Uuid::now_v7();

    for id in [other_id, &unknown.to_string()] {
        let resp = app
            .clone()
            .oneshot(authed(
                "GET",
                &format!("/api/v1/lists/{id}"),
                &session,
                None,
                false,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = body_json(resp.into_body()).await;
        assert_eq!(body["code"], "list_not_found");

        let resp = app
            .clone()
            .oneshot(authed(
                "POST",
                &format!("/api/v1/lists/{id}/todos"),
                &session,
                Some(&json!({ "key": uuid::Uuid::now_v7(), "title": "x" })),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(resp.into_body()).await["code"], "list_not_found");

        for (method, uri, body) in [
            (
                "PATCH",
                format!("/api/v1/lists/{id}"),
                Some(json!({ "version": 1, "name": "x" })),
            ),
            (
                "DELETE",
                format!("/api/v1/lists/{id}?version=1&todo_count=0"),
                None,
            ),
        ] {
            let resp = app
                .clone()
                .oneshot(authed(method, &uri, &session, body.as_ref(), true))
                .await
                .unwrap();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{method} {uri}");
            assert_eq!(body_json(resp.into_body()).await["code"], "list_not_found");
        }
    }

    let own = create_list(&app, &session, "Mine").await;
    let own_id = own["id"].as_str().unwrap();
    let own_todo = app
        .clone()
        .oneshot(authed(
            "POST",
            &format!("/api/v1/lists/{own_id}/todos"),
            &session,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "title": "mine" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(own_todo.status(), StatusCode::OK);

    for todo_id in [other_id, &unknown.to_string()] {
        let resp = app
            .clone()
            .oneshot(authed(
                "PATCH",
                &format!("/api/v1/lists/{own_id}/todos/{todo_id}"),
                &session,
                Some(&json!({ "version": 1, "done": true })),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(resp.into_body()).await["code"], "todo_not_found");

        let resp = app
            .clone()
            .oneshot(authed(
                "DELETE",
                &format!("/api/v1/lists/{own_id}/todos/{todo_id}?version=1"),
                &session,
                None,
                true,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        assert_eq!(body_json(resp.into_body()).await["code"], "todo_not_found");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn cors_allows_patch_and_delete(pool: PgPool) {
    let app = common::create_test_app(pool);
    for method in ["PATCH", "DELETE"] {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/api/v1/lists")
                    .header(header::ORIGIN, "http://localhost:4321")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, method)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let allowed = resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_METHODS)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(allowed.contains(method), "allow-methods: {allowed}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn renaming_and_deleting_lists_and_todos(pool: PgPool) {
    let app = common::create_test_app(pool);
    let session = signup_and_session(&app, "e2e-rename@example.com").await;
    let list = create_list(&app, &session, "Work").await;
    let list_id = list["id"].as_str().unwrap().to_string();

    // Rename the list, then the same name again at the old version.
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}"),
            &session,
            Some(&json!({ "version": 1, "name": "  Job " })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let renamed = body_json(resp.into_body()).await;
    assert_eq!(renamed["name"], "Job");
    assert_eq!(renamed["version"], 2);

    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}"),
            &session,
            Some(&json!({ "version": 1, "name": "Job" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(body_json(resp.into_body()).await["version"], 2);

    // A different name at the old version is a 409 naming the current list.
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}"),
            &session,
            Some(&json!({ "version": 1, "name": "Other" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let stale = body_json(resp.into_body()).await;
    assert_eq!(stale["code"], "stale");
    assert_eq!(stale["current"]["name"], "Job");
    assert_eq!(stale["current"]["version"], 2);

    // A body without a version, and a blank name, are 400s.
    for body in [
        json!({ "name": "x" }),
        json!({ "version": 2, "name": " \n " }),
    ] {
        let resp = app
            .clone()
            .oneshot(authed(
                "PATCH",
                &format!("/api/v1/lists/{list_id}"),
                &session,
                Some(&body),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // Add a todo, rename it, and check exactly one field per PATCH.
    let resp = app
        .clone()
        .oneshot(authed(
            "POST",
            &format!("/api/v1/lists/{list_id}/todos"),
            &session,
            Some(&json!({ "key": uuid::Uuid::now_v7(), "title": "Buy milk" })),
            true,
        ))
        .await
        .unwrap();
    let todo = body_json(resp.into_body()).await;
    let todo_id = todo["id"].as_str().unwrap().to_string();

    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}"),
            &session,
            Some(&json!({ "version": 1, "title": "Buy oat milk" })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let renamed = body_json(resp.into_body()).await;
    assert_eq!(renamed["title"], "Buy oat milk");
    assert_eq!(renamed["version"], 2);
    assert_eq!(renamed["done"], false);

    for body in [
        json!({ "version": 2, "title": "x", "done": true }),
        json!({ "version": 2 }),
        json!({ "title": "x" }),
    ] {
        let resp = app
            .clone()
            .oneshot(authed(
                "PATCH",
                &format!("/api/v1/lists/{list_id}/todos/{todo_id}"),
                &session,
                Some(&body),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST, "body: {body}");
    }

    // A mark at the old version is stale and names the current title.
    let resp = app
        .clone()
        .oneshot(authed(
            "PATCH",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}"),
            &session,
            Some(&json!({ "version": 1, "done": true })),
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let stale = body_json(resp.into_body()).await;
    assert_eq!(stale["current"]["title"], "Buy oat milk");
    assert_eq!(stale["current"]["version"], 2);
    assert_eq!(stale["current"]["done"], false);

    // Delete the todo: stale first, then gone, then 404 on a repeat.
    let resp = app
        .clone()
        .oneshot(authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}?version=1"),
            &session,
            None,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    assert_eq!(body_json(resp.into_body()).await["current"]["version"], 2);

    let resp = app
        .clone()
        .oneshot(authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}?version=2"),
            &session,
            None,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}/todos/{todo_id}?version=2"),
            &session,
            None,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    assert_eq!(body_json(resp.into_body()).await["code"], "todo_not_found");

    // Delete the list: a wrong count deletes nothing and names the count.
    for title in ["one", "two"] {
        let resp = app
            .clone()
            .oneshot(authed(
                "POST",
                &format!("/api/v1/lists/{list_id}/todos"),
                &session,
                Some(&json!({ "key": uuid::Uuid::now_v7(), "title": title })),
                true,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    let resp = app
        .clone()
        .oneshot(authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}?version=2&todo_count=0"),
            &session,
            None,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let stale = body_json(resp.into_body()).await;
    assert_eq!(stale["current"]["todo_count"], 2);
    assert_eq!(stale["current"]["name"], "Job");

    // A missing version is a 400.
    let resp = app
        .clone()
        .oneshot(authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}?todo_count=2"),
            &session,
            None,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

    let resp = app
        .clone()
        .oneshot(authed(
            "DELETE",
            &format!("/api/v1/lists/{list_id}?version=2&todo_count=2"),
            &session,
            None,
            true,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let resp = app
        .clone()
        .oneshot(authed(
            "GET",
            &format!("/api/v1/lists/{list_id}"),
            &session,
            None,
            false,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
