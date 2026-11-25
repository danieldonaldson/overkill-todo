//! sqlx Postgres adapter: repositories plus the [`Database`] handle, which
//! implements the driver-neutral plumbing traits in [`platform::persistence`].
#![allow(
    clippy::double_must_use,
    reason = "`#[async_trait]` marks its boxed futures `#[must_use]`; not actionable here"
)]

use std::sync::Arc;

use application::ports::outgoing::{
    AccountLifecycleRepository, AuditLogRepository, EmailVerificationRepository, ListRepository,
    OAuthAccountRepository, PasswordResetRepository, RefreshTokenRepository, UserRepository,
};
use platform::persistence::DatabaseError;

mod advisory_lock;
pub mod audit_log;
pub mod connection;
pub mod email_verification_repository;
pub mod error;
pub mod list_repository;
mod maintenance;

pub mod models;
pub mod oauth_account_repository;
pub mod password_reset_repository;
pub mod refresh_token_repository;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;
pub mod time;

pub mod user_repository;

pub use audit_log::PostgresAuditLogRepository;
pub use connection::{Database, STATEMENT_LOG_DIRECTIVE};
pub use email_verification_repository::PostgresEmailVerificationRepository;
pub use list_repository::PostgresListRepository;

pub use oauth_account_repository::PostgresOAuthAccountRepository;
pub use password_reset_repository::PostgresPasswordResetRepository;
pub use refresh_token_repository::PostgresRefreshTokenRepository;

pub use user_repository::PostgresUserRepository;

/// Every persistence handle the app needs.
#[derive(Clone)]
pub struct Persistence {
    pub users: Arc<dyn UserRepository>,
    pub account_lifecycle: Arc<dyn AccountLifecycleRepository>,
    pub audit_log: Arc<dyn AuditLogRepository>,
    pub oauth_accounts: Arc<dyn OAuthAccountRepository>,
    pub refresh_tokens: Arc<dyn RefreshTokenRepository>,
    pub email_verifications: Arc<dyn EmailVerificationRepository>,
    pub password_resets: Arc<dyn PasswordResetRepository>,
    pub lists: Arc<dyn ListRepository>,
    pub db: Arc<Database>,
}

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
        audit_log: Arc::new(PostgresAuditLogRepository::new(db.clone())),
        oauth_accounts: Arc::new(PostgresOAuthAccountRepository::new(db.clone())),
        refresh_tokens: Arc::new(PostgresRefreshTokenRepository::new(db.clone())),
        email_verifications: Arc::new(PostgresEmailVerificationRepository::new(db.clone())),
        password_resets: Arc::new(PostgresPasswordResetRepository::new(db.clone())),
        lists: Arc::new(PostgresListRepository::new(db.clone())),
        db,
    }
}
