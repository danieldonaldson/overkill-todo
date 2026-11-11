//! Session-level Postgres advisory locks for leader election.
//!
//! A session-level advisory lock belongs to the database connection that took
//! it, so an [`AdvisoryLock`] owns the dedicated pooled connection it was
//! acquired on. Heartbeats run on that same connection, and [`AdvisoryLock::release`]
//! unlocks on it before handing it back to the pool.

use sqlx::{Postgres, pool::PoolConnection};

use super::{Database, error::DatabaseError};

/// A held session-level advisory lock and the connection holding it.
///
/// Dropping it without [`release`](Self::release) returns the connection to
/// the pool with the lock still held: a pooled connection isn't closed on
/// drop, so always release explicitly.
pub struct AdvisoryLock {
    conn: PoolConnection<Postgres>,
    key: i64,
}

impl Database {
    /// Takes a dedicated connection from the pool and tries
    /// `pg_try_advisory_lock(key)` on it, without waiting.
    ///
    /// Returns `Ok(Some(lock))` if granted. Returns `Ok(None)` if another
    /// session holds the lock, or if the lock query itself fails; either way
    /// the connection goes back to the pool and the caller should stand by.
    ///
    /// # Errors
    /// Returns an error only if no connection can be acquired from the pool.
    pub async fn try_advisory_lock(&self, key: i64) -> Result<Option<AdvisoryLock>, DatabaseError> {
        let mut conn = self.pool().acquire().await?;

        let acquired = sqlx::query_scalar!("SELECT pg_try_advisory_lock($1)", key)
            .fetch_one(&mut *conn)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);

        Ok(acquired.then_some(AdvisoryLock { conn, key }))
    }
}

impl AdvisoryLock {
    /// Pings the lock's connection. An error means the connection — and with
    /// it the lock — is gone.
    ///
    /// # Errors
    /// Returns the driver error if the ping fails.
    pub async fn ping(&mut self) -> Result<(), DatabaseError> {
        sqlx::query_scalar!("SELECT 1")
            .fetch_one(&mut *self.conn)
            .await?;
        Ok(())
    }

    /// Unlocks and returns the connection to the pool.
    ///
    /// Best-effort: on a dead connection the unlock errors and the lock is
    /// freed when the socket closes.
    pub async fn release(mut self) {
        let _ = sqlx::query_scalar!("SELECT pg_advisory_unlock($1)", self.key)
            .fetch_one(&mut *self.conn)
            .await;
        drop(self.conn);
    }
}
