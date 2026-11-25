//! Integration test for the GDPR account-anonymization transaction.
//!
//! Seeds a user plus a row in every child table the anonymize transaction is
//! responsible for, runs the real Postgres `anonymize_account`, and asserts the
//! PII is scrubbed and every child row is gone. This is the multi-table
//! transactional purge that previously had only in-memory coverage.

use std::sync::Arc;

use jiff::{SignedDuration, Timestamp};
use sqlx::PgPool;
use uuid::Uuid;

use application::ports::{
    AccountLifecycleRepository, EmailVerificationRepository, RefreshTokenRepository, UserRepository,
};
use application::use_cases::PurgeScheduledAccountsUseCase;
use domain::entities::{EmailVerificationCode, RefreshToken, User};
use domain::value_objects::Email;
use persistence_postgres::time::ToDb;
use persistence_postgres::{
    Database, PostgresEmailVerificationRepository, PostgresRefreshTokenRepository,
    PostgresUserRepository,
};

/// Total rows across every child table the anonymize transaction must purge,
/// for the given user. Compile-checked per-table counts summed in code so the
/// queries stay `sqlx::query!`-validated.
async fn child_row_count(pool: &PgPool, user_id: Uuid) -> i64 {
    let refresh = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM refresh_tokens WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count refresh_tokens")
    .unwrap_or(0);
    let oauth = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM user_oauth_accounts WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count user_oauth_accounts")
    .unwrap_or(0);
    let verification = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM email_verification_codes WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count email_verification_codes")
    .unwrap_or(0);
    let reset = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM password_reset_codes WHERE user_id = $1",
        user_id
    )
    .fetch_one(pool)
    .await
    .expect("count password_reset_codes")
    .unwrap_or(0);
    refresh + oauth + verification + reset
}

/// Put the account in the state the purge looks for: pending deletion with the
/// grace period over.
async fn schedule_deletion_in_past(pool: &PgPool, user_id: Uuid) {
    sqlx::query!(
        "UPDATE users SET deletion_scheduled_at = NOW() - INTERVAL '1 minute' WHERE id = $1",
        user_id,
    )
    .execute(pool)
    .await
    .expect("schedule deletion");
}

