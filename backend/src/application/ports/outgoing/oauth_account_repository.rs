//! OAuth account repository port for persisting OAuth provider links.

use crate::domain::entities::{OAuthAccount, OAuthProvider};
use crate::domain::errors::DomainError;
use crate::domain::value_objects::UserId;
use async_trait::async_trait;

/// Repository for OAuth account persistence operations.
#[cfg_attr(any(test, feature = "mocks"), mockall::automock)]
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

    /// Save a new OAuth account link.
    async fn save(&self, account: &OAuthAccount) -> Result<(), DomainError>;

    /// Delete OAuth account by user and provider.
    async fn delete_by_user_and_provider(
        &self,
        user_id: &UserId,
        provider: OAuthProvider,
    ) -> Result<bool, DomainError>;

    /// Count OAuth accounts for a user.
    async fn count_by_user_id(&self, user_id: &UserId) -> Result<i64, DomainError>;
}
