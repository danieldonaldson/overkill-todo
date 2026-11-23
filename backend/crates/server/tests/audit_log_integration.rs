//! HTTP integration tests for the admin and self-service audit-log endpoints
//! (`/api/v1/admin/audit-events` and `/api/v1/users/me/security-activity`).

mod common;

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use persistence_postgres::testing::{self, audit_event_kinds, audit_event_rows_json};
use persistence_postgres::{
    Database, PostgresAuditLogRepository, PostgresOAuthAccountRepository, PostgresUserRepository,
};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use application::ports::{AuditLogRepository, OAuthAccountRepository, UserRepository};
use application::use_cases::PurgeScheduledAccountsUseCase;
use domain::entities::{AuditEvent, OAuthAccount, OAuthProvider, User};
use domain::value_objects::{UserId, UserRole};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn db(pool: &PgPool) -> Arc<Database> {
    Arc::new(Database::from_pool(pool.clone()))
}

fn user_repo(pool: &PgPool) -> PostgresUserRepository {
    PostgresUserRepository::new(db(pool))
}

/// Inserts a verified account with `role` and loads it through the repository.
async fn seed_with_role(pool: &PgPool, role: UserRole) -> User {
    let email = format!("audit-{}@example.com", Uuid::now_v7());
    let id = testing::insert_verified_user(pool, &email, role).await;
    user_repo(pool)
        .find_by_id(&UserId::from_uuid(id))
        .await
        .unwrap()
        .unwrap()
}

// ---------------------------------------------------------------------------
// GET /api/v1/admin/audit-events
// ---------------------------------------------------------------------------