#[sqlx::test(migrations = "../../migrations")]
async fn anonymize_scrubs_pii_and_purges_child_rows(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));

    let user_repo = PostgresUserRepository::new(db.clone());
    let refresh_repo = PostgresRefreshTokenRepository::new(db.clone());
    let verification_repo = PostgresEmailVerificationRepository::new(db);

    // --- Seed: a complete user with a row in every anonymize-managed child table.
    let mut user = User::new(
        Email::new("real-person@example.com").unwrap(),
        Some("argon2-hash".to_string()),
        "Real".to_string(),
        "Person".to_string(),
    );
    user.mark_email_verified();
    user_repo.save(&user).await.expect("save user");
    let user_id = *user.id().as_uuid();
    schedule_deletion_in_past(&pool, user_id).await;

    refresh_repo
        .save(&RefreshToken::new(
            *user.id(),
            "token-hash".to_string(),
            "device-fp".to_string(),
            Timestamp::now() + SignedDuration::from_hours(24 * 30),
        ))
        .await
        .expect("seed refresh token");

    verification_repo
        .save(&EmailVerificationCode::new(
            *user.id(),
            "code-hash".to_string(),
        ))
        .await
        .expect("seed verification code");

    sqlx::query!(
        "INSERT INTO user_oauth_accounts (user_id, provider, provider_user_id) \
         VALUES ($1, 'google', $2)",
        user_id,
        "google-sub-123",
    )
    .execute(&pool)
    .await
    .expect("seed oauth account");

    sqlx::query!(
        "INSERT INTO password_reset_codes (user_id, code_hash, expires_at) VALUES ($1, $2, $3)",
        user_id,
        "reset-hash",
        (Timestamp::now() + SignedDuration::from_hours(1)).to_db(),
    )
    .execute(&pool)
    .await
    .expect("seed reset code");

    let list_id = sqlx::query_scalar!(
        "INSERT INTO lists (id, user_id, creation_key, name)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'Private') RETURNING id",
        user_id,
    )
    .fetch_one(&pool)
    .await
    .expect("seed list");
    sqlx::query!(
        "INSERT INTO todos (id, list_id, creation_key, title, position)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'Secret todo', 1)",
        list_id,
    )
    .execute(&pool)
    .await
    .expect("seed todo");

    // Sanity: one row in each of the four child tables before deletion.
    assert_eq!(
        child_row_count(&pool, user_id).await,
        4,
        "seeding should create one row per child table",
    );

    // --- Act: anonymize (the data-scrub half of account deletion).
    let mut anonymized = user.clone();
    let anon_email = Email::new(&format!("deleted-{user_id}@anonymized.local")).unwrap();
    anonymized
        .anonymize(anon_email, jiff::Timestamp::now())
        .unwrap();
    let anonymized_now = user_repo
        .anonymize_account(
            &anonymized,
            &domain::entities::AuditEvent::account_deleted(*anonymized.id()),
        )
        .await
        .expect("anonymize");
    assert!(anonymized_now, "a due account must be anonymized");

    // --- Assert: PII scrubbed on the user row.
    let row = sqlx::query!(
        "SELECT email, password_hash, deleted_at, deletion_scheduled_at FROM users WHERE id = $1",
        user_id,
    )
    .fetch_one(&pool)
    .await
    .expect("load user");

    assert_eq!(row.email, format!("deleted-{user_id}@anonymized.local"));
    assert!(row.password_hash.is_none(), "password hash must be cleared");
    assert!(row.deleted_at.is_some(), "deleted_at must be set");
    assert!(
        row.deletion_scheduled_at.is_none(),
        "deletion_scheduled_at must be cleared",
    );
    assert!(
        !row.email.contains("real-person"),
        "original email must not survive",
    );

    // --- Assert: every child table is empty for this user.
    assert_eq!(
        child_row_count(&pool, user_id).await,
        0,
        "all child rows must be purged",
    );
    let lists = sqlx::query_scalar!("SELECT COUNT(*) FROM lists WHERE user_id = $1", user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    let todos = sqlx::query_scalar!("SELECT COUNT(*) FROM todos WHERE list_id = $1", list_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!((lists, todos), (0, 0), "lists and todos are erased");
}

/// The schema enforces what `AccountStatus` encodes: a deleted account cannot
/// also be pending deletion, whichever code path tries to write it.
#[sqlx::test(migrations = "../../migrations")]
async fn deleted_account_cannot_be_pending_deletion(pool: PgPool) {
    let now = Timestamp::now();
    let err = sqlx::query!(
        r#"
        INSERT INTO users (email, deleted_at, deletion_scheduled_at)
        VALUES ('both@example.com', $1, $2)
        "#,
        now.to_db(),
        (now + SignedDuration::from_hours(24 * 14)).to_db(),
    )
    .execute(&pool)
    .await
    .expect_err("deleted + scheduled must violate the check constraint");

    let db_err = err.as_database_error().expect("a database error");
    assert_eq!(
        db_err.constraint(),
        Some("users_deleted_not_pending_deletion")
    );
}

/// The purge lists due accounts, then anonymizes them one at a time. A user who
/// cancels in between must keep their account and credentials.
#[sqlx::test(migrations = "../../migrations")]
async fn anonymize_skips_account_cancelled_after_the_scan(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let user_repo = PostgresUserRepository::new(db.clone());
    let refresh_repo = PostgresRefreshTokenRepository::new(db);

    let mut user = User::new(
        Email::new("changed-mind@example.com").unwrap(),
        Some("argon2-hash".to_string()),
        "Changed".to_string(),
        "Mind".to_string(),
    );
    user.mark_email_verified();
    user_repo.save(&user).await.expect("save user");
    let user_id = *user.id().as_uuid();
    schedule_deletion_in_past(&pool, user_id).await;
    refresh_repo
        .save(&RefreshToken::new(
            *user.id(),
            "token-hash".to_string(),
            "device-fp".to_string(),
            Timestamp::now() + SignedDuration::from_hours(24 * 30),
        ))
        .await
        .expect("seed refresh token");

    // The scan sees the account as due...
    let listed = user_repo
        .find_ready_for_deletion()
        .await
        .expect("list due accounts");
    let listed = listed
        .into_iter()
        .find(|u| *u.id().as_uuid() == user_id)
        .expect("account listed as due");

    // ...then the user cancels before the purge reaches them.
    sqlx::query!(
        "UPDATE users SET deletion_scheduled_at = NULL, version = version + 1 WHERE id = $1",
        user_id,
    )
    .execute(&pool)
    .await
    .expect("cancel deletion");

    let mut anonymized = listed;
    anonymized
        .anonymize(
            Email::new(&format!("deleted-{user_id}@anonymized.local")).unwrap(),
            jiff::Timestamp::now(),
        )
        .unwrap();
    let anonymized_now = user_repo
        .anonymize_account(
            &anonymized,
            &domain::entities::AuditEvent::account_deleted(*anonymized.id()),
        )
        .await
        .expect("anonymize");

    assert!(!anonymized_now, "a cancelled account must be skipped");
    let row = sqlx::query!(
        "SELECT email, password_hash, deleted_at FROM users WHERE id = $1",
        user_id,
    )
    .fetch_one(&pool)
    .await
    .expect("load user");
    assert_eq!(row.email, "changed-mind@example.com");
    assert!(row.password_hash.is_some());
    assert!(row.deleted_at.is_none());
    assert_eq!(
        child_row_count(&pool, user_id).await,
        1,
        "refresh token kept"
    );
}

/// A list belongs to one Member and a purge never reaches another's.
#[sqlx::test(migrations = "../../migrations")]
async fn purging_one_account_leaves_another_members_lists_untouched(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let user_repo = PostgresUserRepository::new(db);

    let mut doomed = User::new(
        Email::new("doomed@example.com").unwrap(),
        None,
        "Doomed".to_string(),
        "Member".to_string(),
    );
    doomed.mark_email_verified();
    user_repo.save(&doomed).await.unwrap();
    let doomed_id = *doomed.id().as_uuid();
    schedule_deletion_in_past(&pool, doomed_id).await;

    let mut survivor = User::new(
        Email::new("survivor@example.com").unwrap(),
        None,
        "Surviving".to_string(),
        "Member".to_string(),
    );
    survivor.mark_email_verified();
    user_repo.save(&survivor).await.unwrap();
    let survivor_id = *survivor.id().as_uuid();

    for user_id in [doomed_id, survivor_id] {
        sqlx::query!(
            "INSERT INTO lists (id, user_id, creation_key, name)
             VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'A list')",
            user_id,
        )
        .execute(&pool)
        .await
        .unwrap();
    }

    let purged = PurgeScheduledAccountsUseCase::new(Arc::new(user_repo))
        .run_once()
        .await
        .unwrap();
    assert_eq!(purged, 1);
    for (user_id, expected) in [(doomed_id, 0), (survivor_id, 1)] {
        let count = sqlx::query_scalar!("SELECT COUNT(*) FROM lists WHERE user_id = $1", user_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .unwrap_or(0);
        assert_eq!(count, expected);
    }
}

/// A cancelled deletion keeps the Member's lists and todos.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cancelled_deletion_keeps_lists_and_todos(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let user_repo = PostgresUserRepository::new(db);

    let mut user = User::new(
        Email::new("cancelled@example.com").unwrap(),
        None,
        "Cancel".to_string(),
        "Member".to_string(),
    );
    user.mark_email_verified();
    user_repo.save(&user).await.unwrap();
    let user_id = *user.id().as_uuid();
    schedule_deletion_in_past(&pool, user_id).await;
    let list_id = sqlx::query_scalar!(
        "INSERT INTO lists (id, user_id, creation_key, name)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'Kept') RETURNING id",
        user_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO todos (id, list_id, creation_key, title, position)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'Kept todo', 1)",
        list_id,
    )
    .execute(&pool)
    .await
    .unwrap();

    // The Member cancels before the next pass.
    sqlx::query!(
        "UPDATE users SET deletion_scheduled_at = NULL WHERE id = $1",
        user_id
    )
    .execute(&pool)
    .await
    .unwrap();

    let purged = PurgeScheduledAccountsUseCase::new(Arc::new(user_repo))
        .run_once()
        .await
        .unwrap();
    assert_eq!(purged, 0);
    let lists = sqlx::query_scalar!("SELECT COUNT(*) FROM lists WHERE user_id = $1", user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    let todos = sqlx::query_scalar!("SELECT COUNT(*) FROM todos WHERE list_id = $1", list_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!((lists, todos), (1, 1));
}

/// A failure while erasing leaves the whole purge due for the next run.
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_erase_leaves_the_account_due_and_retries(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let user_repo = PostgresUserRepository::new(db);

    let mut user = User::new(
        Email::new("retry@example.com").unwrap(),
        None,
        "Retry".to_string(),
        "Member".to_string(),
    );
    user.mark_email_verified();
    user_repo.save(&user).await.unwrap();
    let user_id = *user.id().as_uuid();
    schedule_deletion_in_past(&pool, user_id).await;
    let list_id = sqlx::query_scalar!(
        "INSERT INTO lists (id, user_id, creation_key, name)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'Still here') RETURNING id",
        user_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Injected failure: deleting this Member's list raises.
    sqlx::query(
        "CREATE FUNCTION fail_lists_delete() RETURNS trigger AS $$
         BEGIN RAISE EXCEPTION 'injected erase failure'; END;
         $$ LANGUAGE plpgsql",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE TRIGGER fail_lists BEFORE DELETE ON lists
         FOR EACH ROW EXECUTE FUNCTION fail_lists_delete()",
    )
    .execute(&pool)
    .await
    .unwrap();

    let mut anonymized = user.clone();
    anonymized
        .anonymize(
            Email::new(&format!("deleted-{user_id}@anonymized.local")).unwrap(),
            Timestamp::now(),
        )
        .unwrap();
    let failed = user_repo
        .anonymize_account(
            &anonymized,
            &domain::entities::AuditEvent::account_deleted(
                domain::value_objects::UserId::from_uuid(user_id),
            ),
        )
        .await;
    assert!(
        failed.is_err(),
        "the injected failure must abort the transaction"
    );

    let row = sqlx::query!(
        "SELECT deleted_at, deletion_scheduled_at FROM users WHERE id = $1",
        user_id
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(row.deleted_at.is_none(), "the account is still due");
    assert!(row.deletion_scheduled_at.is_some());
    let remaining = sqlx::query_scalar!("SELECT COUNT(*) FROM lists WHERE id = $1", list_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!(remaining, 1, "the failed transaction changed nothing");

    sqlx::query("DROP TRIGGER fail_lists ON lists")
        .execute(&pool)
        .await
        .unwrap();
    let retried = PurgeScheduledAccountsUseCase::new(Arc::new(user_repo))
        .run_once()
        .await
        .unwrap();
    assert_eq!(retried, 1, "the next run completes both steps");
    let gone = sqlx::query_scalar!("SELECT COUNT(*) FROM lists WHERE id = $1", list_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!(gone, 0);
}

/// Each purge pass sweeps lists left by a purge from an older release, even
/// when no account is due.
#[sqlx::test(migrations = "../../migrations")]
async fn the_sweep_runs_before_the_no_due_early_return(pool: PgPool) {
    let user_id = sqlx::query_scalar!(
        "INSERT INTO users (id, email, deleted_at, email_verified)
         VALUES (gen_random_uuid(), 'old-purge@example.com', NOW(), true) RETURNING id"
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let list_id = sqlx::query_scalar!(
        "INSERT INTO lists (id, user_id, creation_key, name)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'Left behind') RETURNING id",
        user_id,
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query!(
        "INSERT INTO todos (id, list_id, creation_key, title, position)
         VALUES (gen_random_uuid(), $1, gen_random_uuid(), 'Left behind', 1)",
        list_id,
    )
    .execute(&pool)
    .await
    .unwrap();

    let db = Arc::new(Database::from_pool(pool.clone()));
    let user_repo = Arc::new(PostgresUserRepository::new(db));
    let purged = PurgeScheduledAccountsUseCase::new(user_repo.clone())
        .run_once()
        .await
        .unwrap();
    assert_eq!(purged, 0, "no account is due");
    let lists = sqlx::query_scalar!("SELECT COUNT(*) FROM lists WHERE user_id = $1", user_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    let todos = sqlx::query_scalar!("SELECT COUNT(*) FROM todos WHERE list_id = $1", list_id)
        .fetch_one(&pool)
        .await
        .unwrap()
        .unwrap_or(0);
    assert_eq!((lists, todos), (0, 0), "the sweep cleared the leftovers");
    let swept = user_repo.delete_lists_of_deleted_accounts().await.unwrap();
    assert_eq!(swept, 0, "a second sweep has nothing left");
}
