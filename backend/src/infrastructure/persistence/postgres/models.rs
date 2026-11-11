use crate::domain::entities::OAuthProvider;
use crate::domain::value_objects::UserRole;
use chrono::{DateTime, Utc};
use sqlx::FromRow;
use uuid::Uuid;

/// Postgres `user_role` enum. Mirrors [`UserRole`] so the domain type carries no
/// `sqlx` derive; convert at the repository boundary with `From`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "user_role", rename_all = "snake_case")]
pub enum DbUserRole {
    User,
    Admin,
    SuperAdmin,
}

impl From<UserRole> for DbUserRole {
    fn from(role: UserRole) -> Self {
        match role {
            UserRole::User => Self::User,
            UserRole::Admin => Self::Admin,
            UserRole::SuperAdmin => Self::SuperAdmin,
        }
    }
}

impl From<DbUserRole> for UserRole {
    fn from(role: DbUserRole) -> Self {
        match role {
            DbUserRole::User => Self::User,
            DbUserRole::Admin => Self::Admin,
            DbUserRole::SuperAdmin => Self::SuperAdmin,
        }
    }
}

/// Postgres `oauth_provider` enum. Mirrors [`OAuthProvider`]; see [`DbUserRole`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type)]
#[sqlx(type_name = "oauth_provider", rename_all = "snake_case")]
pub enum DbOAuthProvider {
    Google,
    Apple,
}

impl From<OAuthProvider> for DbOAuthProvider {
    fn from(provider: OAuthProvider) -> Self {
        match provider {
            OAuthProvider::Google => Self::Google,
            OAuthProvider::Apple => Self::Apple,
        }
    }
}

impl From<DbOAuthProvider> for OAuthProvider {
    fn from(provider: DbOAuthProvider) -> Self {
        match provider {
            DbOAuthProvider::Google => Self::Google,
            DbOAuthProvider::Apple => Self::Apple,
        }
    }
}

#[derive(Debug, FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub email: String,
    pub password_hash: Option<String>,
    pub first_name: String,
    pub last_name: String,
    pub email_verified: bool,
    pub pending_email: Option<String>,
    pub pending_email_requested_at: Option<DateTime<Utc>>,
    pub deleted_at: Option<DateTime<Utc>>,
    pub deletion_scheduled_at: Option<DateTime<Utc>>,
    pub role: DbUserRole,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub version: i64,
}

#[derive(Debug, FromRow)]
pub struct RefreshTokenRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub token_hash: String,
    pub device_fingerprint: String,
    pub expires_at: DateTime<Utc>,
    pub revoked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, FromRow)]
pub struct EmailVerificationCodeRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub code_hash: String,
    pub expires_at: DateTime<Utc>,
    pub attempts: i32,
    pub last_sent_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub magic_token_hash: Option<String>,
    pub magic_token_expires_at: Option<DateTime<Utc>>,
    pub lockouts: i32,
}

#[derive(Debug, FromRow)]
pub struct PasswordResetCodeRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub code_hash: String,
    pub expires_at: DateTime<Utc>,
    pub attempts: i32,
    pub last_sent_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub lockouts: i32,
}

#[derive(Debug, FromRow)]
pub struct OAuthAccountRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: DbOAuthProvider,
    pub provider_user_id: String,
    pub provider_email: Option<String>,
    pub linked_at: DateTime<Utc>,
}
