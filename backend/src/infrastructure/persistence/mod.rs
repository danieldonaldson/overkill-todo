//! Persistence layer.

pub mod postgres;

pub use postgres::{
    Database, PostgresEmailVerificationRepository, PostgresOAuthAccountRepository,
    PostgresPasswordResetRepository, PostgresRefreshTokenRepository, PostgresUserRepository,
    run_migrations,
};
