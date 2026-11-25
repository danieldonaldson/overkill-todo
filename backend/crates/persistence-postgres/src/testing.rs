//! Postgres test and benchmark fixtures shared across crates via `test-support`.
//!
//! Queries use `sqlx::query!` / `sqlx::query_scalar!` wherever Postgres allows
//! prepared statements so SQL stays checked against `.sqlx`; the few
//! operations that cannot be prepared (DDL, `VACUUM`, dynamic catalog reads,
//! `MigrateDatabase`/`sqlx::migrate!`) keep runtime SQL with an explicit note.
#![allow(
    clippy::missing_panics_doc,
    reason = "test and benchmark fixtures intentionally panic on unexpected database state"
)]

use domain::entities::OAuthProvider;
use domain::value_objects::UserRole;
use jiff::Timestamp;
use sqlx::migrate::MigrateDatabase;
use sqlx::{PgPool, Postgres};
use uuid::Uuid;

use crate::models::{DbOAuthProvider, DbUserRole};
use crate::time::{ToDb, ToDomain};

/// One-time code tables that share attempt-lockout columns.
#[derive(Clone, Copy)]
pub enum CodeTable {
    EmailVerification,
    PasswordReset,
}

/// Subset of user columns asserted across deletion/purger tests.
pub struct UserSnapshot {
    pub email: String,
    pub deleted_at: Option<Timestamp>,
    pub deletion_scheduled_at: Option<Timestamp>,
    pub password_hash: Option<String>,
    pub first_name: String,
}

/// Removes the todos table, so the next write fails like a broken database.
/// DDL cannot be prepared, hence runtime SQL.
pub async fn drop_todos_table(pool: &PgPool) {
    sqlx::query("DROP TABLE todos CASCADE")
        .execute(pool)
        .await
        .unwrap();
}

