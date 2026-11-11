//! Session-level Postgres advisory locks for leader election.
//!
//! A session-level advisory lock belongs to the database connection that took
//! it, so an [`AdvisoryLock`] owns the dedicated pooled connection it was
//! acquired on. Heartbeats run on that same connection, and `release` unlocks
//! on it before handing it back to the pool.

use async_trait::async_trait;
use sqlx::{Postgres, pool::PoolConnection};

use super::Database;
use crate::infrastructure::persistence::{DatabaseError, LeaderLock, LeaderLocks};

/// A held session-level advisory lock and the connection holding it.
///
/// Dropping it without `release` returns the connection to the pool with the
/// lock still held: a pooled connection isn't closed on drop.
struct AdvisoryLock {
    conn: PoolConnection<Postgres>,
    key: i64,
}

#[async_trait]
impl LeaderLocks for Database {
    /// `pg_try_advisory_lock(key)` on a dedicated pooled connection. If the
    /// lock isn't granted (or the query fails) the connection goes straight
    /// back to the pool.
    async fn try_lock(&self, key: i64) -> Result<Option<Box<dyn LeaderLock>>, DatabaseError> {
        let mut conn = self.pool().acquire().await?;

        let acquired = sqlx::query_scalar!("SELECT pg_try_advisory_lock($1)", key)
            .fetch_one(&mut *conn)
            .await
            .ok()
            .flatten()
            .unwrap_or(false);

        Ok(acquired.then(|| Box::new(AdvisoryLock { conn, key }) as Box<dyn LeaderLock>))
    }
}

#[async_trait]
impl LeaderLock for AdvisoryLock {
    async fn ping(&mut self) -> Result<(), DatabaseError> {
        sqlx::query_scalar!("SELECT 1")
            .fetch_one(&mut *self.conn)
            .await?;
        Ok(())
    }

    async fn release(self: Box<Self>) {
        let Self { mut conn, key } = *self;
        let _ = sqlx::query_scalar!("SELECT pg_advisory_unlock($1)", key)
            .fetch_one(&mut *conn)
            .await;
        drop(conn);
    }
}