/// A request as a persisted account with `role`, or as a Visitor for `None`.
async fn get_as(pool: &PgPool, role: Option<UserRole>, path: &str) -> (StatusCode, Value) {
    let mut req = Request::builder().method("GET").uri(path);
    if let Some(role) = role {
        let requester = seed_with_role(pool, role).await;
        let token = common::mint_session_jwt(*requester.id().as_uuid(), true, role.to_db_str(), 15);
        req = req.header(header::COOKIE, format!("__Host-auth_token={token}"));
    }
    let resp = common::create_test_app(pool.clone())
        .oneshot(req.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Super Admin `actor` promotes `member` through the API, as the app does.
async fn promote_via_api(pool: &PgPool, actor: &User, member: &User) {
    let actor_id = actor.id().as_uuid().to_string();
    let token = common::mint_session_jwt(*actor.id().as_uuid(), true, "super_admin", 15);
    let csrf = common::mint_csrf_token(&actor_id);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/api/v1/admin/users/{}/role", member.id()))
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::COOKIE,
            format!("__Host-auth_token={token}; __Host-csrf_token={csrf}"),
        )
        .header("x-csrf-token", csrf)
        .body(Body::from(json!({ "role": "admin" }).to_string()))
        .unwrap();
    let resp = common::create_test_app(pool.clone())
        .oneshot(req)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[sqlx::test(migrations = "../../migrations")]
async fn super_admin_reads_a_role_change_in_the_audit_log(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    promote_via_api(&pool, &actor, &member).await;

    let (status, body) = get_as(
        &pool,
        Some(UserRole::SuperAdmin),
        "/api/v1/admin/audit-events",
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["total"], 1);
    assert_eq!(body["limit"], 50);
    let event = &body["events"][0];
    assert_eq!(event["kind"], "role_changed");
    assert_eq!(event["subject"]["id"], member.id().to_string());
    assert_eq!(event["subject"]["email"], member.email().as_str());
    assert_eq!(event["subject"]["deleted"], false);
    assert_eq!(event["actor"]["kind"], "user");
    assert_eq!(event["actor"]["account"]["id"], actor.id().to_string());
    assert_eq!(
        event["details"],
        json!({ "from_role": "user", "to_role": "admin" })
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn audit_log_is_refused_to_everyone_but_super_admins(pool: PgPool) {
    for (role, expected) in [
        (Some(UserRole::Admin), StatusCode::FORBIDDEN),
        (Some(UserRole::User), StatusCode::FORBIDDEN),
        (None, StatusCode::UNAUTHORIZED),
    ] {
        let (status, body) = get_as(&pool, role, "/api/v1/admin/audit-events").await;
        assert_eq!(status, expected, "{role:?}: {body}");
        assert!(body.get("events").is_none());
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn audit_log_filters_by_affected_or_acting_account(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let other_actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    let bystander = seed_with_role(&pool, UserRole::User).await;
    promote_via_api(&pool, &actor, &member).await;
    promote_via_api(&pool, &other_actor, &bystander).await;

    let path = |id: &UserId| format!("/api/v1/admin/audit-events?account_id={id}");
    // As the affected account.
    let (_, body) = get_as(&pool, Some(UserRole::SuperAdmin), &path(member.id())).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["events"][0]["subject"]["id"], member.id().to_string());
    // As the acting account.
    let (_, body) = get_as(&pool, Some(UserRole::SuperAdmin), &path(other_actor.id())).await;
    assert_eq!(body["total"], 1);
    assert_eq!(
        body["events"][0]["subject"]["id"],
        bystander.id().to_string()
    );
    // By kind, newest first.
    let (_, body) = get_as(
        &pool,
        Some(UserRole::SuperAdmin),
        "/api/v1/admin/audit-events?kind=role_changed",
    )
    .await;
    assert_eq!(body["total"], 2);
    assert_eq!(
        body["events"][0]["subject"]["id"],
        bystander.id().to_string()
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn audit_log_with_no_matching_events_is_empty_not_an_error(pool: PgPool) {
    let (status, body) = get_as(
        &pool,
        Some(UserRole::SuperAdmin),
        &format!("/api/v1/admin/audit-events?account_id={}", Uuid::now_v7()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total"], 0);
    assert_eq!(body["events"], json!([]));
}

#[sqlx::test(migrations = "../../migrations")]
async fn audit_log_limit_is_capped_and_bad_filters_are_400(pool: PgPool) {
    let (status, body) = get_as(
        &pool,
        Some(UserRole::SuperAdmin),
        "/api/v1/admin/audit-events?limit=1000",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["limit"], 100);

    for query in ["kind=password_stolen", "account_id=not-a-uuid"] {
        let (status, _) = get_as(
            &pool,
            Some(UserRole::SuperAdmin),
            &format!("/api/v1/admin/audit-events?{query}"),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{query}");
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn deleted_account_shows_without_its_former_email(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    promote_via_api(&pool, &actor, &member).await;
    testing::mark_user_deleted_now(&pool, *member.id().as_uuid()).await;

    let (_, body) = get_as(
        &pool,
        Some(UserRole::SuperAdmin),
        "/api/v1/admin/audit-events",
    )
    .await;

    let subject = &body["events"][0]["subject"];
    assert_eq!(subject["id"], member.id().to_string());
    assert_eq!(subject["email"], Value::Null);
    assert_eq!(subject["deleted"], true);
}

// ---------------------------------------------------------------------------
// Minimal personal data (task 2.10)
// ---------------------------------------------------------------------------

async fn send_json(app: &axum::Router, req: Request<Body>) -> (StatusCode, Option<String>, Value) {
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let auth = common::cookie_value(&resp, "__Host-auth_token");
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        auth,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn json_request(
    method: &str,
    uri: &str,
    body: &Value,
    session: Option<(&str, &str)>,
) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .header("x-device-fingerprint", "test-device-abc123");
    if let Some((token, csrf)) = session {
        req = req
            .header(
                header::COOKIE,
                format!("__Host-auth_token={token}; __Host-csrf_token={csrf}"),
            )
            .header("x-csrf-token", csrf);
    }
    req.body(Body::from(body.to_string())).unwrap()
}

/// A Member signs up, links Google, changes their password, exports their
/// data and deletes their account. None of the events this leaves holds their
/// email, name, password or hash, any code or token, or a client address.
#[sqlx::test(migrations = "../../migrations")]
async fn events_after_a_full_account_lifecycle_hold_no_personal_data(pool: PgPool) {
    let app = common::create_test_app(pool.clone());
    let email = "e2e-lifecycle-pii@example.com";
    let (first, second) = ("hunter2hunter2", "brandnewpass99");
    let google_sub = "google-sub-8675309";
    let google_email = "lifecycle.google@gmail.example";

    // Sign up (the test app's verification code is fixed).
    send_json(
        &app,
        json_request(
            "POST",
            "/api/v1/auth/register",
            &json!({ "email": email }),
            None,
        ),
    )
    .await;
    let (_, _, verified) = send_json(
        &app,
        json_request(
            "POST",
            "/api/v1/auth/verify-email",
            &json!({ "email": email, "code": "123456" }),
            None,
        ),
    )
    .await;
    let signup_token = verified["signup_token"]
        .as_str()
        .unwrap_or_else(|| panic!("no signup token: {verified}"))
        .to_string();
    let (status, token, body) = send_json(
        &app,
        json_request(
            "POST",
            "/api/v1/auth/complete-signup",
            &json!({ "signup_token": signup_token, "email": email, "password": first }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = token.unwrap();
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    let user = user_repo(&pool)
        .find_by_email(email)
        .await
        .unwrap()
        .unwrap();
    let id = *user.id();

    // Link Google (the provider exchange itself is out of scope here).
    PostgresOAuthAccountRepository::new(db(&pool))
        .save(
            &OAuthAccount::new(
                id,
                OAuthProvider::Google,
                google_sub.into(),
                Some(google_email.into()),
            ),
            &AuditEvent::sign_in_method_linked(id, OAuthProvider::Google),
        )
        .await
        .unwrap();

    // Change the password.
    let (status, _, body) = send_json(
        &app,
        json_request(
            "POST",
            "/api/v1/users/password/change",
            &json!({ "current_password": first, "new_password": second }),
            Some((&token, &csrf)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let hash = testing::user_snapshot(&pool, *id.as_uuid())
        .await
        .password_hash
        .unwrap();

    // Sign in again, export, then ask for deletion.
    let (status, token, body) = send_json(
        &app,
        json_request(
            "POST",
            "/api/v1/auth/login",
            &json!({ "email": email, "password": second }),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let token = token.unwrap();
    let csrf = body["csrf_token"].as_str().unwrap().to_string();
    let export = Request::builder()
        .uri("/api/v1/users/me/export")
        .header(header::COOKIE, format!("__Host-auth_token={token}"))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send_json(&app, export).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let (status, _, body) = send_json(
        &app,
        json_request(
            "DELETE",
            "/api/v1/users/me",
            &json!({ "password": second, "confirmation": "DELETE" }),
            Some((&token, &csrf)),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    // The grace period ends and the purge deletes the account.
    testing::backdate_deletion_scheduled(&pool, *id.as_uuid(), 1).await;
    let purged = PurgeScheduledAccountsUseCase::new(Arc::new(user_repo(&pool)))
        .run_once()
        .await
        .unwrap();
    assert_eq!(purged, 1);

    assert_eq!(
        audit_event_kinds(&pool).await,
        vec![
            "sign_in_method_linked",
            "password_changed",
            "data_exported",
            "deletion_requested",
            "account_deleted",
        ]
    );
    let rows = audit_event_rows_json(&pool).await;
    let mut forbidden = vec![
        email,
        "lifecycle-pii",
        first,
        second,
        &hash,
        "123456",
        &signup_token,
        &token,
        &csrf,
        google_sub,
        google_email,
        "127.0.0.1",
        "test-device-abc123",
    ];
    for name in [user.first_name(), user.last_name()] {
        if !name.is_empty() {
            forbidden.push(name);
        }
    }
    for row in &rows {
        for secret in &forbidden {
            assert!(
                !row.contains(secret),
                "an audit event holds {secret:?}: {row}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// GET /api/v1/users/me/security-activity (slice 3)
// ---------------------------------------------------------------------------

/// The signed-in `member`'s Security activity at `query`.
async fn activity_of(pool: &PgPool, member: &User, query: &str) -> (StatusCode, Value) {
    // A session issued now, with the account's current role, as after
    // signing in again.
    let role = testing::backdate_tokens_valid_after_and_role(pool, *member.id().as_uuid()).await;
    let token = common::mint_session_jwt(*member.id().as_uuid(), true, &role, 15);
    let req = Request::builder()
        .uri(format!("/api/v1/users/me/security-activity{query}"))
        .header(header::COOKIE, format!("__Host-auth_token={token}"))
        .body(Body::empty())
        .unwrap();
    let resp = common::create_test_app(pool.clone())
        .oneshot(req)
        .await
        .unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[sqlx::test(migrations = "../../migrations")]
async fn member_sees_a_staff_role_change_without_the_staff_account(pool: PgPool) {
    let staff = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    promote_via_api(&pool, &staff, &member).await;
    let id = *member.id();
    user_repo(&pool)
        .update_password(&id, "h", &AuditEvent::password_reset(id))
        .await
        .unwrap();

    let (status, body) = activity_of(&pool, &member, "").await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["events"][0]["kind"], "password_reset");
    assert_eq!(body["events"][0]["actor"], "you");
    assert_eq!(body["events"][1]["kind"], "role_changed");
    assert_eq!(body["events"][1]["actor"], "administrator");
    assert_eq!(
        body["events"][1]["details"],
        json!({ "from_role": "user", "to_role": "admin" })
    );
    assert!(body.get("next_before_id").is_none(), "{body}");
    let text = body.to_string();
    assert!(!text.contains(&staff.id().to_string()), "{text}");
    assert!(!text.contains(staff.email().as_str()), "{text}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn security_activity_holds_only_the_callers_events(pool: PgPool) {
    let member = seed_with_role(&pool, UserRole::User).await;
    let other = seed_with_role(&pool, UserRole::User).await;
    let audit = PostgresAuditLogRepository::new(db(&pool));
    audit
        .record(&AuditEvent::data_exported(*other.id()))
        .await
        .unwrap();
    audit
        .record(&AuditEvent::sessions_revoked_after_reuse(*member.id()))
        .await
        .unwrap();

    let (_, body) = activity_of(&pool, &member, "").await;
    assert_eq!(body["events"].as_array().unwrap().len(), 1);
    assert_eq!(body["events"][0]["kind"], "sessions_revoked_after_reuse");
    assert_eq!(body["events"][0]["actor"], "system");

    // No parameter selects another account: unknown ones are ignored.
    let (_, body) = activity_of(&pool, &member, &format!("?user_id={}", other.id())).await;
    assert_eq!(body["events"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = "../../migrations")]
async fn security_activity_pages_with_before_id(pool: PgPool) {
    let member = seed_with_role(&pool, UserRole::User).await;
    let audit = PostgresAuditLogRepository::new(db(&pool));
    for _ in 0..5 {
        audit
            .record(&AuditEvent::data_exported(*member.id()))
            .await
            .unwrap();
    }

    let (_, first) = activity_of(&pool, &member, "?limit=2").await;
    let ids = |page: &Value| -> Vec<i64> {
        page["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_i64().unwrap())
            .collect()
    };
    assert_eq!(ids(&first).len(), 2);
    let next = first["next_before_id"].as_i64().unwrap();
    assert_eq!(next, ids(&first)[1]);

    let (_, second) = activity_of(&pool, &member, &format!("?limit=2&before_id={next}")).await;
    let (_, third) = activity_of(
        &pool,
        &member,
        &format!("?limit=2&before_id={}", second["next_before_id"]),
    )
    .await;
    let mut all = [ids(&first), ids(&second), ids(&third)].concat();
    assert_eq!(all.len(), 5);
    assert!(third.get("next_before_id").is_none());
    let sorted = {
        all.sort_by(|a, b| b.cmp(a));
        all.clone()
    };
    assert_eq!(all, sorted);
}

#[sqlx::test(migrations = "../../migrations")]
async fn security_activity_is_empty_for_a_new_account_and_refused_to_visitors(pool: PgPool) {
    let member = seed_with_role(&pool, UserRole::User).await;
    let (status, body) = activity_of(&pool, &member, "").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "events": [] }));

    let (status, _) = get_as(&pool, None, "/api/v1/users/me/security-activity").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
