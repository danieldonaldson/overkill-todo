//! Security property P-35 (OT-013): lists and todos are private to their
//! Member, and staff reach them exactly like a Member — never another
//! account's. Every check compares a foreign ID with an unknown one, so a
//! response can't reveal that the foreign row exists.

mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode, header},
};
use domain::value_objects::UserRole;
use persistence_postgres::testing::{insert_list, insert_todo, insert_verified_user};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;

struct Session {
    token: String,
    csrf: String,
}

impl Session {
    fn for_user(user_id: uuid::Uuid, verified: bool, role: &str) -> Self {
        let token = common::mint_session_jwt(user_id, verified, role, 15);
        let csrf = common::mint_csrf_token(&user_id.to_string());
        Self { token, csrf }
    }
}

fn cookie(s: &Session) -> String {
    format!(
        "__Host-auth_token={}; __Host-csrf_token={}",
        s.token, s.csrf
    )
}

fn request(
    method: &str,
    uri: &str,
    session: Option<&Session>,
    body: Option<&Value>,
    csrf: bool,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(session) = session {
        builder = builder.header(header::COOKIE, cookie(session));
    }
    if body.is_some() {
        builder = builder.header(header::CONTENT_TYPE, "application/json");
    }
    if csrf && let Some(session) = session {
        builder = builder.header("x-csrf-token", &session.csrf);
    }
    let body = body.map_or_else(Body::empty, |value| {
        Body::from(serde_json::to_vec(value).unwrap())
    });
    builder.body(body).unwrap()
}

