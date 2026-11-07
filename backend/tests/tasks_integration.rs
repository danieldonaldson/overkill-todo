//! Integration tests for DB-backed background-task machinery:
//!
//! - `leader::run_as_leader` — Postgres advisory-lock leader election
//!   (acquire, contention between two replicas, shutdown + lock release).
//! - `PurgeScheduledAccountsUseCase::run_once` driven against the real Postgres
//!   repositories (the run-loops themselves are infinite `tokio` loops, so we exercise the single-iteration use cases they
//!   call, which is where all the DB logic lives).
//!
//! Test glue uses runtime `sqlx::query*` (not the compile-checked macros) so the
//! suite builds under `SQLX_OFFLINE=true` without regenerating the `.sqlx` cache.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sqlx::PgPool;
use tokio::sync::{Notify, watch};

use app::application::use_cases::PurgeScheduledAccountsUseCase;
use app::infrastructure::persistence::{Database, PostgresUserRepository};
use app::infrastructure::tasks::leader::run_as_leader;

async fn try_lock(pool: &PgPool, key: i64) -> bool {
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
        .bind(key)
        .fetch_one(&mut *conn)
        .await
        .unwrap()
}

// ===========================================================================
// leader::run_as_leader — advisory-lock leader election
// ===========================================================================

/// A single replica acquires the lock, runs its worker, and steps down cleanly
/// when shutdown is signalled, releasing the advisory lock.
#[sqlx::test(migrations = "./migrations")]
async fn leader_acquires_runs_and_releases_on_shutdown(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let lock_key: i64 = 9_000_001;

    let ran = Arc::new(Notify::new());
    let ran_signal = ran.clone();
    let (tx, rx) = watch::channel(false);

    let db_for_leader = db.clone();
    let handle = tokio::spawn(async move {
        run_as_leader(db_for_leader, lock_key, "test-leader", rx, move || {
            let ran_signal = ran_signal.clone();
            async move {
                // Mark that we became leader, then idle until cancelled.
                ran_signal.notify_one();
                std::future::pending::<()>().await;
            }
        })
        .await;
    });

    // Wait until the worker ran (leadership acquired).
    tokio::time::timeout(Duration::from_secs(5), ran.notified())
        .await
        .expect("worker should have started as leader");

    // While held, the lock is not grantable to another session.
    let mut other = pool.acquire().await.unwrap();
    let acquired_elsewhere = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
        .bind(lock_key)
        .fetch_one(&mut *other)
        .await
        .unwrap();
    assert!(
        !acquired_elsewhere,
        "lock should be held by the leader, not grantable elsewhere"
    );

    // Signal shutdown; the leader cancels its worker, releases the lock, returns.
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("leader loop should stop on shutdown")
        .unwrap();

    // Release the lock we grabbed above, then confirm a fresh session can now
    // acquire it (proving the leader's own lock was released on step-down).
    sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(lock_key)
        .fetch_one(&mut *other)
        .await
        .unwrap();
    drop(other);

    assert!(
        try_lock(&pool, lock_key).await,
        "lock should be free after the leader stepped down"
    );
}

/// With the lock already held by another session, the would-be leader stands by
/// (its worker never runs) until shutdown.
#[sqlx::test(migrations = "./migrations")]
async fn leader_stands_by_when_lock_held_elsewhere(pool: PgPool) {
    let db = Arc::new(Database::from_pool(pool.clone()));
    let lock_key: i64 = 9_000_002;

    // Hold the lock on a dedicated connection so the leader can't acquire it.
    let mut holder = pool.acquire().await.unwrap();
    let held = sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock($1)")
        .bind(lock_key)
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    assert!(held, "test must hold the lock first");

    let ran = Arc::new(AtomicBool::new(false));
    let ran_flag = ran.clone();
    let (tx, rx) = watch::channel(false);

    let handle = tokio::spawn(async move {
        run_as_leader(db, lock_key, "standby-leader", rx, move || {
            let ran_flag = ran_flag.clone();
            async move {
                ran_flag.store(true, Ordering::SeqCst);
                std::future::pending::<()>().await;
            }
        })
        .await;
    });

    // Give the loop a couple of poll cycles; it should be standing by, not leading.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !ran.load(Ordering::SeqCst),
        "worker must not run while the lock is held elsewhere"
    );

    // Shutdown the standby; it should exit promptly without ever leading.
    tx.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("standby loop should stop on shutdown")
        .unwrap();

    sqlx::query_scalar::<_, bool>("SELECT pg_advisory_unlock($1)")
        .bind(lock_key)
        .fetch_one(&mut *holder)
        .await
        .unwrap();
}

