//! Repository and schema integration tests for the audit log (OT-008): the
//! append-only guard on `audit_events` and the atomicity of each audited action
//! with its event.

use std::sync::Arc;

use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use application::ports::AuditLogRepository;
use application::ports::{
    AccountLifecycleRepository, OAuthAccountRepository, RefreshTokenRepository, UserRepository,
};
use domain::entities::{
    Actor, AuditEvent, AuditKind, OAuthAccount, OAuthProvider, RefreshToken, User,
};
use domain::errors::DomainError;
use domain::value_objects::{Email, UserId, UserRole};
use jiff::{SignedDuration, Timestamp};
use persistence_postgres::testing::{self, audit_event_kinds};
use persistence_postgres::{
    Database, PostgresAuditLogRepository, PostgresOAuthAccountRepository,
    PostgresRefreshTokenRepository, PostgresUserRepository,
};

// ---------------------------------------------------------------------------
// Coverage guard
// ---------------------------------------------------------------------------

/// The `#[sqlx::test(migrations = "../../migrations")]`s proving each kind is stored if and only if its action
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
        // Change and set both go through `save_audited`, as do deletion
        // requests and cancellations.
        AuditKind::PasswordChanged
        | AuditKind::PasswordSet
        | AuditKind::DeletionRequested
        | AuditKind::DeletionCancelled => &[
            save_audited_writes_the_event_with_the_save,
            save_audited_version_conflict_leaves_no_event,
            failed_event_insert_rolls_back_save_audited,
        ],
        AuditKind::PasswordReset => &[
            password_reset_writes_exactly_one_event,
            password_reset_of_a_missing_account_leaves_no_event,
            failed_event_insert_rolls_back_the_password_reset,
        ],
        AuditKind::SignInMethodLinked => &[
            link_writes_exactly_one_event,
            duplicate_link_leaves_no_event,
            failed_event_insert_rolls_back_the_link,
        ],
        AuditKind::SignInMethodUnlinked => &[
            unlink_writes_exactly_one_event,
            unlinking_nothing_leaves_no_event,
            failed_event_insert_rolls_back_the_unlink,
        ],
        AuditKind::AccountDeleted => &[
            purge_writes_one_event_and_keeps_earlier_ones,
            purge_of_a_cancelled_deletion_leaves_no_event,
            failed_event_insert_rolls_back_the_purge,
        ],
        AuditKind::DataExported => &[
            export_record_writes_exactly_one_event,
            failed_export_record_is_an_error,
        ],
        AuditKind::SessionsRevokedAfterReuse => &[
            reuse_revocation_writes_exactly_one_event,
            reuse_revocation_with_no_live_session_leaves_no_event,
            failed_event_insert_rolls_back_the_reuse_revocation,
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
    let email = format!("audit-{}@example.com", Uuid::now_v7());
    let id = testing::insert_verified_user(pool, &email, role).await;
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

#[sqlx::test(migrations = "../../migrations")]
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

#[sqlx::test(migrations = "../../migrations")]
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

#[sqlx::test(migrations = "../../migrations")]
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

#[sqlx::test(migrations = "../../migrations")]
async fn audit_event_truncate_is_refused(pool: PgPool) {
    insert_event_aged(&pool, OVER_SEVEN_YEARS_DAYS).await;
    let err = sqlx::query("TRUNCATE audit_events")
        .execute(&pool)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("append-only"), "{err}");
    assert_eq!(event_count(&pool).await, 1);
}

#[sqlx::test(migrations = "../../migrations")]
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

#[sqlx::test(migrations = "../../migrations")]
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

#[sqlx::test(migrations = "../../migrations")]
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

#[sqlx::test(migrations = "../../migrations")]
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

#[sqlx::test(migrations = "../../migrations")]
async fn failed_event_insert_rolls_back_the_role_change(pool: PgPool) {
    let actor = seed_with_role(&pool, UserRole::SuperAdmin).await;
    let member = seed_with_role(&pool, UserRole::User).await;
    let member_id = *member.id();
    // Make every role-change event fail to insert, as if the table refused it.
    testing::refuse_role_changed_audit_events(&pool).await;

    let err = change_role(&pool, member, *actor.id(), UserRole::Admin)
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    assert_eq!(stored_role(&pool, &member_id).await, "user");
    assert_eq!(stored_events(&pool).await, Vec::new());
}

#[sqlx::test(migrations = "../../migrations")]
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
// Slice 2: the other account actions
// ---------------------------------------------------------------------------