async fn response(app: &axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let resp = app.clone().oneshot(request).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// A verified member, another member with a private list and todo, and an
/// unknown pair of IDs.
struct Fixture {
    app: axum::Router,
    member: Session,
    other: Session,
    other_list: String,
    other_todo: String,
    own_list: uuid::Uuid,
    own_todo: uuid::Uuid,
    unknown_list: String,
    unknown_todo: String,
}

/// The other Member still sees exactly their one untouched todo.
async fn assert_other_untouched(f: &Fixture) {
    let uri = format!("/api/v1/lists/{}", f.other_list);
    let (status, body) = response(&f.app, request("GET", &uri, Some(&f.other), None, false)).await;
    assert_eq!(status, StatusCode::OK);
    let todos = body["todos"].as_array().unwrap();
    assert_eq!(todos.len(), 1);
    assert_eq!(todos[0]["id"], f.other_todo);
    assert_eq!(todos[0]["title"], "Other's todo");
    assert_eq!(todos[0]["version"], 1);
    assert_eq!(todos[0]["done"], false);
}

async fn fixture(pool: PgPool) -> Fixture {
    let app = common::create_test_app(pool.clone());
    let member_id = insert_verified_user(&pool, "e2e-p35-member@example.com", UserRole::User).await;
    let other_id = insert_verified_user(&pool, "e2e-p35-other@example.com", UserRole::User).await;
    let other_list_id = insert_list(&pool, other_id, "Other's list").await;
    let other_todo_id = insert_todo(&pool, other_list_id, "Other's todo", Some(1), None).await;
    let own_list_id = insert_list(&pool, member_id, "My list").await;
    let own_todo_id = insert_todo(&pool, own_list_id, "My todo", Some(1), None).await;

    Fixture {
        app,
        member: Session::for_user(member_id, true, "user"),
        other: Session::for_user(other_id, true, "user"),
        other_list: other_list_id.to_string(),
        other_todo: other_todo_id.to_string(),
        own_list: own_list_id,
        own_todo: own_todo_id,
        unknown_list: uuid::Uuid::now_v7().to_string(),
        unknown_todo: uuid::Uuid::now_v7().to_string(),
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn another_members_ids_answer_exactly_like_unknown_ids(pool: PgPool) {
    let f = fixture(pool).await;
    let own = f.own_list;

    for (foreign, unknown) in [
        (f.other_list.clone(), f.unknown_list.clone()),
        (f.unknown_list.clone(), uuid::Uuid::now_v7().to_string()),
    ] {
        let paths: Vec<(&str, String, Option<Value>, bool)> = vec![
            ("GET", format!("/api/v1/lists/{foreign}"), None, false),
            (
                "PATCH",
                format!("/api/v1/lists/{foreign}"),
                Some(json!({ "version": 1, "name": "x" })),
                true,
            ),
            (
                "DELETE",
                format!("/api/v1/lists/{foreign}?version=1&todo_count=0"),
                None,
                true,
            ),
            (
                "POST",
                format!("/api/v1/lists/{foreign}/todos"),
                Some(json!({ "key": uuid::Uuid::now_v7(), "title": "x" })),
                true,
            ),
            (
                "PATCH",
                format!("/api/v1/lists/{foreign}/todos/{}", f.other_todo),
                Some(json!({ "version": 1, "done": true })),
                true,
            ),
            (
                "PATCH",
                format!("/api/v1/lists/{foreign}/todos/{}", f.other_todo),
                Some(json!({ "version": 1, "title": "x" })),
                true,
            ),
            (
                "POST",
                format!("/api/v1/lists/{foreign}/todos/{}/move", f.other_todo),
                Some(json!({ "above": [], "below": [] })),
                true,
            ),
            (
                "DELETE",
                format!("/api/v1/lists/{foreign}/todos/{}?version=1", f.other_todo),
                None,
                true,
            ),
        ];
        let unknown_paths: Vec<(&str, String, Option<Value>, bool)> = vec![
            ("GET", format!("/api/v1/lists/{unknown}"), None, false),
            (
                "PATCH",
                format!("/api/v1/lists/{unknown}"),
                Some(json!({ "version": 1, "name": "x" })),
                true,
            ),
            (
                "DELETE",
                format!("/api/v1/lists/{unknown}?version=1&todo_count=0"),
                None,
                true,
            ),
            (
                "POST",
                format!("/api/v1/lists/{unknown}/todos"),
                Some(json!({ "key": uuid::Uuid::now_v7(), "title": "x" })),
                true,
            ),
            (
                "PATCH",
                format!("/api/v1/lists/{unknown}/todos/{}", f.unknown_todo),
                Some(json!({ "version": 1, "done": true })),
                true,
            ),
            (
                "PATCH",
                format!("/api/v1/lists/{unknown}/todos/{}", f.unknown_todo),
                Some(json!({ "version": 1, "title": "x" })),
                true,
            ),
            (
                "POST",
                format!("/api/v1/lists/{unknown}/todos/{}/move", f.unknown_todo),
                Some(json!({ "above": [], "below": [] })),
                true,
            ),
            (
                "DELETE",
                format!("/api/v1/lists/{unknown}/todos/{}?version=1", f.unknown_todo),
                None,
                true,
            ),
        ];
        for ((method, uri, body, csrf), (umethod, uuri, ubody, ucsrf)) in
            paths.iter().zip(unknown_paths.iter())
        {
            let foreign_response = response(
                &f.app,
                request(method, uri, Some(&f.member), body.as_ref(), *csrf),
            )
            .await;
            let unknown_response = response(
                &f.app,
                request(umethod, uuri, Some(&f.member), ubody.as_ref(), *ucsrf),
            )
            .await;
            assert_eq!(foreign_response, unknown_response, "{method} {uri}");
            assert_eq!(foreign_response.0, StatusCode::NOT_FOUND);
            assert_eq!(foreign_response.1["code"], "list_not_found");
        }
    }

    // A todo in another Member's list, requested through the caller's own list,
    // is the same as an unknown todo ID — for a mark, a rename, a move and a delete.
    for (method, suffix, body) in [
        (
            "PATCH",
            String::new(),
            Some(json!({ "version": 1, "done": true })),
        ),
        (
            "PATCH",
            String::new(),
            Some(json!({ "version": 1, "title": "x" })),
        ),
        (
            "POST",
            "/move".to_string(),
            Some(json!({ "above": [], "below": [] })),
        ),
        ("DELETE", "?version=1".to_string(), None),
    ] {
        let foreign_todo = response(
            &f.app,
            request(
                method,
                &format!("/api/v1/lists/{own}/todos/{}{suffix}", f.other_todo),
                Some(&f.member),
                body.as_ref(),
                true,
            ),
        )
        .await;
        let unknown_todo = response(
            &f.app,
            request(
                method,
                &format!("/api/v1/lists/{own}/todos/{}{suffix}", f.unknown_todo),
                Some(&f.member),
                body.as_ref(),
                true,
            ),
        )
        .await;
        assert_eq!(foreign_todo, unknown_todo, "{method} {suffix}");
        assert_eq!(foreign_todo.0, StatusCode::NOT_FOUND);
        assert_eq!(foreign_todo.1["code"], "todo_not_found");
    }

    assert_other_untouched(&f).await;

    // The caller's own todo still works, and the list picker never names the
    // other Member's list.
    let ok = response(
        &f.app,
        request(
            "PATCH",
            &format!("/api/v1/lists/{own}/todos/{}", f.own_todo),
            Some(&f.member),
            Some(&json!({ "version": 1, "done": true })),
            true,
        ),
    )
    .await;
    assert_eq!(ok.0, StatusCode::OK);
    let lists = response(
        &f.app,
        request("GET", "/api/v1/lists", Some(&f.member), None, false),
    )
    .await;
    assert_eq!(lists.0, StatusCode::OK);
    assert_eq!(lists.1.as_array().unwrap().len(), 1);
    assert_eq!(lists.1[0]["id"], own.to_string());
}

#[sqlx::test(migrations = "../../migrations")]
async fn staff_get_the_same_404_and_never_see_another_accounts_lists(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let admin_id = insert_verified_user(&pool, "e2e-p35-admin@example.com", UserRole::Admin).await;
    let super_id =
        insert_verified_user(&pool, "e2e-p35-super@example.com", UserRole::SuperAdmin).await;

    for (role, user_id) in [("admin", admin_id), ("super_admin", super_id)] {
        let session = Session::for_user(user_id, true, role);
        let foreign = response(
            &f.app,
            request(
                "GET",
                &format!("/api/v1/lists/{}", f.other_list),
                Some(&session),
                None,
                false,
            ),
        )
        .await;
        let unknown = response(
            &f.app,
            request(
                "GET",
                &format!("/api/v1/lists/{}", f.unknown_list),
                Some(&session),
                None,
                false,
            ),
        )
        .await;
        assert_eq!(foreign, unknown, "{role}");
        assert_eq!(foreign.0, StatusCode::NOT_FOUND);
        assert_eq!(foreign.1["code"], "list_not_found");

        for (method, path, body) in [
            ("PATCH", String::new(), json!({ "version": 1, "name": "x" })),
            ("DELETE", "?version=1&todo_count=0".to_string(), Value::Null),
            (
                "POST",
                "/todos".to_string(),
                json!({ "key": uuid::Uuid::now_v7(), "title": "x" }),
            ),
            (
                "PATCH",
                format!("/todos/{}", f.other_todo),
                json!({ "version": 1, "done": true }),
            ),
            (
                "PATCH",
                format!("/todos/{}", f.other_todo),
                json!({ "version": 1, "title": "x" }),
            ),
            (
                "POST",
                format!("/todos/{}/move", f.other_todo),
                json!({ "above": [], "below": [] }),
            ),
            (
                "DELETE",
                format!("/todos/{}?version=1", f.other_todo),
                Value::Null,
            ),
        ] {
            let body = (!body.is_null()).then_some(&body);
            let mut answers = Vec::new();
            for list in [&f.other_list, &f.unknown_list] {
                let uri = format!("/api/v1/lists/{list}{path}");
                answers.push(
                    response(&f.app, request(method, &uri, Some(&session), body, true)).await,
                );
            }
            assert_eq!(answers[0], answers[1], "{role} {method}");
            assert_eq!(answers[0].0, StatusCode::NOT_FOUND, "{role} {method}");
            assert_eq!(answers[0].1["code"], "list_not_found", "{role} {method}");
        }
        assert_other_untouched(&f).await;

        let lists = response(
            &f.app,
            request("GET", "/api/v1/lists", Some(&session), None, false),
        )
        .await;
        assert_eq!(lists.0, StatusCode::OK);
        assert!(
            lists.1.as_array().unwrap().is_empty(),
            "{role} has no lists"
        );
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn visitors_and_unverified_members_are_refused(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let unverified_id =
        insert_verified_user(&pool, "e2e-p35-unverified@example.com", UserRole::User).await;
    let unverified = Session::for_user(unverified_id, false, "user");

    let paths = [
        ("GET", "/api/v1/lists".to_string(), None, false),
        (
            "POST",
            "/api/v1/lists".to_string(),
            Some(json!({ "key": uuid::Uuid::now_v7(), "name": "x" })),
            true,
        ),
        (
            "GET",
            format!("/api/v1/lists/{}", f.other_list),
            None,
            false,
        ),
        (
            "PATCH",
            format!("/api/v1/lists/{}", f.other_list),
            Some(json!({ "version": 1, "name": "x" })),
            true,
        ),
        (
            "DELETE",
            format!("/api/v1/lists/{}?version=1&todo_count=0", f.other_list),
            None,
            true,
        ),
        (
            "POST",
            format!("/api/v1/lists/{}/todos", f.other_list),
            Some(json!({ "key": uuid::Uuid::now_v7(), "title": "x" })),
            true,
        ),
        (
            "PATCH",
            format!("/api/v1/lists/{}/todos/{}", f.other_list, f.other_todo),
            Some(json!({ "version": 1, "done": true })),
            true,
        ),
        (
            "PATCH",
            format!("/api/v1/lists/{}/todos/{}", f.other_list, f.other_todo),
            Some(json!({ "version": 1, "title": "x" })),
            true,
        ),
        (
            "POST",
            format!("/api/v1/lists/{}/todos/{}/move", f.other_list, f.other_todo),
            Some(json!({ "above": [], "below": [] })),
            true,
        ),
        (
            "DELETE",
            format!(
                "/api/v1/lists/{}/todos/{}?version=1",
                f.other_list, f.other_todo
            ),
            None,
            true,
        ),
    ];

    for (method, uri, body, csrf) in &paths {
        let visitor = response(&f.app, request(method, uri, None, body.as_ref(), *csrf)).await;
        // A state-changing request without a session is refused by CSRF first;
        // either way the handler is unreachable.
        if *method == "GET" {
            assert_eq!(
                visitor.0,
                StatusCode::UNAUTHORIZED,
                "visitor {method} {uri}"
            );
        } else {
            assert!(
                matches!(visitor.0, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN),
                "visitor {method} {uri}: {}",
                visitor.0
            );
        }

        let unverified_response = response(
            &f.app,
            request(method, uri, Some(&unverified), body.as_ref(), *csrf),
        )
        .await;
        assert_eq!(
            unverified_response.0,
            StatusCode::FORBIDDEN,
            "unverified {method} {uri}"
        );
        assert_eq!(unverified_response.1["code"], "email_not_verified");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_missing_csrf_token_refuses_every_change(pool: PgPool) {
    let f = fixture(pool).await;
    let changes = [
        (
            "POST",
            "/api/v1/lists".to_string(),
            Some(json!({ "key": uuid::Uuid::now_v7(), "name": "x" })),
        ),
        (
            "POST",
            format!("/api/v1/lists/{}/todos", f.own_list),
            Some(json!({ "key": uuid::Uuid::now_v7(), "title": "x" })),
        ),
        (
            "PATCH",
            format!("/api/v1/lists/{}/todos/{}", f.own_list, f.own_todo),
            Some(json!({ "version": 1, "done": true })),
        ),
        (
            "PATCH",
            format!("/api/v1/lists/{}/todos/{}", f.own_list, f.own_todo),
            Some(json!({ "version": 1, "title": "x" })),
        ),
        (
            "PATCH",
            format!("/api/v1/lists/{}", f.own_list),
            Some(json!({ "version": 1, "name": "x" })),
        ),
        (
            "DELETE",
            format!("/api/v1/lists/{}?version=1&todo_count=1", f.own_list),
            None,
        ),
        (
            "POST",
            format!("/api/v1/lists/{}/todos/{}/move", f.own_list, f.own_todo),
            Some(json!({ "above": [], "below": [] })),
        ),
        (
            "DELETE",
            format!(
                "/api/v1/lists/{}/todos/{}?version=1",
                f.own_list, f.own_todo
            ),
            None,
        ),
    ];
    for (method, uri, body) in changes {
        let refused = response(
            &f.app,
            request(method, &uri, Some(&f.member), body.as_ref(), false),
        )
        .await;
        assert_eq!(refused.0, StatusCode::FORBIDDEN, "{method} {uri}");
        assert_eq!(refused.1["code"], "csrf_invalid");
    }
}

/// Spec: Positioning next to another Member's todo. A foreign neighbour gets
/// the answer an unknown one gets, wherever it is named, and is untouched.
#[sqlx::test(migrations = "../../migrations")]
async fn a_foreign_neighbour_answers_exactly_like_an_unknown_one(pool: PgPool) {
    let f = fixture(pool.clone()).await;
    let second = insert_todo(&pool, f.own_list, "My second", Some(2), None).await;
    let third = insert_todo(&pool, f.own_list, "My third", Some(3), None).await;
    let uri = format!("/api/v1/lists/{}/todos/{second}/move", f.own_list);
    let reset = json!({ "above": [f.own_todo], "below": [third] });

    let body = |neighbour: &str, shape: usize| match shape {
        0 => json!({ "above": [neighbour], "below": [] }),
        1 => json!({ "above": [], "below": [neighbour] }),
        2 => json!({ "above": [neighbour], "below": [f.own_todo] }),
        3 => json!({ "above": [neighbour, third], "below": [] }),
        _ => json!({ "above": [neighbour], "below": [neighbour] }),
    };
    for shape in 0..5 {
        let mut answers = Vec::new();
        for neighbour in [&f.other_todo, &f.unknown_todo] {
            let put_back = request("POST", &uri, Some(&f.member), Some(&reset), true);
            assert_eq!(response(&f.app, put_back).await.0, StatusCode::OK);
            let body = body(neighbour, shape);
            let request = request("POST", &uri, Some(&f.member), Some(&body), true);
            answers.push(response(&f.app, request).await);
        }
        assert_eq!(answers[0], answers[1], "shape {shape}");
        assert_eq!(answers[0].0, StatusCode::OK, "shape {shape}");
        assert_eq!(
            answers[0].1["open_todo_ids"].as_array().unwrap().len(),
            3,
            "only the caller's todos"
        );
    }
    assert_other_untouched(&f).await;
}
