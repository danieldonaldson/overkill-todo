//! `PostgreSQL` connection pool management.

use sqlx::{ConnectOptions, PgPool, postgres::PgConnectOptions, postgres::PgPoolOptions};
use std::{str::FromStr, time::Duration};
use tracing::log::LevelFilter;

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
}

/// Creates a `PostgreSQL` connection pool.
///
/// # Errors
/// Returns an error if the connection string is invalid or the pool cannot be created.
pub async fn create_pool(database_url: &str, max_connections: u32) -> Result<PgPool, sqlx::Error> {
    let connect_options = PgConnectOptions::from_str(database_url)?
        .log_statements(LevelFilter::Debug)
        .log_slow_statements(LevelFilter::Warn, Duration::from_millis(100));

    PgPoolOptions::new()
        .max_connections(max_connections)
        .acquire_timeout(Duration::from_secs(10))
        .connect_with(connect_options)
        .await
}

/// Runs database migrations from the migrations directory.
///
/// # Errors
/// Returns an error if migrations fail to run.
pub async fn run_migrations(pool: &PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("./migrations").run(pool).await
}
