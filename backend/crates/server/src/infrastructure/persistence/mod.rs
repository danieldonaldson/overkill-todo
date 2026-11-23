//! Persistence adapter selection (`postgres` until Slice 2b moves it to its own crate).

use std::sync::Arc;

use application::ports::outgoing::{
    AccountLifecycleRepository, AuditLogRepository, EmailVerificationRepository,
    OAuthAccountRepository, PasswordResetRepository, RefreshTokenRepository, UserRepository,
};

pub mod postgres;

pub use platform::persistence::{
    DatabaseError, DatabaseHealth, DatabaseMaintenance, LeaderLock, LeaderLocks,
    MAINTENANCE_BATCH_SIZE, MaintenanceReport, PoolStats,
};
pub use postgres::STATEMENT_LOG_DIRECTIVE;

/// The selected driver's database handle.
pub type Database = postgres::Database;

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
    pub db: Arc<Database>,
}