fn db(pool: &PgPool) -> Arc<Database> {
    Arc::new(Database::from_pool(pool.clone()))
}

/// Makes every event insert fail from here on, as if the table refused it.
async fn refuse_every_event(pool: &PgPool) {
    sqlx::query(
        "ALTER TABLE audit_events ADD CONSTRAINT test_refuse_everything CHECK (false) NOT VALID",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn stored_kinds(pool: &PgPool) -> Vec<String> {
    audit_event_kinds(pool).await
}

async fn stored_password_hash(pool: &PgPool, id: &UserId) -> Option<String> {
    sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn link_count(pool: &PgPool, id: &UserId) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM user_oauth_accounts WHERE user_id = $1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

fn google_link(id: UserId, sub: &str) -> OAuthAccount {
    OAuthAccount::new(id, OAuthProvider::Google, sub.into(), None)
}

// --- save_audited (password changed / set, deletion requested / cancelled)

#[sqlx::test(migrations = "../../migrations")]
async fn save_audited_writes_the_event_with_the_save(pool: PgPool) {
    let mut user = seed_with_role(&pool, UserRole::User).await;
    let id = *user.id();
    user.update_password("new-hash".into());

    user_repo(&pool)
        .save_audited(&user, &AuditEvent::password_changed(id))
        .await
        .unwrap();

    assert_eq!(
        stored_password_hash(&pool, &id).await.as_deref(),
        Some("new-hash")
    );
    assert_eq!(stored_kinds(&pool).await, vec!["password_changed"]);
    let (_, subject, actor_kind, actor, _) = stored_events(&pool).await.remove(0);
    assert_eq!(
        (subject, actor_kind.as_str(), actor),
        (*id.as_uuid(), "user", Some(*id.as_uuid()))
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn save_audited_version_conflict_leaves_no_event(pool: PgPool) {
    let user = seed_with_role(&pool, UserRole::User).await;
    let id = *user.id();
    let mut stale = user.clone();
    let mut fresh = user;
    fresh.schedule_deletion(Timestamp::now()).unwrap();
    user_repo(&pool).save(&fresh).await.unwrap();

    stale.update_password("new-hash".into());
    let err = user_repo(&pool)
        .save_audited(&stale, &AuditEvent::password_changed(id))
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Conflict(_)), "{err:?}");
    assert_eq!(stored_password_hash(&pool, &id).await, None);
    assert_eq!(stored_kinds(&pool).await, Vec::<String>::new());
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_event_insert_rolls_back_save_audited(pool: PgPool) {
    let mut user = seed_with_role(&pool, UserRole::User).await;
    let id = *user.id();
    refuse_every_event(&pool).await;
    user.schedule_deletion(Timestamp::now()).unwrap();

    let err = user_repo(&pool)
        .save_audited(&user, &AuditEvent::deletion_requested(id))
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    let stored = user_repo(&pool).find_by_id(&id).await.unwrap().unwrap();
    assert_eq!(stored.deletion_scheduled_at(), None);
    assert_eq!(stored.version(), user.version());
}

// --- password reset (update_password)

#[sqlx::test(migrations = "../../migrations")]
async fn password_reset_writes_exactly_one_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();

    user_repo(&pool)
        .update_password(&id, "reset-hash", &AuditEvent::password_reset(id))
        .await
        .unwrap();

    assert_eq!(
        stored_password_hash(&pool, &id).await.as_deref(),
        Some("reset-hash")
    );
    assert_eq!(stored_kinds(&pool).await, vec!["password_reset"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn password_reset_of_a_missing_account_leaves_no_event(pool: PgPool) {
    let ghost = UserId::new();
    let err = user_repo(&pool)
        .update_password(&ghost, "reset-hash", &AuditEvent::password_reset(ghost))
        .await
        .unwrap_err();
    assert!(matches!(err, DomainError::NotFound(_)), "{err:?}");
    assert_eq!(stored_kinds(&pool).await, Vec::<String>::new());
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_event_insert_rolls_back_the_password_reset(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    refuse_every_event(&pool).await;

    let err = user_repo(&pool)
        .update_password(&id, "reset-hash", &AuditEvent::password_reset(id))
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    assert_eq!(stored_password_hash(&pool, &id).await, None);
}

// --- sign-in methods

#[sqlx::test(migrations = "../../migrations")]
async fn link_writes_exactly_one_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    let links = PostgresOAuthAccountRepository::new(db(&pool));

    links
        .save(
            &google_link(id, "sub-1"),
            &AuditEvent::sign_in_method_linked(id, OAuthProvider::Google),
        )
        .await
        .unwrap();

    assert_eq!(link_count(&pool, &id).await, 1);
    assert_eq!(stored_kinds(&pool).await, vec!["sign_in_method_linked"]);
    assert_eq!(
        stored_events(&pool).await[0].4,
        json!({ "provider": "google" })
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn duplicate_link_leaves_no_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    let links = PostgresOAuthAccountRepository::new(db(&pool));
    let event = AuditEvent::sign_in_method_linked(id, OAuthProvider::Google);
    links.save(&google_link(id, "sub-1"), &event).await.unwrap();

    let err = links
        .save(&google_link(id, "sub-1"), &event)
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Conflict(_)), "{err:?}");
    assert_eq!(stored_kinds(&pool).await, vec!["sign_in_method_linked"]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_event_insert_rolls_back_the_link(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    refuse_every_event(&pool).await;

    let err = PostgresOAuthAccountRepository::new(db(&pool))
        .save(
            &google_link(id, "sub-1"),
            &AuditEvent::sign_in_method_linked(id, OAuthProvider::Google),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    assert_eq!(link_count(&pool, &id).await, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn unlink_writes_exactly_one_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    let links = PostgresOAuthAccountRepository::new(db(&pool));
    links
        .save(
            &google_link(id, "sub-1"),
            &AuditEvent::sign_in_method_linked(id, OAuthProvider::Google),
        )
        .await
        .unwrap();

    let deleted = links
        .delete_by_user_and_provider(
            &id,
            OAuthProvider::Google,
            &AuditEvent::sign_in_method_unlinked(id, OAuthProvider::Google),
        )
        .await
        .unwrap();

    assert!(deleted);
    assert_eq!(link_count(&pool, &id).await, 0);
    assert_eq!(
        stored_kinds(&pool).await,
        vec!["sign_in_method_linked", "sign_in_method_unlinked"]
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn unlinking_nothing_leaves_no_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    let deleted = PostgresOAuthAccountRepository::new(db(&pool))
        .delete_by_user_and_provider(
            &id,
            OAuthProvider::Google,
            &AuditEvent::sign_in_method_unlinked(id, OAuthProvider::Google),
        )
        .await
        .unwrap();
    assert!(!deleted);
    assert_eq!(stored_kinds(&pool).await, Vec::<String>::new());
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_event_insert_rolls_back_the_unlink(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    let links = PostgresOAuthAccountRepository::new(db(&pool));
    links
        .save(
            &google_link(id, "sub-1"),
            &AuditEvent::sign_in_method_linked(id, OAuthProvider::Google),
        )
        .await
        .unwrap();
    refuse_every_event(&pool).await;

    let err = links
        .delete_by_user_and_provider(
            &id,
            OAuthProvider::Google,
            &AuditEvent::sign_in_method_unlinked(id, OAuthProvider::Google),
        )
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    assert_eq!(link_count(&pool, &id).await, 1);
}

// --- account deleted by the purge

/// An account whose deletion grace period is over, anonymised as the purge
/// would, ready for `anonymize_account`.
async fn due_for_purge(pool: &PgPool) -> User {
    let user = seed_with_role(pool, UserRole::User).await;
    testing::backdate_deletion_scheduled(pool, *user.id().as_uuid(), 1).await;
    let mut due = user_repo(pool)
        .find_by_id(user.id())
        .await
        .unwrap()
        .unwrap();
    let anonymous = Email::new(&format!("deleted-{}@anonymized.local", user.id())).unwrap();
    due.anonymize(anonymous, Timestamp::now()).unwrap();
    due
}

async fn is_anonymised(pool: &PgPool, id: &UserId) -> bool {
    sqlx::query_scalar("SELECT deleted_at IS NOT NULL FROM users WHERE id = $1")
        .bind(id.as_uuid())
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn purge_writes_one_event_and_keeps_earlier_ones(pool: PgPool) {
    let due = due_for_purge(&pool).await;
    let id = *due.id();
    // An earlier event about the account, which must survive its deletion.
    user_repo(&pool)
        .update_password(&id, "h", &AuditEvent::password_reset(id))
        .await
        .unwrap();

    let applied = user_repo(&pool)
        .anonymize_account(&due, &AuditEvent::account_deleted(id))
        .await
        .unwrap();

    assert!(applied);
    assert!(is_anonymised(&pool, &id).await);
    assert_eq!(
        stored_kinds(&pool).await,
        vec!["password_reset", "account_deleted"]
    );
    let deleted = &stored_events(&pool).await[1];
    assert_eq!(
        (deleted.1, deleted.2.as_str(), deleted.3),
        (*id.as_uuid(), "system", None)
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn purge_of_a_cancelled_deletion_leaves_no_event(pool: PgPool) {
    let due = due_for_purge(&pool).await;
    let id = *due.id();
    // The Member cancels after the purge listed the account.
    sqlx::query("UPDATE users SET deletion_scheduled_at = NULL WHERE id = $1")
        .bind(id.as_uuid())
        .execute(&pool)
        .await
        .unwrap();

    let applied = user_repo(&pool)
        .anonymize_account(&due, &AuditEvent::account_deleted(id))
        .await
        .unwrap();

    assert!(!applied);
    assert!(!is_anonymised(&pool, &id).await);
    assert_eq!(stored_kinds(&pool).await, Vec::<String>::new());
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_event_insert_rolls_back_the_purge(pool: PgPool) {
    let due = due_for_purge(&pool).await;
    let id = *due.id();
    refuse_every_event(&pool).await;

    let err = user_repo(&pool)
        .anonymize_account(&due, &AuditEvent::account_deleted(id))
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    assert!(!is_anonymised(&pool, &id).await);
}

// --- data exported

#[sqlx::test(migrations = "../../migrations")]
async fn export_record_writes_exactly_one_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    PostgresAuditLogRepository::new(db(&pool))
        .record(&AuditEvent::data_exported(id))
        .await
        .unwrap();
    assert_eq!(stored_kinds(&pool).await, vec!["data_exported"]);
}

/// The export use case returns this error instead of the data (unit-tested in
/// `export_user_data.rs`), so no export happens unrecorded.
#[sqlx::test(migrations = "../../migrations")]
async fn failed_export_record_is_an_error(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    refuse_every_event(&pool).await;
    let err = PostgresAuditLogRepository::new(db(&pool))
        .record(&AuditEvent::data_exported(id))
        .await
        .unwrap_err();
    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
}

// --- sessions signed out after token reuse

async fn seed_session(pool: &PgPool, id: UserId, hash: &str) {
    let token = RefreshToken::new(
        id,
        hash.into(),
        "device".into(),
        Timestamp::now() + SignedDuration::from_hours(24),
    );
    PostgresRefreshTokenRepository::new(db(pool))
        .save(&token)
        .await
        .unwrap();
}

async fn live_sessions(pool: &PgPool, id: &UserId) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM refresh_tokens WHERE user_id = $1 AND revoked_at IS NULL",
    )
    .bind(id.as_uuid())
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = "../../migrations")]
async fn reuse_revocation_writes_exactly_one_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    seed_session(&pool, id, "h1").await;
    seed_session(&pool, id, "h2").await;

    let revoked = PostgresRefreshTokenRepository::new(db(&pool))
        .revoke_all_for_user_audited(&id, &AuditEvent::sessions_revoked_after_reuse(id))
        .await
        .unwrap();

    assert_eq!(revoked, 2);
    assert_eq!(live_sessions(&pool, &id).await, 0);
    assert_eq!(
        stored_kinds(&pool).await,
        vec!["sessions_revoked_after_reuse"]
    );
    assert_eq!(stored_events(&pool).await[0].2, "system");
}

#[sqlx::test(migrations = "../../migrations")]
async fn reuse_revocation_with_no_live_session_leaves_no_event(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    let revoked = PostgresRefreshTokenRepository::new(db(&pool))
        .revoke_all_for_user_audited(&id, &AuditEvent::sessions_revoked_after_reuse(id))
        .await
        .unwrap();
    assert_eq!(revoked, 0);
    assert_eq!(stored_kinds(&pool).await, Vec::<String>::new());
}

#[sqlx::test(migrations = "../../migrations")]
async fn failed_event_insert_rolls_back_the_reuse_revocation(pool: PgPool) {
    let id = *seed_with_role(&pool, UserRole::User).await.id();
    seed_session(&pool, id, "h1").await;
    refuse_every_event(&pool).await;

    let err = PostgresRefreshTokenRepository::new(db(&pool))
        .revoke_all_for_user_audited(&id, &AuditEvent::sessions_revoked_after_reuse(id))
        .await
        .unwrap_err();

    assert!(matches!(err, DomainError::Persistence(_)), "{err:?}");
    assert_eq!(live_sessions(&pool, &id).await, 1);
}
