//! Persistence layer.
//!
//! This module root is driver-neutral: the traits every driver adapter
//! implements for database plumbing (readiness, pool gauges, leader election,
//! maintenance), the opaque [`DatabaseError`] they fail with, and the plain
//! data they return. Repositories are reached through the application's
//! outgoing ports instead.
//!
//! The driver is chosen at compile time. Adapters live in submodules
//! (`postgres` is the sqlx one); [`Database`] names the selected adapter's
//! handle, and each adapter hands back a [`Persistence`] bundle. Plumbing
//! consumers are generic over these traits (or use [`Database`] and call only
//! trait methods), so dispatch is static. Nothing outside this module imports
//! `persistence::postgres` except `main.rs` wiring and tests.

use std::future::Future;
use std::sync::Arc;

use crate::application::ports::outgoing::{
    AccountLifecycleRepository, EmailVerificationRepository, OAuthAccountRepository,
    PasswordResetRepository, RefreshTokenRepository, UserRepository,
};

pub mod postgres;

pub use postgres::STATEMENT_LOG_DIRECTIVE;

/// The selected driver's database handle. It implements [`DatabaseHealth`],
/// [`LeaderLocks`] and [`DatabaseMaintenance`]; callers outside the adapter
/// use only those trait methods.
pub type Database = postgres::Database;

/// Every persistence handle the app needs.
///
/// Repositories are `Arc<dyn Port>` because that is what the use cases take
/// (and mock). The plumbing handle is the concrete selected [`Database`].
#[derive(Clone)]
pub struct Persistence {
    pub users: Arc<dyn UserRepository>,
    pub account_lifecycle: Arc<dyn AccountLifecycleRepository>,
    pub oauth_accounts: Arc<dyn OAuthAccountRepository>,
    pub refresh_tokens: Arc<dyn RefreshTokenRepository>,
    pub email_verifications: Arc<dyn EmailVerificationRepository>,
    pub password_resets: Arc<dyn PasswordResetRepository>,
    pub db: Arc<Database>,
}

/// A database failure from an infrastructure-internal operation (leader lock,
/// maintenance, health probe, startup) that has no domain meaning.
///
/// Wraps the driver error so callers can log or propagate it without naming
/// the driver. `Display` is the driver error's own message, unchanged.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct DatabaseError(Box<dyn std::error::Error + Send + Sync>);

impl DatabaseError {
    /// Wraps a driver error. Adapters use this (or a `From` impl built on it).
    pub fn new(error: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self(Box::new(error))
    }
}

/// Connection counts at one instant, for the pool gauges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Connections currently open (idle + in use).
    pub size: u32,
    /// Open connections not checked out.
    pub idle: usize,
}

/// Readiness probe and pool introspection.
pub trait DatabaseHealth: Send + Sync {
    /// Runs a trivial query to confirm the database is reachable.
    ///
    /// # Errors
    /// Returns the driver error if no connection is available or the query fails.
    fn ping(&self) -> impl Future<Output = Result<(), DatabaseError>> + Send;

    /// Current open and idle connection counts.
    fn pool_stats(&self) -> PoolStats;

    /// The most connections the pool may open.
    fn max_connections(&self) -> u32;
}

/// Session-level locks for leader election.
pub trait LeaderLocks: Send + Sync {
    /// A held lock and the connection holding it.
    type Lock: LeaderLock;

    /// Tries to take the lock for `key` on a dedicated connection, without
    /// waiting.
    ///
    /// Returns `Ok(None)` if another session holds it (or the lock query
    /// fails); the caller should stand by.
    ///
    /// # Errors
    /// Returns an error only if no connection can be acquired.
    fn try_lock(
        &self,
        key: i64,
    ) -> impl Future<Output = Result<Option<Self::Lock>, DatabaseError>> + Send;
}

/// A held leader lock and the connection holding it.
///
/// Always [`release`](Self::release) it: dropping may return the connection to
/// the pool with the lock still held.
pub trait LeaderLock: Send {
    /// Pings the lock's connection. An error means the connection, and with
    /// it the lock, is gone.
    ///
    /// # Errors
    /// Returns the driver error if the ping fails.
    fn ping(&mut self) -> impl Future<Output = Result<(), DatabaseError>> + Send;

    /// Unlocks and returns the connection. Best-effort: on a dead connection
    /// the lock is freed when the socket closes.
    fn release(self) -> impl Future<Output = ()> + Send;
}

/// Most rows each statement of a maintenance pass deletes.
pub const MAINTENANCE_BATCH_SIZE: i64 = 1_000;

/// Rows deleted by one maintenance pass, per table.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MaintenanceReport {
    pub refresh_tokens: u64,
    pub email_verification_codes: u64,
    pub password_reset_codes: u64,
}

impl MaintenanceReport {
    /// Total rows deleted by a pass.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.refresh_tokens + self.email_verification_codes + self.password_reset_codes
    }
}

/// Bounded deletion of expired rows, run periodically by
/// `tasks::database_maintenance`.
///
/// Each statement touches at most [`MAINTENANCE_BATCH_SIZE`] rows, keeping
/// transactions short so a backlog can't monopolise the database.
pub trait DatabaseMaintenance: Send + Sync {
    /// Runs one bounded pass.
    ///
    /// # Errors
    /// Returns the first database error; later tables wait for the next pass.
    fn purge_expired(
        &self,
    ) -> impl Future<Output = Result<MaintenanceReport, DatabaseError>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_total_sums_all_tables() {
        let report = MaintenanceReport {
            refresh_tokens: 1,
            email_verification_codes: 2,
            password_reset_codes: 3,
        };
        assert_eq!(report.total(), 6);
    }
}
