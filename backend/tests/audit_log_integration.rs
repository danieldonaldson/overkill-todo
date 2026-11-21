//! Integration tests for the audit log (OT-008): the append-only guard on
//! `audit_events` and the atomicity of each audited action with its event.

mod common;

use std::sync::Arc;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

use overkill_todo::application::ports::UserRepository;
use overkill_todo::domain::entities::{Actor, AuditEvent, AuditKind, User};
use overkill_todo::domain::errors::DomainError;
use overkill_todo::domain::value_objects::{UserId, UserRole};
use overkill_todo::infrastructure::persistence::postgres::{Database, PostgresUserRepository};

// ---------------------------------------------------------------------------
// Coverage guard
// ---------------------------------------------------------------------------

/// The `#[sqlx::test]`s proving each kind is stored if and only if its action
/// took effect. No wildcard arm: a new [`AuditKind`] doesn't compile until it
/// names its tests here, and a name that isn't a test doesn't compile either.
fn atomicity_tests(kind: AuditKind) -> &'static [fn()] {
    match kind {
        AuditKind::RoleChanged => &[
            role_change_writes_exactly_one_event,
            refused_last_super_admin_demotion_leaves_no_event,
            role_change_version_conflict_leaves_no_event,
            failed_event_insert_rolls_back_the_role_change,
            concurrent_role_changes_give_one_event_per_applied_change,
        ],
    }
}

#[test]
fn every_audit_kind_has_atomicity_tests() {
    for kind in AuditKind::all() {
        assert!(
            !atomicity_tests(*kind).is_empty(),
            "{} has no atomicity test",
            kind.as_str()
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn user_repo(pool: &PgPool) -> PostgresUserRepository {
    PostgresUserRepository::new(Arc::new(Database::from_pool(pool.clone())))
}

/// Inserts a verified account with `role` and loads it through the repository.
async fn seed_with_role(pool: &PgPool, role: UserRole) -> User {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO users (id, email, role, email_verified)
         VALUES ($1, $2, $3::user_role, true)",
    )
    .bind(id)
    .bind(format!("audit-{id}@example.com"))
    .bind(role.to_db_str())
    .execute(pool)
    .await
    .unwrap();
    user_repo(pool)
        .find_by_id(&UserId::from_uuid(id))
        .await
        .unwrap()
        .unwrap()
}

/// Changes `user`'s role to `to` as `actor`, the way the use case does.
async fn change_role(
    pool: &PgPool,
    mut user: User,
    actor: UserId,
    to: UserRole,
) -> Result<(), DomainError> {
    let from = user.role();
    let event = AuditEvent::role_changed(*user.id(), Actor::User(actor), from, to);
    user.set_role(to);
    user_repo(pool)
        .save_role_change(&user, UserRole::removes_super_admin(from, to), &event)
        .await
}

async fn stored_role(pool: &PgPool, id: &UserId) -> String {
    sqlx::query_scalar("SELECT role::text FROM users WHERE id = $1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Every stored event as (kind, subject, actor kind, actor, details), in order.
async fn stored_events(pool: &PgPool) -> Vec<(String, Uuid, String, Option<Uuid>, Value)> {
    sqlx::query_as(
        "SELECT kind, subject_id, actor_kind, actor_id, details FROM audit_events ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

/// Inserts a role-change event with the given age in days and returns its ID.
async fn insert_event_aged(pool: &PgPool, age_days: i32) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO audit_events (occurred_at, kind, subject_id, actor_kind, details)
         VALUES (now() - make_interval(days => $1), 'role_changed', $2, 'operator',
                 '{\"from\": \"user\", \"to\": \"super_admin\"}')
         RETURNING id",
    )
    .bind(age_days)
    .bind(Uuid::now_v7())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn event_count(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM audit_events")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Days in 7 years plus one, counting the leap days such a span always holds.
const OVER_SEVEN_YEARS_DAYS: i32 = 7 * 365 + 2 + 1;

#[sqlx::test(migrations = "./migrations")]
async fn audit_event_update_is_refused(pool: PgPool) {
    let id = insert_event_aged(&pool, 0).await;
    let err =
        sqlx::query("UPDATE audit_events SET kind = 'role_changed', trace_id = 'x' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap_err();
    assert!(err.to_string().contains("append-only"), "{err}");
    let trace: Option<String> =
        sqlx::query_scalar("SELECT trace_id FROM audit_events WHERE id = $1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(trace, None);
}

#[sqlx::test(migrations = "./migrations")]
async fn audit_event_delete_of_a_fresh_row_is_refused(pool: PgPool) {
    let id = insert_event_aged(&pool, 0).await;
    // Just under 7 years old is still protected.
    insert_event_aged(&pool, 7 * 365).await;
    for sql in [
        "DELETE FROM audit_events WHERE id = $1",
        "DELETE FROM audit_events WHERE TRUE OR id = $1",
    ] {
        let err = sqlx::query(sql).bind(id).execute(&pool).await.unwrap_err();
        assert!(err.to_string().contains("append-only"), "{err}");
    }
    assert_eq!(event_count(&pool).await, 2);
}

#[sqlx::test(migrations = "./migrations")]
async fn audit_event_delete_past_seven_years_is_allowed(pool: PgPool) {
    let old = insert_event_aged(&pool, OVER_SEVEN_YEARS_DAYS).await;
    let fresh = insert_event_aged(&pool, 0).await;
    let deleted = sqlx::query("DELETE FROM audit_events WHERE id = $1")
        .bind(old)
        .execute(&pool)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(deleted, 1);
    let left: Vec<i64> = sqlx::query_scalar("SELECT id FROM audit_events")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(left, vec![fresh]);
}

#[sqlx::test(migrations = "./migrations")]
async fn audit_event_truncate_is_refused(pool: PgPool) {
    insert_event_aged(&pool, OVER_SEVEN_YEARS_DAYS).await;
    let err = sqlx::query("TRUNCATE audit_events")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("append-only"), "{err}");
    assert_eq!(event_count(&pool).await, 1);
}

#[sqlx::test(migrations = "./migrations")]
async fn audit_event_actor_id_is_set_only_for_user_actors(pool: PgPool) {
    for (actor_kind, actor_id) in [("user", None), ("operator", Some(Uuid::now_v7()))] {
        let result = sqlx::query(
            "INSERT INTO audit_events (kind, subject_id, actor_kind, actor_id)
             VALUES ('role_changed', $1, $2, $3)",
        )
        .bind(Uuid::now_v7())
        .bind(actor_kind)
        .bind(actor_id)
        .execute(&pool)
        .await;
        assert!(
            result.is_err(),
            "{actor_kind} with {actor_id:?} was accepted"
        );
    }
}

// ---------------------------------------------------------------------------
// Role changes: event if and only if the change took effect
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "./migrations")]
async fn role_change_writes_exactly_one_event(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    let member_id = *member.id();

    change_role(&pool, member, *actor.id(), UserRole::Admin)
        .await
        .unwrap();

    assert_eq!(stored_role(&pool, &member_id).await, "admin");
    assert_eq!(
        stored_events(&pool).await,
        vec![(
            "role_changed".into(),
            *member_id.as_uuid(),
            "user".into(),
            Some(*actor.id().as_uuid()),
            json!({ "from": "user", "to": "admin" }),
        )]
    );
}

#[sqlx::test(migrations = "./migrations")]
async fn refused_last_super_admin_demotion_leaves_no_event(pool: PgPool) {
    let only = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let only_id = *only.id();
    let actor = UserId::new();

    let err = change_role(&pool, only, actor, UserRole::User)
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Forbidden(_)), "{err:?}");
    assert_eq!(stored_role(&pool, &only_id).await, "super_admin");
    assert_eq!(stored_events(&pool).await, Vec::new());
}

#[sqlx::test(migrations = "./migrations")]
async fn role_change_version_conflict_leaves_no_event(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    let member_id = *member.id();
    let stale = member.clone();

    change_role(&pool, member, *actor.id(), UserRole::Admin)
        .await
        .unwrap();
    let err = change_role(&pool, stale, *actor.id(), UserRole::SuperAdmin)
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Conflict(_)), "{err:?}");
    assert_eq!(stored_role(&pool, &member_id).await, "admin");
    let events = stored_events(&pool).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].4, json!({ "from": "user", "to": "admin" }));
}