/// Inserts a verified user with `email` and `role`, returning its `id`.
pub async fn insert_verified_user(pool: &PgPool, email: &str, role: UserRole) -> Uuid {
    let id = Uuid::now_v7();
    let db_role = DbUserRole::from(role);
    sqlx::query!(
        "INSERT INTO users (id, email, role, email_verified) VALUES ($1, $2, $3, true)",
        id,
        email,
        db_role as DbUserRole,
    )
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Inserts a verified user whose deletion is scheduled `scheduled_in_days` from now.
pub async fn insert_scheduled_deletion_user(
    pool: &PgPool,
    email: &str,
    scheduled_in_days: i32,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query!(
        "INSERT INTO users (id, email, password_hash, first_name, last_name, email_verified, deletion_scheduled_at)
         VALUES ($1, $2, 'hash', 'Real', 'Name', true, NOW() + make_interval(days => $3))",
        id,
        email,
        scheduled_in_days,
    )
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Looks up a user's `id` by exact email.
pub async fn user_id_by_email(pool: &PgPool, email: &str) -> Uuid {
    sqlx::query_scalar!("SELECT id FROM users WHERE email = $1", email)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Total rows in `users`.
pub async fn count_users(pool: &PgPool) -> i64 {
    sqlx::query_scalar!("SELECT COUNT(*) FROM users")
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap_or(0)
}

/// Number of `users` rows matching `email`.
pub async fn count_users_by_email(pool: &PgPool, email: &str) -> i64 {
    sqlx::query_scalar!("SELECT COUNT(*) FROM users WHERE email = $1", email)
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap_or(0)
}

/// Whether the user's email has been marked verified.
pub async fn is_email_verified(pool: &PgPool, user_id: Uuid) -> bool {
    sqlx::query_scalar!("SELECT email_verified FROM users WHERE id = $1", user_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Reads account deletion/scrubbing columns for `user_id`.
pub async fn user_snapshot(pool: &PgPool, user_id: Uuid) -> UserSnapshot {
    let row = sqlx::query!(
        "SELECT email, deleted_at, deletion_scheduled_at, password_hash, first_name
         FROM users WHERE id = $1",
        user_id,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    UserSnapshot {
        email: row.email,
        deleted_at: row.deleted_at.to_domain(),
        deletion_scheduled_at: row.deletion_scheduled_at.to_domain(),
        password_hash: row.password_hash,
        first_name: row.first_name,
    }
}

/// Clears a user's password hash and sets explicit first/last names (for IDOR checks).
pub async fn set_passwordless_user_name(
    pool: &PgPool,
    user_id: Uuid,
    first_name: &str,
    last_name: &str,
) {
    sqlx::query!(
        "UPDATE users SET first_name = $2, last_name = $3, password_hash = NULL WHERE id = $1",
        user_id,
        first_name,
        last_name,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Inserts an OAuth link row directly for test setup.
pub async fn insert_oauth_account(
    pool: &PgPool,
    user_id: Uuid,
    provider: OAuthProvider,
    provider_user_id: &str,
    provider_email: &str,
) {
    let db_provider = DbOAuthProvider::from(provider);
    sqlx::query!(
        "INSERT INTO user_oauth_accounts (user_id, provider, provider_user_id, provider_email)
         VALUES ($1, $2, $3, $4)",
        user_id,
        db_provider as DbOAuthProvider,
        provider_user_id,
        provider_email,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Concatenated JSON snapshot of a user and its linked OAuth and refresh-token rows.
pub async fn account_snapshot_text(pool: &PgPool, user_id: Uuid) -> String {
    sqlx::query_scalar!(
        "SELECT concat_ws(' | ',
            coalesce((SELECT row_to_json(u)::text FROM users u WHERE u.id = $1), 'null'),
            coalesce((SELECT json_agg(o ORDER BY o.id)::text FROM user_oauth_accounts o WHERE o.user_id = $1), 'null'),
            coalesce((SELECT json_agg(r ORDER BY r.id)::text FROM refresh_tokens r WHERE r.user_id = $1), 'null'))",
        user_id,
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or_default()
}

/// Marks `user_id` as deleted right now.
pub async fn mark_user_deleted_now(pool: &PgPool, user_id: Uuid) {
    sqlx::query!("UPDATE users SET deleted_at = NOW() WHERE id = $1", user_id)
        .execute(pool)
        .await
        .unwrap();
}

/// Moves a user's `deletion_scheduled_at` `days` into the past.
pub async fn backdate_deletion_scheduled(pool: &PgPool, user_id: Uuid, days: i32) {
    sqlx::query!(
        "UPDATE users SET deletion_scheduled_at = NOW() - make_interval(days => $2) WHERE id = $1",
        user_id,
        days,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Backdates `tokens_valid_after` by 1 minute and returns the user's current role text.
pub async fn backdate_tokens_valid_after_and_role(pool: &PgPool, user_id: Uuid) -> String {
    sqlx::query_scalar!(
        "UPDATE users SET tokens_valid_after = NOW() - INTERVAL '1 minute' WHERE id = $1
         RETURNING role::text",
        user_id,
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or_default()
}

/// Expires the user's one-time code in `table` by 1 hour.
pub async fn expire_code_for_email(pool: &PgPool, table: CodeTable, email: &str) {
    match table {
        CodeTable::EmailVerification => {
            sqlx::query!(
                "UPDATE email_verification_codes SET expires_at = NOW() - INTERVAL '1 hour'
                 WHERE user_id = (SELECT id FROM users WHERE email = $1)",
                email,
            )
            .execute(pool)
            .await
            .unwrap();
        }
        CodeTable::PasswordReset => {
            sqlx::query!(
                "UPDATE password_reset_codes SET expires_at = NOW() - INTERVAL '1 hour'
                 WHERE user_id = (SELECT id FROM users WHERE email = $1)",
                email,
            )
            .execute(pool)
            .await
            .unwrap();
        }
    }
}

/// Updates the `attempts` counter on a user's code row.
pub async fn set_code_attempts(pool: &PgPool, table: CodeTable, user_id: Uuid, attempts: i32) {
    match table {
        CodeTable::EmailVerification => {
            sqlx::query!(
                "UPDATE email_verification_codes SET attempts = $2 WHERE user_id = $1",
                user_id,
                attempts,
            )
            .execute(pool)
            .await
            .unwrap();
        }
        CodeTable::PasswordReset => {
            sqlx::query!(
                "UPDATE password_reset_codes SET attempts = $2 WHERE user_id = $1",
                user_id,
                attempts,
            )
            .execute(pool)
            .await
            .unwrap();
        }
    }
}

/// Moves `last_sent_at` `minutes_ago` into the past on a user's code row.
pub async fn age_code_last_sent(pool: &PgPool, table: CodeTable, user_id: Uuid, minutes_ago: i32) {
    match table {
        CodeTable::EmailVerification => {
            sqlx::query!(
                "UPDATE email_verification_codes
                 SET last_sent_at = NOW() - make_interval(mins => $2)
                 WHERE user_id = $1",
                user_id,
                minutes_ago,
            )
            .execute(pool)
            .await
            .unwrap();
        }
        CodeTable::PasswordReset => {
            sqlx::query!(
                "UPDATE password_reset_codes
                 SET last_sent_at = NOW() - make_interval(mins => $2)
                 WHERE user_id = $1",
                user_id,
                minutes_ago,
            )
            .execute(pool)
            .await
            .unwrap();
        }
    }
}

/// Sets a code row to 4 attempts, 0 lockouts, sent 6 minutes ago, live for 10 minutes.
pub async fn arm_code_for_race(pool: &PgPool, table: CodeTable, user_id: Uuid) -> u64 {
    match table {
        CodeTable::EmailVerification => sqlx::query!(
            "UPDATE email_verification_codes
             SET attempts = 4, lockouts = 0,
                 last_sent_at = NOW() - INTERVAL '6 minutes',
                 expires_at = NOW() + INTERVAL '10 minutes'
             WHERE user_id = $1",
            user_id,
        )
        .execute(pool)
        .await
        .unwrap()
        .rows_affected(),
        CodeTable::PasswordReset => sqlx::query!(
            "UPDATE password_reset_codes
             SET attempts = 4, lockouts = 0,
                 last_sent_at = NOW() - INTERVAL '6 minutes',
                 expires_at = NOW() + INTERVAL '10 minutes'
             WHERE user_id = $1",
            user_id,
        )
        .execute(pool)
        .await
        .unwrap()
        .rows_affected(),
    }
}

/// Reads `(attempts, lockouts)` for `user_id` in `table`.
pub async fn code_counters(pool: &PgPool, table: CodeTable, user_id: Uuid) -> (i32, i32) {
    match table {
        CodeTable::EmailVerification => {
            let row = sqlx::query!(
                "SELECT attempts, lockouts FROM email_verification_codes WHERE user_id = $1",
                user_id,
            )
            .fetch_one(pool)
            .await
            .unwrap();
            (row.attempts, row.lockouts)
        }
        CodeTable::PasswordReset => {
            let row = sqlx::query!(
                "SELECT attempts, lockouts FROM password_reset_codes WHERE user_id = $1",
                user_id,
            )
            .fetch_one(pool)
            .await
            .unwrap();
            (row.attempts, row.lockouts)
        }
    }
}

/// Sets the verification magic-token hash and expiry on a user's verification code.
pub async fn set_verification_magic_token(
    pool: &PgPool,
    user_id: Uuid,
    hash: &str,
    expires_at: Timestamp,
) -> u64 {
    sqlx::query!(
        "UPDATE email_verification_codes
         SET magic_token_hash = $2, magic_token_expires_at = $3
         WHERE user_id = $1",
        user_id,
        hash,
        expires_at.to_db(),
    )
    .execute(pool)
    .await
    .unwrap()
    .rows_affected()
}

/// Expires all refresh tokens belonging to `email`.
pub async fn expire_refresh_tokens_for_email(pool: &PgPool, email: &str) {
    sqlx::query!(
        "UPDATE refresh_tokens SET expires_at = NOW() - INTERVAL '1 minute'
         WHERE user_id = (SELECT id FROM users WHERE email = $1)",
        email,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Returns `(total, active)` refresh token counts for `user_id`.
pub async fn refresh_token_counts(pool: &PgPool, user_id: Uuid) -> (i64, i64) {
    let row = sqlx::query!(
        "SELECT COUNT(*) AS total,
                COUNT(*) FILTER (WHERE revoked_at IS NULL) AS active
         FROM refresh_tokens WHERE user_id = $1",
        user_id,
    )
    .fetch_one(pool)
    .await
    .unwrap();
    (row.total.unwrap_or(0), row.active.unwrap_or(0))
}

/// Whether the refresh token `token_id` has a `revoked_at` timestamp.
pub async fn is_refresh_token_revoked(pool: &PgPool, token_id: Uuid) -> bool {
    sqlx::query_scalar!(
        "SELECT revoked_at IS NOT NULL FROM refresh_tokens WHERE id = $1",
        token_id,
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or(false)
}

/// Reads the `trace_id` stored on the audit event row.
pub async fn latest_audit_event_trace_id(pool: &PgPool) -> Option<String> {
    sqlx::query_scalar!("SELECT trace_id FROM audit_events ORDER BY id DESC LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Returns every stored `audit_events.kind` ordered by `id`.
pub async fn audit_event_kinds(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar!("SELECT kind FROM audit_events ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Returns every `audit_events` row serialized as JSON text.
pub async fn audit_event_rows_json(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar!("SELECT row_to_json(e)::text FROM audit_events e")
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .flatten()
        .collect()
}

/// Adds a check constraint that rejects inserting `role_changed` audit events.
pub async fn refuse_role_changed_audit_events(pool: &PgPool) {
    // DDL (`ALTER TABLE ... ADD CONSTRAINT`) cannot be prepared by `sqlx::query!`.
    sqlx::query(
        "ALTER TABLE audit_events ADD CONSTRAINT test_refuse_role_changed
         CHECK (kind <> 'role_changed') NOT VALID",
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Every row of every table in the `public` schema, as text.
pub async fn dump_database(pool: &PgPool) -> String {
    let tables: Vec<String> = sqlx::query_scalar!(
        "SELECT table_name::text FROM information_schema.tables
         WHERE table_schema = 'public' AND table_type = 'BASE TABLE'"
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .into_iter()
    .flatten()
    .collect();
    assert!(
        tables.iter().any(|t| t == "users"),
        "dumped the wrong schema: {tables:?}"
    );
    let mut dump = String::new();
    for table in tables {
        // Table names come from `information_schema` and are interpolated dynamically,
        // so this per-table dump cannot use a static `sqlx::query_scalar!`.
        let rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT row_to_json(t)::text FROM \"{table}\" t"
        )))
        .fetch_all(pool)
        .await
        .unwrap();
        for row in rows {
            dump.push_str(&table);
            dump.push_str(": ");
            dump.push_str(&row);
            dump.push('\n');
        }
    }
    dump
}

/// Drops, creates, and migrates a dedicated benchmark database, returning a connected pool.
pub async fn recreate_bench_pool(url: &str) -> PgPool {
    // `MigrateDatabase` DDL and `sqlx::migrate!` manage the database schema lifecycle
    // rather than static queryable statements.
    if Postgres::database_exists(url).await.unwrap() {
        Postgres::drop_database(url).await.unwrap();
    }
    Postgres::create_database(url).await.unwrap();
    let pool = PgPool::connect(url).await.unwrap();
    sqlx::migrate!("../../migrations").run(&pool).await.unwrap();
    pool
}

/// Spreads `count` synthetic `role_changed` audit events across existing accounts and runs `VACUUM ANALYZE`.
pub async fn seed_bench_audit_events(pool: &PgPool, count: i32) {
    sqlx::query!(
        "WITH accounts AS (
           SELECT array_agg(id ORDER BY id) AS ids FROM users
         )
         INSERT INTO audit_events (kind, subject_id, actor_kind, actor_id, details)
         SELECT 'role_changed',
                ids[1 + n % array_length(ids, 1)],
                'user',
                ids[1 + (n + 1) % array_length(ids, 1)],
                '{\"from\": \"user\", \"to\": \"admin\"}'
         FROM accounts, generate_series(1, $1) AS n",
        count,
    )
    .execute(pool)
    .await
    .unwrap();
    // `VACUUM ANALYZE` cannot execute inside a Postgres prepared statement.
    sqlx::query("VACUUM ANALYZE audit_events")
        .execute(pool)
        .await
        .unwrap();
}

/// Returns the first user `id` as text for benchmark filtering.
pub async fn first_bench_user_id(pool: &PgPool) -> String {
    sqlx::query_scalar!("SELECT id::text FROM users ORDER BY id LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
        .unwrap_or_default()
}

/// Inserts a list owned by `user_id`, returning its ID.
pub async fn insert_list(pool: &PgPool, user_id: Uuid, name: &str) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO lists (id, user_id, creation_key, name) VALUES ($1, $2, $3, $4)",
        id,
        user_id,
        Uuid::new_v4(),
        name,
    )
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Inserts a todo into `list_id`; `completed_at` set makes it done and
/// `position` must be `None`, and vice versa.
pub async fn insert_todo(
    pool: &PgPool,
    list_id: Uuid,
    title: &str,
    position: Option<i64>,
    completed_at: Option<Timestamp>,
) -> Uuid {
    let id = Uuid::now_v7();
    sqlx::query!(
        "INSERT INTO todos (id, list_id, creation_key, title, position, completed_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
        id,
        list_id,
        Uuid::new_v4(),
        title,
        position,
        completed_at.to_db(),
    )
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Inserts `count` lists for the account with `email`, oldest first.
pub async fn insert_lists_for_email(pool: &PgPool, email: &str, count: i32) {
    let user_id: Uuid = sqlx::query_scalar!("SELECT id FROM users WHERE email = $1", email)
        .fetch_one(pool)
        .await
        .unwrap();
    for i in 0..count {
        sqlx::query!(
            "INSERT INTO lists (id, user_id, creation_key, name) VALUES (gen_random_uuid(), $1, gen_random_uuid(), $2)",
            user_id,
            format!("List {i}"),
        )
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Inserts `count` open todos at the bottom of `list_id`.
pub async fn insert_todos_for_list(pool: &PgPool, list_id: Uuid, count: i32) {
    sqlx::query!(
        "INSERT INTO todos (id, list_id, creation_key, title, position)
         SELECT gen_random_uuid(), $1, gen_random_uuid(), 't' || n, n::bigint * 1048576
         FROM generate_series(1, $2) AS n",
        list_id,
        count,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Marks the account with `email` deleted now, as the purger would.
pub async fn mark_user_deleted_by_email(pool: &PgPool, email: &str) {
    sqlx::query!(
        "UPDATE users SET deleted_at = NOW() WHERE email = $1",
        email
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Writes a title the domain would never accept, so reading the todo fails.
pub async fn corrupt_todo_title(pool: &PgPool, todo_id: Uuid) {
    sqlx::query!(
        "UPDATE todos SET title = 'bad' || chr(10) || 'title' WHERE id = $1",
        todo_id,
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Number of audit events of `kind` for the account with `email`.
pub async fn count_audit_events_for_email(pool: &PgPool, email: &str, kind: &str) -> i64 {
    sqlx::query_scalar!(
        "SELECT COUNT(*) FROM audit_events WHERE kind = $1 AND subject_id = (SELECT id FROM users WHERE email = $2)",
        kind,
        email,
    )
    .fetch_one(pool)
    .await
    .unwrap()
    .unwrap_or(0)
}