// ===========================================================================
// account purger — PurgeScheduledAccountsUseCase::run_once over real repos
// ===========================================================================

/// A user still within the grace window is not touched.
#[sqlx::test(migrations = "./migrations")]
async fn purge_skips_accounts_within_grace_period(pool: PgPool) {
    let email = "e2e-purge-skip@example.com";
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        r"
        INSERT INTO users (id, email, password_hash, first_name, last_name, email_verified, deletion_scheduled_at)
        VALUES ($1, $2, 'hash', 'Real', 'Name', true, NOW() + INTERVAL '7 days')
        ",
    )
    .bind(id)
    .bind(email)
    .execute(&pool)
    .await
    .unwrap();

    let uc = purge_uc(&pool);
    assert_eq!(uc.run_once().await.unwrap(), 0);

    let still_there = sqlx::query_scalar::<_, String>("SELECT email FROM users WHERE id = $1")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(still_there, email, "user must be untouched within grace");
}

/// No accounts scheduled for deletion is a clean no-op.
#[sqlx::test(migrations = "./migrations")]
async fn purge_no_scheduled_accounts_is_noop(pool: PgPool) {
    let uc = purge_uc(&pool);
    assert_eq!(uc.run_once().await.unwrap(), 0);
}

fn purge_uc(pool: &PgPool) -> PurgeScheduledAccountsUseCase {
    let db = Arc::new(Database::from_pool(pool.clone()));
    PurgeScheduledAccountsUseCase::new(Arc::new(PostgresUserRepository::new(db)))
}

/// A user whose deletion grace period has elapsed is
/// anonymized: PII scrubbed, `deleted_at` set, `deletion_scheduled_at` cleared.
#[sqlx::test(migrations = "./migrations")]
async fn purge_finalizes_account_past_grace_period(pool: PgPool) {
    let email = "e2e-purge-me@example.com";
    let id = uuid::Uuid::new_v4();
    sqlx::query(
        r"
        INSERT INTO users (id, email, password_hash, first_name, last_name, email_verified, deletion_scheduled_at)
        VALUES ($1, $2, 'hash', 'Real', 'Name', true, NOW() - INTERVAL '1 day')
        ",
    )
    .bind(id)
    .bind(email)
    .execute(&pool)
    .await
    .unwrap();

    let uc = purge_uc(&pool);
    assert_eq!(uc.run_once().await.unwrap(), 1);

    let row = sqlx::query_as::<_, (String, Option<chrono::DateTime<chrono::Utc>>, Option<chrono::DateTime<chrono::Utc>>, Option<String>, String)>(
        "SELECT email, deleted_at, deletion_scheduled_at, password_hash, first_name FROM users WHERE id = $1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (email_after, deleted_at, deletion_scheduled_at, password_hash, first_name) = row;

    assert!(deleted_at.is_some(), "deleted_at should be set");
    assert!(
        deletion_scheduled_at.is_none(),
        "deletion_scheduled_at should be cleared"
    );
    assert!(password_hash.is_none(), "password should be scrubbed");
    assert!(
        email_after.starts_with("deleted-"),
        "email should be anonymized, got {email_after}"
    );
    assert_ne!(first_name, "Real", "name should be scrubbed");
}
