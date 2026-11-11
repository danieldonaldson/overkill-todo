//! sqlx Postgres adapter: repositories plus the [`Database`] handle, which
//! implements the driver-neutral plumbing traits in the parent module.

use std::sync::Arc;

use super::{DatabaseError, Persistence};

mod advisory_lock;
pub mod connection;
pub mod email_verification_repository;
pub mod error;
mod maintenance;

pub mod models;
pub mod oauth_account_repository;
pub mod password_reset_repository;
pub mod refresh_token_repository;

pub mod user_repository;

pub use connection::{Database, STATEMENT_LOG_DIRECTIVE};
pub use email_verification_repository::PostgresEmailVerificationRepository;

pub use oauth_account_repository::PostgresOAuthAccountRepository;
pub use password_reset_repository::PostgresPasswordResetRepository;
pub use refresh_token_repository::PostgresRefreshTokenRepository;

pub use user_repository::PostgresUserRepository;

/// Connects, runs pending migrations, and wires every port to this adapter.
///
/// # Errors
/// Returns an error if the pool can't be created or a migration fails.
pub async fn open(database_url: &str, max_connections: u32) -> Result<Persistence, DatabaseError> {
    let db = Database::new(database_url, max_connections).await?;
    db.run_migrations().await?;
    tracing::info!("Database migrations applied");
    Ok(wire(Arc::new(db)))
}

/// Wires every port to an existing handle. Tests use this with a
/// `#[sqlx::test]` pool, which is already migrated.
#[must_use]
pub fn wire(db: Arc<Database>) -> Persistence {
    let users = Arc::new(PostgresUserRepository::new(db.clone()));
    Persistence {
        users: users.clone(),
        account_lifecycle: users,
        oauth_accounts: Arc::new(PostgresOAuthAccountRepository::new(db.clone())),
        refresh_tokens: Arc::new(PostgresRefreshTokenRepository::new(db.clone())),
        email_verifications: Arc::new(PostgresEmailVerificationRepository::new(db.clone())),
        password_resets: Arc::new(PostgresPasswordResetRepository::new(db.clone())),
        health: db.clone(),
        leader_locks: db.clone(),
        maintenance: db,
    }
}
