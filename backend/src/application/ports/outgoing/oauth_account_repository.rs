//! OAuth account repository port for persisting OAuth provider links.

use crate::domain::entities::{AuditEvent, OAuthAccount, OAuthProvider};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use async_trait::async_trait;

/// Repository for OAuth account persistence operations.
#[cfg_attr(test, mockall::automock)]
#[async_trait]
pub trait OAuthAccountRepository: Send + Sync {
    /// Find OAuth account by provider and provider's user ID.
    async fn find_by_provider_user_id(
        &self,
        provider: OAuthProvider,
        provider_user_id: &str,
    ) -> Result<Option<OAuthAccount>, DomainError>;

    /// Find all OAuth accounts for a user.
    async fn find_by_user_id(&self, user_id: &UserId) -> Result<Vec<OAuthAccount>, DomainError>;

    /// Save a new OAuth account link, storing `event` (sign-in method linked)
    /// in the same transaction (OT-008).
    async fn save(&self, account: &OAuthAccount, event: &AuditEvent) -> Result<(), DomainError>;

    /// Delete OAuth account by user and provider. When a link was deleted,
    /// `event` (sign-in method unlinked) is stored in the same transaction;
    /// when there was none (`Ok(false)`), nothing is written.
    async fn delete_by_user_and_provider(
        &self,
        user_id: &UserId,
        provider: OAuthProvider,
        event: &AuditEvent,
    ) -> Result<bool, DomainError>;

    /// Delete every OAuth link for a user. Returns how many links were removed.
    async fn delete_all_for_user(&self, user_id: &UserId) -> Result<u64, DomainError>;

    /// Count OAuth accounts for a user.
    async fn count_by_user_id(&self, user_id: &UserId) -> Result<i64, DomainError>;
}