#[sqlx::test(migrations = "./migrations")]
async fn failed_event_insert_rolls_back_the_role_change(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    let member_id = *member.id();
    // Make every role-change event fail to insert, as if the table refused it.
    sqlx::query(
        "ALTER TABLE audit_events ADD CONSTRAINT test_refuse_role_changed
         CHECK (kind <> 'role_changed') NOT VALID",
    )
    .execute(&pool)
    .await
    .unwrap();

    let err = change_role(&pool, member, *actor.id(), UserRole::Admin)
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    assert_eq!(stored_role(&pool, &member_id).await, "user");
    assert_eq!(stored_events(&pool).await, Vec::new());
}

#[sqlx::test(migrations = "./migrations")]
async fn concurrent_role_changes_give_one_event_per_applied_change(pool: PgPool) {
    let first = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let second = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    let member_id = *member.id();

    // Two Super Admins change the same account at once, from the same loaded
    // version: exactly one change applies, with exactly one event.
    let (a, b) = tokio::join!(
        change_role(&pool, member.clone(), *first.id(), UserRole::Admin),
        change_role(&pool, member, *second.id(), UserRole::SuperAdmin),
    );
    assert_eq!(
        usize::from(a.is_ok()) + usize::from(b.is_ok()),
        1,
        "{a:?} {b:?}"
    );
    let winner_role = stored_role(&pool, &member_id).await;
    let events = stored_events(&pool).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].4["to"], json!(winner_role));

    // Changes applied one after another are recorded in that order.
    let reloaded = user_repo(&pool)
        .find_by_id(&member_id)
        .await
        .unwrap()
        .unwrap();
    change_role(&pool, reloaded, *first.id(), UserRole::User)
        .await
        .unwrap();
    let reloaded = user_repo(&pool)
        .find_by_id(&member_id)
        .await
        .unwrap()
        .unwrap();
    change_role(&pool, reloaded, *second.id(), UserRole::Admin)
        .await
        .unwrap();
    let tos: Vec<Value> = stored_events(&pool)
        .await
        .into_iter()
        .map(|e| e.4["to"].clone())
        .collect();
    assert_eq!(tos, vec![json!(winner_role), json!("user"), json!("admin")]);
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
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

#[sqlx::test(migrations = "./migrations")]
async fn deleted_account_shows_without_its_former_email(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    promote_via_api(&pool, &actor, &member).await;
    sqlx::query("UPDATE users SET deleted_at = now() WHERE id = $1")
        .bind(member.id().as_uuid())
        .execute(&pool)
        .await
        .unwrap();

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
