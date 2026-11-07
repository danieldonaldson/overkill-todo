//! Postgres advisory-lock leader election for singleton background workers.
//!
//! Some background work must run on exactly one replica: a poller sharing a
//! single cursor (two consumers split or lose events), a reconciler or purge
//! (duplicates race or waste work). Running it only while holding a
//! session-level advisory lock guarantees exactly one replica is active; the
//! rest stand by and take over if the leader dies.
//!
//! Robustness: the lock is held on a dedicated pooled connection and the running
//! task is raced against a heartbeat on that same connection. If the connection
//! (and thus the lock) is lost, the task is cancelled and the replica steps down,
//! so we never end up with two leaders after a transient DB blip.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use crate::infrastructure::persistence::postgres::connection::Database;

/// How long a stood-down/standby replica waits before re-attempting the lock.
const STANDBY_INTERVAL: Duration = Duration::from_secs(15);
/// How often the leader pings its lock connection to confirm it still holds it.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);

/// Runs `task_factory()` only while this replica holds the advisory lock.
///
/// Retries forever until `shutdown` flips to `true`, at which point the running
/// workers are cancelled, the advisory lock is released, and the loop returns so
/// the process can exit cleanly. `task_factory` is called fresh each time
/// leadership is (re)acquired and is expected to run until cancelled.
pub async fn run_as_leader<F, Fut>(
    db: Arc<Database>,
    lock_key: i64,
    label: &'static str,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    task_factory: F,
) where
    F: Fn() -> Fut,
    Fut: Future<Output = ()>,
{
    loop {
        if *shutdown.borrow() {
            break;
        }

        let mut conn = match db.pool().acquire().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!(label, error = %e, "Leader election: cannot acquire DB connection");
                if standby_or_shutdown(&mut shutdown).await {
                    break;
                }
                continue;
            }
        };

        let acquired = sqlx::query_scalar!("SELECT pg_try_advisory_lock($1)", lock_key)
            .fetch_one(&mut *conn)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);

        if !acquired {
            // Another replica leads; return the connection and stand by.
            drop(conn);
            tracing::debug!(label, "Leader lock held elsewhere; standing by");
            if standby_or_shutdown(&mut shutdown).await {
                break;
            }
            continue;
        }

        tracing::info!(label, "Acquired leader lock; running background workers");
        // Race the workers against a heartbeat on the lock connection and the
        // shutdown signal. If the connection dies (lock lost) the heartbeat
        // returns; on shutdown we cancel the workers and step down. Either way
        // the workers are dropped (cancelled) before we release the lock.
        let mut stopping = false;
        tokio::select! {
            () = task_factory() => {
                tracing::warn!(label, "Leader workers exited unexpectedly; will re-elect");
            }
            () = heartbeat(&mut conn, label) => {
                tracing::warn!(label, "Lost leader lock connection; stepping down");
            }
            () = wait_for_shutdown(&mut shutdown) => {
                tracing::info!(label, "Shutdown requested; cancelling leader workers");
                stopping = true;
            }
        }

        // Release explicitly: a pooled connection isn't closed on drop, so the
        // session-level lock would otherwise linger. Best-effort — on a dead
        // connection this errors and the lock is freed when the socket closes.
        let _ = sqlx::query_scalar!("SELECT pg_advisory_unlock($1)", lock_key)
            .fetch_one(&mut *conn)
            .await;
        drop(conn);

        if stopping || *shutdown.borrow() {
            break;
        }
        if standby_or_shutdown(&mut shutdown).await {
            break;
        }
    }
    tracing::info!(label, "Leader loop stopped");
}

/// Sleeps for the standby interval, returning early if shutdown is requested.
/// Returns `true` if the caller should stop (shutdown), `false` to keep looping.
async fn standby_or_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) -> bool {
    tokio::select! {
        () = tokio::time::sleep(STANDBY_INTERVAL) => false,
        () = wait_for_shutdown(shutdown) => true,
    }
}

/// Completes once the shutdown watch holds `true` (or the sender is dropped).
async fn wait_for_shutdown(shutdown: &mut tokio::sync::watch::Receiver<bool>) {
    // Already requested before we started waiting.
    if *shutdown.borrow() {
        return;
    }
    while shutdown.changed().await.is_ok() {
        if *shutdown.borrow() {
            return;
        }
    }
    // Sender dropped — treat as a shutdown request so we don't hang.
}

/// Pings the lock connection until a ping fails (the connection — and the lock —
/// is gone), then returns so the caller steps down.
async fn heartbeat(conn: &mut sqlx::pool::PoolConnection<sqlx::Postgres>, label: &str) {
    loop {
        tokio::time::sleep(HEARTBEAT_INTERVAL).await;
        if let Err(e) = sqlx::query_scalar!("SELECT 1").fetch_one(&mut **conn).await {
            tracing::error!(label, error = %e, "Leader heartbeat failed");
            return;
        }
    }
}
