//! `PostgreSQL` connection pool management.

use sqlx::{ConnectOptions, PgPool, postgres::PgConnectOptions, postgres::PgPoolOptions};
use std::{str::FromStr, time::Duration};
use tracing::log::LevelFilter;

use super::error::DatabaseError;

/// `tracing` filter directive for the driver's per-statement events (SQL text
/// only, never bound values), as emitted under [`with_statement_logging`].
pub const STATEMENT_LOG_DIRECTIVE: &str = "sqlx::query=info";

/// Connection counts at one instant, for the pool gauges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PoolStats {
    /// Connections currently open (idle + in use).
    pub size: u32,
    /// Open connections not checked out.
    pub idle: usize,
}

/// Holds a single database connection pool.
#[derive(Clone)]
pub struct Database {
    pool: PgPool,
}

impl Database {
    /// Creates a database pool from a connection URL.
    ///
    /// # Errors
    /// Returns an error if the connection string is invalid or the pool cannot be created.
    pub async fn new(database_url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = create_pool(database_url, max_connections).await?;
        Ok(Self { pool })
    }

    /// Creates a [`Database`] from an existing pool (for testing).
    #[must_use]
    pub const fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Returns the connection pool for reads and writes.
    #[must_use]
    pub const fn pool(&self) -> &PgPool {
        &self.pool
    }

    /// Runs `SELECT 1` to confirm the database is reachable (readiness probe).
    ///
    /// # Errors
    /// Returns the driver error if no connection is available or the query fails.
    pub async fn ping(&self) -> Result<(), DatabaseError> {
        sqlx::query!("SELECT 1 as ping")
            .fetch_one(&self.pool)
            .await?;
        Ok(())
    }

    /// Current open and idle connection counts.
    #[must_use]
    pub fn pool_stats(&self) -> PoolStats {
        PoolStats {
            size: self.pool.size(),
            idle: self.pool.num_idle(),
        }
    }

    /// The most connections the pool may open.
    #[must_use]
    pub fn max_connections(&self) -> u32 {
        self.pool.options().get_max_connections()
    }
}

/// Creates a `PostgreSQL` connection pool.
///
/// # Errors
/// Returns an error if the connection string is invalid or the pool cannot be created.
pub async fn create_pool(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let connect_options = with_statement_logging(PgConnectOptions::from_str(database_url)?);

    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(connect_options)
        .await
}

/// Statement logging used by every production pool.
///
/// Statements are logged at info so the trace export filter
/// ([`STATEMENT_LOG_DIRECTIVE`]) attaches each one's SQL text (never bound values) to
/// its `repo.*` span. The default stdout filter has no `sqlx` directive, so
/// they stay out of the logs. Public so tests can apply the same settings to a
/// `#[sqlx::test]` pool.
#[must_use]
pub fn with_statement_logging(options: PgConnectOptions) -> PgConnectOptions {
    options
        .log_statements(LevelFilter::Info)
        .log_slow_statements(LevelFilter::Warn, Duration::from_millis(100))
}

/// Runs database migrations from the migrations directory.
///
/// # Errors
/// Returns an error if migrations fail to run.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
